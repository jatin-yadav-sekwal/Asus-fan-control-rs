//! Named-pipe IPC between the elevated GUI (client) and the SYSTEM helper
//! (server).
//!
//! `\\.\AsusSAIO` only answers to `NT AUTHORITY\SYSTEM`, so the process that
//! touches hardware cannot be the process that draws the window. The helper is
//! started by Task Scheduler as SYSTEM (headless, session 0 — irrelevant, it
//! never paints); the GUI stays a normal elevated process and reaches the
//! driver through this pipe.
//!
//! The pipe name is a random GUID handed to the helper on its command line by
//! the scheduled task, and the DACL grants access to only two principals:
//! `NT AUTHORITY\SYSTEM` and `BUILTIN\Administrators`. Nothing else can
//! connect — and since the GUI always elevates before it attaches, the client
//! is always one of those two.

use std::ffi::c_void;
use std::io;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::error::DriverError;

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

const OPCODE_HELLO: u8 = 0x01;
const OPCODE_GET_TEMP: u8 = 0x02;
const OPCODE_GET_RPMS: u8 = 0x03;
const OPCODE_SET_DUTY: u8 = 0x04;
const OPCODE_SET_ALL: u8 = 0x05;
const OPCODE_RESET_ONE: u8 = 0x06;
const OPCODE_RESET_ALL: u8 = 0x07;
const OPCODE_PING: u8 = 0x08;
const OPCODE_GET_RPM_ONE: u8 = 0x09;
const OPCODE_SHUTDOWN: u8 = 0x7f;

const STATUS_OK: u8 = 0;
const STATUS_ERR: u8 = 1;

/// Hard ceiling on a single frame. Requests are tiny; this only guards against
/// a corrupt or hostile writer from allocating unbounded memory.
const MAX_FRAME: u32 = 64 * 1024;

/// Largest count of fans we will ever report (matches `AsusDriver`'s clamp).
const MAX_FANS: u8 = 16;

/// How long the client waits for the helper's pipe to appear after the
/// scheduled task has been started.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Win32 FFI (hand-declared, matching the style used in `service.rs`)
// ---------------------------------------------------------------------------

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: isize = -1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_NOWAIT: u32 = 0x0000_0001;

const ERROR_BROKEN_PIPE: u32 = 109;
const ERROR_PIPE_NOT_CONNECTED: u32 = 233;
const ERROR_NO_DATA: u32 = 232;
const ERROR_SEM_TIMEOUT: u32 = 121;
const ERROR_PIPE_CONNECTED: u32 = 535;
/// `ConnectNamedPipe` in `PIPE_NOWAIT` mode: nobody has connected *yet*.
const ERROR_PIPE_LISTENING: u32 = 536;
const WAIT_TIMEOUT: u32 = 258;

const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const ACL_REVISION: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct SecurityDescriptor {
    revision: u8,
    sbz1: u8,
    control: u16,
    owner: *mut c_void,
    group: *mut c_void,
    sacl: *mut c_void,
    dacl: *mut c_void,
}

