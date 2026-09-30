use std::ffi::OsStr;
use std::os::raw::c_void;
use std::os::windows::ffi::OsStrExt;
use tracing::{debug, info};

type PdhHQuery = *mut c_void;
type PdhHCounter = *mut c_void;

#[repr(C)]
struct PdhFmtCounterValue {
    c_status: u32,
    double_value: f64,
}

pub struct WindowsThermalReader {
    _lib: libloading::Library,
    h_query: PdhHQuery,
    h_counter: PdhHCounter,
    is_high_precision: bool,
    pdh_collect_query_data: unsafe extern "system" fn(PdhHQuery) -> u32,
    pdh_get_formatted_counter_value:
        unsafe extern "system" fn(PdhHCounter, u32, *mut u32, *mut PdhFmtCounterValue) -> u32,
    pdh_close_query: unsafe extern "system" fn(PdhHQuery) -> u32,
}

unsafe impl Send for WindowsThermalReader {}
unsafe impl Sync for WindowsThermalReader {}

impl WindowsThermalReader {
    pub fn new() -> Option<Self> {
        let lib = unsafe { libloading::Library::new("pdh.dll").ok()? };

        let pdh_open_query_w: unsafe extern "system" fn(*const u16, usize, *mut PdhHQuery) -> u32 =
            unsafe { *lib.get(b"PdhOpenQueryW\0").ok()? };
        let pdh_add_counter_w: unsafe extern "system" fn(
            PdhHQuery,
            *const u16,
            usize,
            *mut PdhHCounter,
        ) -> u32 = unsafe { *lib.get(b"PdhAddCounterW\0").ok()? };
        let pdh_collect_query_data: unsafe extern "system" fn(PdhHQuery) -> u32 =
            unsafe { *lib.get(b"PdhCollectQueryData\0").ok()? };
        let pdh_get_formatted_counter_value: unsafe extern "system" fn(
            PdhHCounter,
            u32,
            *mut u32,
            *mut PdhFmtCounterValue,
        ) -> u32 = unsafe { *lib.get(b"PdhGetFormattedCounterValue\0").ok()? };
        let pdh_close_query: unsafe extern "system" fn(PdhHQuery) -> u32 =
            unsafe { *lib.get(b"PdhCloseQuery\0").ok()? };

        let mut h_query: PdhHQuery = std::ptr::null_mut();
        if unsafe { pdh_open_query_w(std::ptr::null(), 0, &mut h_query) } != 0 {
            return None;
        }

        // Try High Precision Temperature first, then standard Temperature
        let candidate_counters = [
            (
                r"\Thermal Zone Information(*)\High Precision Temperature",
                true,
            ),
            (r"\Thermal Zone Information(*)\Temperature", false),
        ];

        let mut h_counter: PdhHCounter = std::ptr::null_mut();
        let mut selected_is_hp = false;
        let mut added = false;

        for (path, is_hp) in candidate_counters {
            let wide: Vec<u16> = OsStr::new(path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            if unsafe { pdh_add_counter_w(h_query, wide.as_ptr(), 0, &mut h_counter) } == 0 {
                selected_is_hp = is_hp;
                added = true;
                debug!("WindowsThermalReader bound to: {}", path);
                break;
            }
        }

        if !added {
            unsafe {
                (pdh_close_query)(h_query);
            }
            return None;
        }

        // Prime the PDH query
        unsafe {
            (pdh_collect_query_data)(h_query);
        }

        info!("Windows ACPI Thermal Zone monitor initialized successfully via PDH");

        Some(Self {
            _lib: lib,
            h_query,
            h_counter,
            is_high_precision: selected_is_hp,
            pdh_collect_query_data,
            pdh_get_formatted_counter_value,
            pdh_close_query,
        })
    }

    pub fn read_temperature_c(&mut self) -> Option<u32> {
        unsafe {
            if (self.pdh_collect_query_data)(self.h_query) != 0 {
                return None;
            }

            let mut val = PdhFmtCounterValue {
                c_status: 0,
                double_value: 0.0,
            };
            let mut val_type = 0u32;
            const PDH_FMT_DOUBLE: u32 = 0x00000200;

            if (self.pdh_get_formatted_counter_value)(
                self.h_counter,
                PDH_FMT_DOUBLE,
                &mut val_type,
                &mut val,
            ) != 0
            {
                return None;
            }

            let kelvin = if self.is_high_precision || val.double_value > 1000.0 {
                val.double_value / 10.0 // tenths of Kelvin
            } else {
                val.double_value
            };

            let celsius = kelvin - 273.15;
            if (10.0..=125.0).contains(&celsius) {
                Some(celsius.round() as u32)
            } else {
                None
            }
        }
    }
}

impl Drop for WindowsThermalReader {
    fn drop(&mut self) {
        if !self.h_query.is_null() {
            unsafe {
                (self.pdh_close_query)(self.h_query);
            }
        }
    }
}
