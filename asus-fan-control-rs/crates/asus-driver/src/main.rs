use clap::{Parser, Subcommand};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use asus_driver::{AsusDriver, DriverError};

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

    let mut driver = match AsusDriver::new() {
        Ok(d) => d,
        Err(DriverError::ElevationRequired) => {
            eprintln!("[-] Error: Access Denied.");
            eprintln!("    Communicating with the ASUS kernel driver (\\.\\AsusWinIO) requires Administrator privileges.");
            eprintln!("\n[>] Please right-click run-cli.bat and choose 'Run as administrator'");
            eprintln!("    or run from an elevated command prompt.\n");
            eprintln!("Press Enter to exit...");
            let _ = std::io::stdin().read_line(&mut String::new());
            std::process::exit(1);
        }
        Err(DriverError::DllNotFound(paths)) => {
            eprintln!("[-] Error: AsusWinIO64.dll was not found.");
            eprintln!("    Searched locations:");
            for p in paths {
                eprintln!("      - {:?}", p);
            }
            eprintln!("\nPlease copy AsusWinIO64.dll to the current directory or install MyASUS.");
            eprintln!("\nPress Enter to exit...");
            let _ = std::io::stdin().read_line(&mut String::new());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("[-] Failed to initialize driver: {}", e);
            eprintln!("\nPress Enter to exit...");
            let _ = std::io::stdin().read_line(&mut String::new());
            std::process::exit(1);
        }
    };

    let command = cli.command.unwrap_or(Commands::Status);

    match command {
        Commands::Status => {
            print_status(&mut driver)?;
        }
        Commands::SetAll { percent } => {
            if percent == 0 {
                println!("[*] Resetting all fans to BIOS automatic control...");
                driver.reset_all_to_bios()?;
                println!("[+] Successfully restored BIOS curve.");
            } else {
                println!("[*] Setting all fans to {}%...", percent);
                driver.set_all_fans_percent(percent)?;
                println!("[+] Successfully updated all fans to {}%.", percent);
            }
            std::thread::sleep(Duration::from_millis(500));
            print_status(&mut driver)?;
        }
        Commands::Set { fan_id, percent } => {
            if percent == 0 {
                println!("[*] Resetting Fan #{} to BIOS control...", fan_id);
                driver.reset_fan_to_bios(fan_id)?;
            } else {
                println!("[*] Setting Fan #{} to {}%...", fan_id, percent);
                driver.set_fan_percent(fan_id, percent)?;
            }
            std::thread::sleep(Duration::from_millis(500));
            print_status(&mut driver)?;
        }
        Commands::Reset => {
            println!("[*] Restoring all fans to factory BIOS control...");
            driver.reset_all_to_bios()?;
            println!("[+] Reset complete.");
            print_status(&mut driver)?;
        }
        Commands::Monitor { interval_ms } => {
            println!("[*] Starting live monitor (Interval: {}ms). Press Ctrl+C to exit...", interval_ms);
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let r = running.clone();

            ctrlc::set_handler(move || {
                r.store(false, std::sync::atomic::Ordering::SeqCst);
                println!("\n[*] Stopping monitor and safely restoring BIOS control...");
            }).ok();

            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let temp = driver.get_cpu_temperature().unwrap_or(0);
                let fan_count = driver.fan_count();
                let mut fan_str = String::new();

                for i in 0..fan_count {
                    let rpm = driver.get_fan_rpm(i).unwrap_or(0);
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

fn print_status(driver: &mut AsusDriver) -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = driver.get_system_telemetry()?;

    println!("[+] Hardware Status:");
    println!("    CPU Temperature : {}°C", telemetry.cpu_temp_c);
    println!("    Controllable Fans: {}", telemetry.fans.len());

    for fan in &telemetry.fans {
        println!("    - {}: {} RPM", fan.id, fan.rpm);
    }
    println!();
    Ok(())
}