impl SecurityDescriptor {
    fn zeroed() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Acl {
    acl_revision: u8,
    sbz1: u8,
    acl_size: u16,
    ace_count: u16,
    sbz2: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SecurityAttributes {
    n_length: u32,
    lp_security_descriptor: *mut c_void,
    b_inherit_handle: i32,
}

#[link(name = "kernel32")]
extern "system" {
    #[link_name = "GetLastError"]
    fn last_error() -> u32;

    fn CreateNamedPipeW(
        lp_name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer_size: u32,
        in_buffer_size: u32,
        default_timeout: u32,
        security_attributes: *const SecurityAttributes,
    ) -> Handle;

    fn ConnectNamedPipe(handle: Handle, overlapped: *mut c_void) -> i32;
    fn WaitNamedPipeW(lp_name: *const u16, timeout: u32) -> i32;
    fn DisconnectNamedPipe(handle: Handle) -> i32;
    fn SetNamedPipeHandleState(
        handle: Handle,
        mode: *const u32,
        max_collection_count: *const u32,
        collect_data_timeout: *const u32,
    ) -> i32;

    /// Declared with a `*mut c_void` security attribute to match `acpi.rs`
    /// exactly — two declarations of the same symbol with different types are
    /// rejected by `clashing_extern_declarations`.
    #[link_name = "CreateFileW"]
    fn open_existing_file(
        lp_file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;

    fn ReadFile(
        handle: Handle,
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn WriteFile(
        handle: Handle,
        buffer: *const c_void,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn PeekNamedPipe(
        handle: Handle,
        buffer: *mut c_void,
        buffer_size: u32,
        bytes_read: *mut u32,
        bytes_available: *mut u32,
        bytes_left: *mut u32,
    ) -> i32;

    fn CloseHandle(object: Handle) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn InitializeSecurityDescriptor(
        security_descriptor: *mut SecurityDescriptor,
        revision: u32,
    ) -> i32;

    fn SetSecurityDescriptorDacl(
        security_descriptor: *mut SecurityDescriptor,
        dacl_present: i32,
        dacl: *mut Acl,
        dacl_defaulted: i32,
    ) -> i32;

    fn InitializeAcl(acl: *mut Acl, acl_length: u32, acl_revision: u32) -> i32;

    fn AddAccessAllowedAce(
        acl: *mut Acl,
        acl_revision: u32,
        access_mask: u32,
        sid: *const c_void,
    ) -> i32;

    fn CreateWellKnownSid(
        sid_type: i32,
        domain_sid: *const c_void,
        result_sid: *mut c_void,
        result_sid_length: *mut u32,
    ) -> i32;
}

const WIN_LOCAL_SYSTEM_SID: i32 = 22;
const WIN_BUILTIN_ADMINISTRATORS_SID: i32 = 26;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn err(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("{} (Win32 error {})", msg, unsafe { last_error() }))
}

fn is_fatal_pipe_error(code: u32) -> bool {
    matches!(
        code,
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA
    )
}

// ---------------------------------------------------------------------------
// DACL: SYSTEM + Administrators, nothing else
// ---------------------------------------------------------------------------

/// A `SECURITY_ATTRIBUTES` whose DACL admits exactly two principals:
/// `NT AUTHORITY\SYSTEM` and `BUILTIN\Administrators`.
///
/// The "other" principal cannot be the *creating* process's user: the helper
/// runs as SYSTEM, so resolving its own token would list SYSTEM twice and shut
/// the elevated GUI out. The client is always an elevated copy of the app
/// (both the GUI and the CLI force elevation first), so Administrators is both
/// the correct principal and the tighter one — a non-elevated process carries
/// Administrators as deny-only and is rejected.
///
/// The descriptor and ACL live inside this struct; `attributes` points at them
/// through heap allocations that never move, so `&self.attributes` stays valid
/// for as long as the struct does.
pub struct PipeSecurity {
    /// Owns the ACL bytes and the security descriptor; both pointers stored in
    /// `attributes` refer to heap data that never moves.
    _acl: Vec<u8>,
    descriptor: Box<SecurityDescriptor>,
    attributes: SecurityAttributes,
}

impl PipeSecurity {
    fn build() -> Self {
        // ACE layout: 8-byte ACE header + SID. Worst realistic SID is ~68
        // bytes; 4 KB covers both ACEs with slack to spare. The buffer must not
        // reallocate after `InitializeAcl`, or the stored ACL pointer dangles.
        let mut acl = vec![0u8; 4096];

        let mut system_sid = vec![0u8; 128];
        let mut system_sid_len = system_sid.len() as u32;
        let mut ready = unsafe {
            CreateWellKnownSid(
                WIN_LOCAL_SYSTEM_SID,
                std::ptr::null(),
                system_sid.as_mut_ptr() as *mut c_void,
                &mut system_sid_len,
            ) != 0
        };

        let mut admin_sid = vec![0u8; 128];
        let mut admin_sid_len = admin_sid.len() as u32;
        ready = ready
            && unsafe {
                CreateWellKnownSid(
                    WIN_BUILTIN_ADMINISTRATORS_SID,
                    std::ptr::null(),
                    admin_sid.as_mut_ptr() as *mut c_void,
                    &mut admin_sid_len,
                ) != 0
            };

        let acl_ptr = acl.as_mut_ptr() as *mut Acl;
        ready = ready && unsafe { InitializeAcl(acl_ptr, acl.len() as u32, ACL_REVISION) } != 0;

        // Grant the two principals only the access a duplex pipe needs.
        for sid in [system_sid.as_ptr(), admin_sid.as_ptr()] {
            if !ready {
                break;
            }
            ready = unsafe {
                AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_READ | GENERIC_WRITE, sid as *const c_void)
                    != 0
            };
        }

        let mut descriptor = Box::new(SecurityDescriptor::zeroed());
        if ready {
            ready = unsafe {
                InitializeSecurityDescriptor(
                    &mut *descriptor as *mut SecurityDescriptor,
                    SECURITY_DESCRIPTOR_REVISION,
                ) != 0
            };
        }
        if ready {
            ready = unsafe {
                SetSecurityDescriptorDacl(&mut *descriptor as *mut SecurityDescriptor, 1, acl_ptr, 0)
                    != 0
            };
        }
        if !ready {
            warn!(
                "Could not build a restricted pipe DACL (error {}); falling back to \
                 the default descriptor",
                unsafe { last_error() }
            );
        }

        let attributes = SecurityAttributes {
            n_length: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: if ready {
                &mut *descriptor as *mut SecurityDescriptor as *mut c_void
            } else {
                std::ptr::null_mut()
            },
            b_inherit_handle: 0,
        };

        Self {
            _acl: acl,
            descriptor,
            attributes,
        }
    }

    fn as_ptr(&self) -> *const SecurityAttributes {
        if self.descriptor.as_ref() as *const SecurityDescriptor as *const c_void
            == self.attributes.lp_security_descriptor as *const c_void
        {
            &self.attributes as *const SecurityAttributes
        } else {
            std::ptr::null()
        }
    }
}

// ---------------------------------------------------------------------------
// Frame codec
// ---------------------------------------------------------------------------

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_exact_timeout(handle: Handle, buf: &mut [u8], deadline: Option<Instant>) -> io::Result<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe read timed out"));
            }
        }

