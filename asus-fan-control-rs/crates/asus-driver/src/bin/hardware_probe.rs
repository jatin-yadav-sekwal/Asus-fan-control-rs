/// Definitive DLL probe — tests AsusWinIO64.dll from SYSTEM context
/// to verify the DLL works exactly like the original C# AsusFanControl app.
///
/// The original C# app runs as SYSTEM via `PsExec -i -s -d` and the DLL works.
/// This probe replicates that exact pattern.

fn main() {
    println!("=== AsusWinIO64 DLL Definitive Probe ===\n");

    // Print security context
    println!("Process ID: {}", std::process::id());

    // Try to load the DLL from multiple locations
    let dll_paths = [
        r"AsusWinIO64.dll",
        r"C:\Users\Asus\Downloads\AsusFanControl\AsusWinIO64.dll",
        r"C:\Users\Asus\Downloads\AsusFanControl\AsusFanControl\AsusFanControl\AsusWinIO64.dll",
    ];

    let mut lib: Option<libloading::Library> = None;

    for path in &dll_paths {
        if std::path::Path::new(path).exists() {
            match unsafe { libloading::Library::new(path) } {
                Ok(l) => {
                    println!("[OK] Loaded DLL from: {}", path);
                    lib = Some(l);
                    break;
                }
                Err(e) => {
                    println!("[FAIL] {} -> {:?}", path, e);
                }
            }
        }
    }

    let lib = match lib {
        Some(l) => l,
        None => {
            println!("FATAL: Could not load AsusWinIO64.dll from any location!");
            return;
        }
    };

    unsafe {
        // Load all function pointers
        let init: libloading::Symbol<unsafe extern "system" fn()> =
            lib.get(b"InitializeWinIo\0").expect("InitializeWinIo not found");
        let shutdown: libloading::Symbol<unsafe extern "system" fn()> =
            lib.get(b"ShutdownWinIo\0").expect("ShutdownWinIo not found");
        let fan_counts: libloading::Symbol<unsafe extern "system" fn() -> i32> =
            lib.get(b"HealthyTable_FanCounts\0").expect("FanCounts not found");
        let set_fan_index: libloading::Symbol<unsafe extern "system" fn(u8)> =
            lib.get(b"HealthyTable_SetFanIndex\0").expect("SetFanIndex not found");
        let fan_rpm: libloading::Symbol<unsafe extern "system" fn() -> i32> =
            lib.get(b"HealthyTable_FanRPM\0").expect("FanRPM not found");
        let get_test_mode: libloading::Symbol<unsafe extern "system" fn() -> i32> =
            lib.get(b"HealthyTable_GetFanTestMode\0").expect("GetFanTestMode not found");
        let set_test_mode: libloading::Symbol<unsafe extern "system" fn(u16)> =
            lib.get(b"HealthyTable_SetFanTestMode\0").expect("SetFanTestMode not found");
        let set_pwm: libloading::Symbol<unsafe extern "system" fn(i16)> =
            lib.get(b"HealthyTable_SetFanPwmDuty\0").expect("SetFanPwmDuty not found");
        let cpu_temp: libloading::Symbol<unsafe extern "system" fn() -> u64> =
            lib.get(b"Thermal_Read_Cpu_Temperature\0").expect("Thermal not found");

        println!("[OK] All symbols resolved\n");

        // STAGE 1: Initialize
        println!("--- STAGE 1: InitializeWinIo ---");
        init();
        std::thread::sleep(std::time::Duration::from_millis(200));
        println!("  InitializeWinIo() called, waited 200ms\n");

        // STAGE 2: Basic reads
        println!("--- STAGE 2: Basic Reads ---");
        let fc = fan_counts();
        println!("  HealthyTable_FanCounts() = {}", fc);

        let temp = cpu_temp();
        println!("  Thermal_Read_Cpu_Temperature() = {} (0x{:X})", temp, temp);

        // STAGE 3: Fan RPM for each fan
        println!("\n--- STAGE 3: Fan RPM Readings ---");
        for i in 0..2u8 {
            set_fan_index(i);
            std::thread::sleep(std::time::Duration::from_millis(30));
            let rpm = fan_rpm();
            let tm = get_test_mode();
            println!("  Fan #{}: RPM={}, TestMode={}", i, rpm, tm);
        }

        // STAGE 4: Live monitoring (5 readings, 1s apart)
        println!("\n--- STAGE 4: Live Monitoring (5 readings) ---");
        for tick in 0..5 {
            set_fan_index(0);
            std::thread::sleep(std::time::Duration::from_millis(20));
            let rpm = fan_rpm();
            let temp_val = cpu_temp();
            println!("  [t={}] Fan#0 RPM={}, CPU Temp={}", tick, rpm, temp_val);
            if tick < 4 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }

        // STAGE 5: Fan control test — set fan 0 to 100% duty
        println!("\n--- STAGE 5: Fan Control Test (100% for 3s, then restore) ---");
        set_fan_index(0);
        set_test_mode(0x01);
        set_pwm(255);
        println!("  Set Fan#0: TestMode=1, PWM=255 (100%)");
        
        // Check if it took effect
        std::thread::sleep(std::time::Duration::from_millis(500));
        set_fan_index(0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let rpm_after = fan_rpm();
        let tm_after = get_test_mode();
        println!("  After set: RPM={}, TestMode={}", rpm_after, tm_after);

        // Wait 3 seconds at 100%
        for tick in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            set_fan_index(0);
            std::thread::sleep(std::time::Duration::from_millis(20));
            let rpm = fan_rpm();
            println!("  [+{}s] Fan#0 RPM={}", tick + 1, rpm);
        }

        // STAGE 6: Restore to BIOS control
        println!("\n--- STAGE 6: Restore BIOS Control ---");
        for i in 0..2u8 {
            set_fan_index(i);
            set_test_mode(0x00);
            set_pwm(0);
            println!("  Fan#{}: TestMode=0, PWM=0 (BIOS control)", i);
        }

        // Verify restoration
        std::thread::sleep(std::time::Duration::from_secs(1));
        for i in 0..2u8 {
            set_fan_index(i);
            std::thread::sleep(std::time::Duration::from_millis(20));
            let rpm = fan_rpm();
            let tm = get_test_mode();
            println!("  Fan#{} after restore: RPM={}, TestMode={}", i, rpm, tm);
        }

        // Shutdown
        println!("\n--- Shutdown ---");
        shutdown();
        println!("  ShutdownWinIo() called");
    }

    println!("\n[DONE] DLL probe complete.");
}
