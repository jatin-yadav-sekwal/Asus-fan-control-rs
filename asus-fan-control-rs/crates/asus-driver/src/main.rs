use clap::{Parser, Subcommand};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use asus_driver::{DriverError, SafeAsusDriver, HELPER_ARG, PIPE_ARG_PREFIX};

#[derive(Parser)]
#[command(name = "asus-driver-cli")]
#[command(about = "Direct hardware test harness and CLI for AsusFanControl in Rust", long_about = None)]
struct Cli {
    #[arg(short, long, help = "Enable verbose debug logs")]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Read and display current CPU temperature and fan RPMs
    Status,

    /// Set speed for all fans (0-100%, 0 restores BIOS control)
    SetAll {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },

    /// Set speed for a specific fan index (0-100%, 0 restores BIOS control)
    Set {
        #[arg(help = "Fan index (0 for CPU, 1 for GPU)")]
        fan_id: u8,
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },

    /// Reset all fans back to factory BIOS automatic control
    Reset,

    /// Live telemetry monitor loop (press Ctrl+C to stop)
    Monitor {
        #[arg(short, long, default_value_t = 1000, help = "Poll interval in milliseconds")]
        interval_ms: u64,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Headless SYSTEM worker: started by our own scheduled task, never prints.
    if let Some(pipe) = std::env::args()
        .find_map(|arg| arg.strip_prefix(PIPE_ARG_PREFIX).map(|s| s.to_string()))
    {
        std::process::exit(asus_driver::helper::run(&pipe));
    }

    let cli = Cli::parse();

    let filter = if cli.verbose {
        "debug,asus_driver=debug"
    } else {
        "info,asus_driver=info"
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(filter))
        .with(tracing_subscriber::fmt::layer())
        .init();

    println!("=====================================================");
    println!("  Asus Fan Control - Rust Hardware HAL Test (ACPI/WinIO) ");
    println!("=====================================================\n");

    // Exactly one session per machine: a second one would delete the running
    // helper's scheduled task and start a competitor against the same EC.
    if !asus_driver::acquire_single_instance() {
        eprintln!("[-] Asus Fan Control is already running (GUI or another CLI).");
        eprintln!("    Close it first — only one client can own \\\\.\\AsusSAIO at a time.");
        std::process::exit(1);
    }

    // The scheduled task is created under SYSTEM, which needs admin rights.
    if !asus_driver::is_elevated() {
        eprintln!("[*] Administrator privileges are required to start the SYSTEM helper.");
        eprintln!("[*] Relaunching elevated...\n");
        asus_driver::release_single_instance();
        let args = requote_args();
        match asus_driver::relaunch_elevated_with_args(&args) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("[-] UAC elevation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    let driver = match attach_to_helper() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[-] Could not reach the SYSTEM helper: {}", notice(&e));
            asus_driver::stop_system_task();
            asus_driver::release_single_instance();
            std::process::exit(1);
        }
    };

    let result = run_command(&driver, cli.command.unwrap_or(Commands::Status));

    // Dropping the driver closes the pipe; the helper then restores BIOS
    // control and removes its own task. This is the orderly path.
    drop(driver);
    std::thread::sleep(Duration::from_millis(400));
    asus_driver::stop_system_task();
    asus_driver::release_single_instance();

    result
}

/// Starts the SYSTEM helper (the shared `--sys-helper` implementation) and
/// attaches to it over the control pipe.
fn attach_to_helper() -> Result<SafeAsusDriver, DriverError> {
    let exe = std::env::current_exe().map_err(|e| DriverError::DriverNotReady {
        reason: format!("cannot resolve own path: {}", e),
    })?;

    let pipe = asus_driver::make_pipe_name();
    asus_driver::start_system_task(&exe, &[HELPER_ARG, &format!("{}{}", PIPE_ARG_PREFIX, pipe)])
        .map_err(|e| DriverError::DriverNotReady {
            reason: format!("could not start helper: {}", e),
        })?;

    match SafeAsusDriver::connect(&pipe) {
        Ok(d) => Ok(d),
        Err(e) => {
            asus_driver::stop_system_task();
            Err(e)
        }
    }
}

fn run_command(
    driver: &SafeAsusDriver,
    command: Commands,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Commands::Status => {
            print_status(driver)?;
        }
        Commands::SetAll { percent } => {
            if percent == 0 {
                println!("[*] Resetting all fans to BIOS automatic control...");
                driver.lock().reset_all_to_bios()?;
                println!("[+] Successfully restored BIOS curve.");
            } else {
                println!("[*] Setting all fans to {}%...", percent);
                driver.lock().set_all_fans_percent(percent)?;
                println!("[+] Successfully updated all fans to {}%.", percent);
            }
            std::thread::sleep(Duration::from_millis(500));
            print_status(driver)?;
        }
        Commands::Set { fan_id, percent } => {
            if percent == 0 {
                println!("[*] Resetting Fan #{} to BIOS control...", fan_id);
                driver.lock().reset_fan_to_bios(fan_id)?;
            } else {
                println!("[*] Setting Fan #{} to {}%...", fan_id, percent);
                driver.lock().set_fan_percent(fan_id, percent)?;
            }
            std::thread::sleep(Duration::from_millis(500));
            print_status(driver)?;
        }
        Commands::Reset => {
            println!("[*] Restoring all fans to factory BIOS control...");
            driver.lock().reset_all_to_bios()?;
            println!("[+] Reset complete.");
            print_status(driver)?;
        }
        Commands::Monitor { interval_ms } => {
            println!(
                "[*] Starting live monitor (Interval: {}ms). Press Ctrl+C to exit...",
                interval_ms
            );
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let r = running.clone();

            ctrlc::set_handler(move || {
                r.store(false, std::sync::atomic::Ordering::SeqCst);
                println!("\n[*] Stopping monitor and safely restoring BIOS control...");
            })
            .ok();

            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let temp = driver.lock().get_cpu_temperature().unwrap_or(0);
                let fan_count = driver.lock().fan_count();
                let mut fan_str = String::new();

                for i in 0..fan_count {
                    let rpm = driver.lock().get_fan_rpm(i).unwrap_or(0);
                    fan_str.push_str(&format!("Fan #{}: {:4} RPM   ", i, rpm));
                }

                print!("\r[CPU Temp: {:2}°C] | {}", temp, fan_str);
                std::io::Write::flush(&mut std::io::stdout())?;
                std::thread::sleep(Duration::from_millis(interval_ms));
            }
            println!();
        }
    }

    Ok(())
}

fn print_status(driver: &SafeAsusDriver) -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = driver.lock().get_system_telemetry()?;

    println!("[+] Hardware Status:");
    println!("    CPU Temperature : {}°C", telemetry.cpu_temp_c);
    println!("    Controllable Fans: {}", telemetry.fans.len());

    for fan in &telemetry.fans {
        println!("    - {}: {} RPM", fan.id, fan.rpm);
    }
    println!();
    Ok(())
}

fn notice(err: &DriverError) -> String {
    match err {
        DriverError::ElevationRequired | DriverError::HealthyTableUnresponsive { .. } => {
            "\\\\.\\AsusSAIO answered with -1, which means the helper is not running as \
             NT AUTHORITY\\SYSTEM. Check the helper log under %LOCALAPPDATA%\\AsusFanControl."
                .to_string()
        }
        other => other.to_string(),
    }
}

/// Rebuilds a command line for the elevated copy of ourselves.
fn requote_args() -> String {
    std::env::args()
        .skip(1)
        .map(|arg| {
            if arg.contains(' ') || arg.contains('"') {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