        let mut avail: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut avail,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let e = unsafe { last_error() };
            if is_fatal_pipe_error(e) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"));
            }
            // ERROR_NO_DATA means no message yet; keep polling.
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }

        if avail == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }

        let want = (buf.len() - filled).min(avail as usize);
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                handle,
                buf[filled..].as_mut_ptr() as *mut c_void,
                want as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let e = unsafe { last_error() };
            if is_fatal_pipe_error(e) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"));
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        filled += read as usize;
    }
    Ok(())
}

fn read_frame(handle: Handle, deadline: Option<Instant>) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    read_exact_timeout(handle, &mut len_buf, deadline)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("oversized pipe frame ({} bytes)", len),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    read_exact_timeout(handle, &mut payload, deadline)?;
    Ok(payload)
}

fn write_frame(handle: Handle, payload: &[u8]) -> io::Result<()> {
    let frame = encode_frame(payload);
    let mut written: u32 = 0;
    let ok = unsafe {
        WriteFile(
            handle,
            frame.as_ptr() as *const c_void,
            frame.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = unsafe { last_error() };
        if is_fatal_pipe_error(e) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"));
        }
        return Err(err("WriteFile on control pipe"));
    }
    if written as usize != frame.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short write on control pipe",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Client (GUI side)
// ---------------------------------------------------------------------------

/// A blocking request/response client over the control pipe.
///
/// Callers serialise through the `AsusDriver` mutex, so exactly one request is
/// in flight at a time — no request ids or demultiplexing needed.
pub struct PipeClient {
    handle: Handle,
    pipe_name: String,
}

// SAFETY: the handle is an ordinary kernel object handle, and every use is
// serialised by the `AsusDriver` mutex that owns the backend — the same
// reasoning that makes `AcpiDevice` and the WinIO backend `Send`.
unsafe impl Send for PipeClient {}

impl PipeClient {
    /// Opens the helper's pipe, waiting up to `CONNECT_TIMEOUT` for it to
    /// appear (the scheduled task has only just been started).
    pub fn connect(pipe_name: &str) -> Result<Self, DriverError> {
        let wide_name = wide(pipe_name);
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let handle = loop {
            let h = unsafe {
                open_existing_file(
                    wide_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null_mut(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if h as isize != INVALID_HANDLE_VALUE {
                break h;
            }

            let e = unsafe { last_error() };
            if e == ERROR_SEM_TIMEOUT || e == WAIT_TIMEOUT {
                // Named pipe is busy with another client; wait for a free slot.
                unsafe { WaitNamedPipeW(wide_name.as_ptr(), 200) };
            }
            if Instant::now() >= deadline {
                return Err(DriverError::PipeConnectFailed {
                    pipe: pipe_name.to_string(),
                    source: std::io::Error::from_raw_os_error(e as i32),
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        info!("Connected to SYSTEM helper over {}", pipe_name);
        Ok(Self {
            handle,
            pipe_name: pipe_name.to_string(),
        })
    }

    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    fn request(&self, payload: &[u8], deadline: Option<Instant>) -> Result<Vec<u8>, DriverError> {
        write_frame(self.handle, payload)
            .map_err(|source| DriverError::PipeIo { pipe: self.pipe_name.clone(), source })?;

        let body = read_frame(self.handle, deadline)
            .map_err(|source| DriverError::PipeIo { pipe: self.pipe_name.clone(), source })?;

        if body.is_empty() {
            return Err(DriverError::PipeProtocol("empty response".to_string()));
        }
        match body[0] {
            STATUS_OK => Ok(body[1..].to_vec()),
            STATUS_ERR => {
                let msg = if body.len() >= 3 {
                    let n = u16::from_le_bytes([body[1], body[2]]) as usize;
                    let end = (3 + n).min(body.len());
                    String::from_utf8_lossy(&body[3..end]).into_owned()
                } else {
                    "helper reported an error".to_string()
                };
                Err(DriverError::PipeProtocol(msg))
            }
            other => Err(DriverError::PipeProtocol(format!("unknown status byte {:#04x}", other))),
        }
    }

    pub fn hello(&self) -> Result<(u8, u32), DriverError> {
        let body = self.request(&[OPCODE_HELLO], None)?;
        if body.len() < 5 {
            return Err(DriverError::PipeProtocol("short Hello response".to_string()));
        }
        Ok((body[0].clamp(1, MAX_FANS), u32::from_le_bytes(body[1..5].try_into().unwrap())))
    }

    pub fn get_temp(&self) -> Result<u32, DriverError> {
        let body = self.request(&[OPCODE_GET_TEMP], None)?;
        if body.len() < 4 {
            return Err(DriverError::PipeProtocol("short temperature response".to_string()));
        }
        Ok(u32::from_le_bytes(body[0..4].try_into().unwrap()))
    }

    pub fn get_rpms(&self) -> Result<Vec<u32>, DriverError> {
        let body = self.request(&[OPCODE_GET_RPMS], None)?;
        if body.is_empty() {
            return Ok(Vec::new());
        }
        let count = body[0] as usize;
        let mut rpms = Vec::with_capacity(count);
        for i in 0..count {
            let off = 1 + i * 4;
            if off + 4 > body.len() {
                break;
            }
            rpms.push(u32::from_le_bytes(body[off..off + 4].try_into().unwrap()));
        }
        Ok(rpms)
    }

    pub fn get_rpm(&self, fan_idx: u8) -> Result<u32, DriverError> {
        let body = self.request(&[OPCODE_GET_RPM_ONE, fan_idx], None)?;
        if body.len() < 4 {
            return Err(DriverError::PipeProtocol("short RPM response".to_string()));
        }
        Ok(u32::from_le_bytes(body[0..4].try_into().unwrap()))
    }

    pub fn set_duty(&self, fan_idx: u8, percent: u8) -> Result<(), DriverError> {
        self.request(&[OPCODE_SET_DUTY, fan_idx, percent], None).map(|_| ())
    }

    pub fn set_all(&self, percent: u8) -> Result<(), DriverError> {
        self.request(&[OPCODE_SET_ALL, percent], None).map(|_| ())
    }

    pub fn reset_one(&self, fan_idx: u8) -> Result<(), DriverError> {
        self.request(&[OPCODE_RESET_ONE, fan_idx], None).map(|_| ())
    }

    pub fn reset_all(&self) -> Result<(), DriverError> {
        self.request(&[OPCODE_RESET_ALL], None).map(|_| ())
    }

    /// Handshake liveness probe used by the failsafe.
    pub fn ping(&self) -> Result<(), DriverError> {
        self.request(&[OPCODE_PING], None).map(|_| ())
    }

    /// Politely ask the helper to exit and tear the pipe down.
    pub fn shutdown(&self) -> Result<(), DriverError> {
        self.request(&[OPCODE_SHUTDOWN], None).map(|_| ())
    }
}

impl Drop for PipeClient {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.handle as isize != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.handle) };
            debug!("Control pipe closed");
        }
    }
}

// ---------------------------------------------------------------------------
// Server (helper side)
// ---------------------------------------------------------------------------

/// Creates the control pipe and waits for the GUI to connect, up to
/// `connect_timeout`.
///
/// The wait runs in `PIPE_NOWAIT` mode so a helper started by a task whose GUI
/// already died gives up instead of sitting in `ConnectNamedPipe` forever; the
/// pipe switches back to blocking mode for normal traffic once a client is in.
pub fn create_server(pipe_name: &str, connect_timeout: Duration) -> Result<Handle, DriverError> {
    let wide_name = wide(pipe_name);
    // The DACL must outlive CreateNamedPipeW, which copies it.
    let security = PipeSecurity::build();

    let handle = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            64 * 1024,
            64 * 1024,
            0,
            security.as_ptr(),
        )
    };
    if handle as isize == INVALID_HANDLE_VALUE {
        return Err(DriverError::PipeIo {
            pipe: pipe_name.to_string(),
            source: err("CreateNamedPipeW"),
        });
    }

    let mode_nowait = PIPE_NOWAIT;
    if unsafe {
        SetNamedPipeHandleState(
            handle,
            &mode_nowait as *const u32,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        warn!(
            "SetNamedPipeHandleState(PIPE_NOWAIT) failed (error {}); the helper may \
             outlive its GUI",
            unsafe { last_error() }
        );
    }

    let deadline = Instant::now() + connect_timeout;
    loop {
        if unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 {
            break;
        }
        let e = unsafe { last_error() };
        // ERROR_PIPE_CONNECTED: the client arrived between create and connect.
        if e == ERROR_PIPE_CONNECTED {
            break;
        }
        // The pipe is in PIPE_NOWAIT mode, so "no client yet" arrives as
        // ERROR_PIPE_LISTENING (the normal case) or ERROR_PIPE_NOT_CONNECTED /
        // ERROR_NO_DATA (if the handle is observed mid-transition). Anything
        // else is a real failure.
        if e != ERROR_PIPE_LISTENING && e != ERROR_PIPE_NOT_CONNECTED && e != ERROR_NO_DATA {
            unsafe { CloseHandle(handle) };
            return Err(DriverError::PipeIo {
                pipe: pipe_name.to_string(),
                source: err("ConnectNamedPipe"),
            });
        }
        if Instant::now() >= deadline {
            unsafe { CloseHandle(handle) };
            return Err(DriverError::PipeProtocol(format!(
                "no GUI connected within {}s; exiting",
                connect_timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let mode_wait = PIPE_WAIT;
    unsafe {
        SetNamedPipeHandleState(
            handle,
            &mode_wait as *const u32,
            std::ptr::null(),
            std::ptr::null(),
        )
    };

    info!("SYSTEM helper connected to GUI on {}", pipe_name);
    Ok(handle)
}

pub struct PipeServer {
    handle: Handle,
}

impl PipeServer {
    pub fn new(handle: Handle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> Handle {
        self.handle
    }

    pub fn read_request(&self) -> io::Result<Vec<u8>> {
        read_frame(self.handle, None)
    }

    pub fn write_response(&self, payload: &[u8]) -> io::Result<()> {
        write_frame(self.handle, payload)
    }

    pub fn write_ok(&self, body: &[u8]) -> io::Result<()> {
        let mut payload = Vec::with_capacity(body.len() + 1);
        payload.push(STATUS_OK);
        payload.extend_from_slice(body);
        self.write_response(&payload)
    }

    pub fn write_err(&self, message: &str) -> io::Result<()> {
        let bytes = message.as_bytes();
        let n = bytes.len().min(u16::MAX as usize) as u16;
        let mut payload = Vec::with_capacity(4 + n as usize);
        payload.push(STATUS_ERR);
        payload.extend_from_slice(&n.to_le_bytes());
        payload.extend_from_slice(&bytes[..n as usize]);
        self.write_response(&payload)
    }

    pub fn is_connected(&self) -> bool {
        // A zero-byte peek succeeds only while the client is still attached.
        let mut avail: u32 = 0;
        unsafe {
            PeekNamedPipe(
                self.handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut avail,
                std::ptr::null_mut(),
            ) != 0
                || last_error() == ERROR_NO_DATA
        }
    }

    /// Executes one decoded request against the local (SYSTEM) driver and
    /// writes the response.
    ///
    /// Returns `Ok(false)` when the client asked the helper to shut down, and
    /// `Err` only when the response itself could not be written — i.e. the pipe
    /// is dead and the caller should stop serving.
    pub fn handle_request(
        &self,
        req: &[u8],
        driver: &crate::driver::SafeAsusDriver,
    ) -> io::Result<bool> {
        let ok = |body: Vec<u8>| self.write_ok(&body).map(|()| true);
        let fail = |msg: String| self.write_err(&msg).map(|()| true);

        match req.first().copied().unwrap_or(0) {
            OPCODE_HELLO => {
                let (count, temp) = {
                    let mut d = driver.lock();
                    let t = d.get_cpu_temperature().unwrap_or(0);
                    (d.fan_count(), t)
                };
                let mut body = Vec::with_capacity(5);
                body.push(count);
                body.extend_from_slice(&temp.to_le_bytes());
                ok(body)
            }

            OPCODE_GET_TEMP => {
                let temp = driver.lock().get_cpu_temperature().unwrap_or(0);
                ok(temp.to_le_bytes().to_vec())
            }

            OPCODE_GET_RPMS => {
                let rpms = driver.lock().get_all_fan_rpms().unwrap_or_default();
                let mut body = Vec::with_capacity(1 + rpms.len() * 4);
                body.push(rpms.len() as u8);
                for rpm in &rpms {
                    body.extend_from_slice(&rpm.to_le_bytes());
                }
                ok(body)
            }

            OPCODE_GET_RPM_ONE => {
                let idx = req.get(1).copied().unwrap_or(0);
                match driver.lock().get_fan_rpm(idx) {
                    Ok(rpm) => ok(rpm.to_le_bytes().to_vec()),
                    Err(e) => fail(e.to_string()),
                }
            }

            OPCODE_SET_DUTY if req.len() >= 3 => {
                let (idx, percent) = (req[1], req[2]);
                match driver.lock().set_fan_percent(idx, percent) {
                    Ok(()) => ok(Vec::new()),
                    Err(e) => fail(e.to_string()),
                }
            }

            OPCODE_SET_ALL if req.len() >= 2 => {
                let percent = req[1];
                match driver.lock().set_all_fans_percent(percent) {
                    Ok(()) => ok(Vec::new()),
                    Err(e) => fail(e.to_string()),
                }
            }

            OPCODE_RESET_ONE if req.len() >= 2 => {
                let idx = req[1];
                match driver.lock().reset_fan_to_bios(idx) {
                    Ok(()) => ok(Vec::new()),
                    Err(e) => fail(e.to_string()),
                }
            }

            OPCODE_RESET_ALL => match driver.lock().reset_all_to_bios() {
                Ok(()) => ok(Vec::new()),
                Err(e) => fail(e.to_string()),
            },

            OPCODE_PING => ok(Vec::new()),

            OPCODE_SHUTDOWN => {
                self.write_ok(&[])?;
                return Ok(false);
            }

            OPCODE_SET_DUTY | OPCODE_SET_ALL | OPCODE_RESET_ONE => {
                self.write_err("malformed request")?;
                Ok(true)
            }

            other => self
                .write_err(&format!("unknown opcode {:#04x}", other))
                .map(|()| true),
        }
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.handle as isize != INVALID_HANDLE_VALUE {
            unsafe {
                DisconnectNamedPipe(self.handle);
                CloseHandle(self.handle);
            }
            debug!("Control pipe server shut down");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let payload = [OPCODE_SET_ALL, 42];
        let frame = encode_frame(&payload);
        assert_eq!(&frame[0..4], &(2u32).to_le_bytes());
        assert_eq!(&frame[4..], &payload);
    }

    #[test]
    fn rejects_oversized_declared_length() {
        // A 1 GB declared length must be refused before any allocation.
        let bogus = (u32::MAX).to_le_bytes();
        let len = u32::from_le_bytes(bogus);
        assert!(len > MAX_FRAME);
    }

    #[test]
    fn error_response_encoding() {
        let mut payload = vec![STATUS_ERR];
        payload.extend_from_slice(&3u16.to_le_bytes());
        payload.extend_from_slice(b"bad");
        assert_eq!(payload[0], STATUS_ERR);
        assert_eq!(u16::from_le_bytes([payload[1], payload[2]]), 3);
        assert_eq!(&payload[3..], b"bad");
    }

    fn unique_pipe() -> String {
        format!(
            "\\\\.\\pipe\\AsusFanControl-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    }

    #[test]
    fn server_waits_for_a_client_that_never_comes() {
        // The pipe is put into PIPE_NOWAIT mode before any client arrives, so
        // ConnectNamedPipe reports ERROR_PIPE_LISTENING on every attempt. That
        // is the normal "nobody has connected yet" signal — treating it as a
        // failure made the SYSTEM helper exit before it ever served a request.
        match create_server(&unique_pipe(), Duration::from_millis(400)) {
            Ok(handle) => {
                unsafe { CloseHandle(handle) };
            }
            Err(DriverError::PipeProtocol(msg)) => {
                assert!(
                    msg.contains("no GUI connected"),
                    "unexpected protocol error: {msg}"
                )
            }
            Err(other) => panic!("create_server failed instead of timing out: {other}"),
        }
    }
}
