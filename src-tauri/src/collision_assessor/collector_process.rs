//! Killable native process boundary for the privileged metadata census.

use super::api::{CensusCollectionFailure, MetadataCensusCollector};
use super::capability::DiscoveryCancellation;
use super::discovery::{
    decode_frame, encode_frame, execute_request, request_from_snapshot, validate_response,
    CensusHelperRequest, CensusHelperResponse, DiscoveryError, MAX_FRAME_BYTES,
};
use super::identity::{physical_path_identity, PhysicalPathIdentity, PhysicalPathKind};
use super::registry::{CensusInputSnapshot, DiscoveryCensus};
use crate::supervisor::unix_ms;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const INTERNAL_HELPER_ARG: &str = "--perfect-planner-collision-census-helper-v1";
const MAX_STDERR_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(4);
const PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_NONCES_PER_PROCESS: usize = 65_536;
const NONCE_GENERATION_ATTEMPTS: usize = 4;
static USED_NONCES: OnceLock<Mutex<BTreeSet<[u8; 32]>>> = OnceLock::new();

pub(crate) struct NativeProcessCollector {
    executable: PathBuf,
    executable_identity: PhysicalPathIdentity,
}

impl NativeProcessCollector {
    pub(crate) fn for_current_executable() -> Result<Self, CensusCollectionFailure> {
        #[cfg(not(windows))]
        {
            return Err(CensusCollectionFailure::Unavailable);
        }
        #[cfg(windows)]
        {
            let executable =
                std::env::current_exe().map_err(|_| CensusCollectionFailure::Unavailable)?;
            super::discovery::validate_local_path(&executable, PhysicalPathKind::RegularFile)
                .map_err(map_discovery_error)?;
            let executable_identity =
                physical_path_identity(&executable, PhysicalPathKind::RegularFile)
                    .map_err(|_| CensusCollectionFailure::IdentityChanged)?;
            Ok(Self {
                executable,
                executable_identity,
            })
        }
    }
}

impl MetadataCensusCollector for NativeProcessCollector {
    fn collect(
        &self,
        input: CensusInputSnapshot,
        capability_deadline_ms: u64,
        cancellation: DiscoveryCancellation,
    ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
        #[cfg(not(windows))]
        {
            let _ = (input, capability_deadline_ms, cancellation);
            return Err(CensusCollectionFailure::Unavailable);
        }
        #[cfg(windows)]
        {
            if cancellation.is_cancelled() || unix_ms() >= capability_deadline_ms {
                return Err(CensusCollectionFailure::Timeout);
            }
            let mut nonce = generate_nonce().map_err(map_discovery_error)?;
            let request = request_from_snapshot(&input, nonce.clone(), capability_deadline_ms)
                .map_err(map_discovery_error)?;
            let request_frame = encode_frame(&request).map_err(map_discovery_error)?;
            let spec = ProcessSpec::production(&self.executable);
            let process = exchange(
                &spec,
                &self.executable_identity,
                request_frame,
                capability_deadline_ms,
                cancellation,
            );
            nonce.replace_range(.., &"0".repeat(64));
            let output = process.output.map_err(map_process_error)?;
            let response: CensusHelperResponse =
                decode_frame(&output).map_err(map_discovery_error)?;
            validate_response(&request, response).map_err(map_discovery_error)
        }
    }
}

/// Called by `main` before Tauri initializes. A helper-looking invocation with extra arguments is
/// rejected instead of falling through into the desktop application.
pub fn dispatch_internal_helper() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    classify_internal_args(&args).map(|valid| if valid { run_helper_stdio() } else { 64 })
}

fn classify_internal_args(args: &[OsString]) -> Option<bool> {
    let helper_named = args
        .iter()
        .any(|arg| arg == OsStr::new(INTERNAL_HELPER_ARG));
    if !helper_named {
        return None;
    }
    Some(args.len() == 1 && args[0] == OsStr::new(INTERNAL_HELPER_ARG))
}

fn run_helper_stdio() -> i32 {
    let mut input = Vec::new();
    if std::io::stdin()
        .take((MAX_FRAME_BYTES + 13) as u64)
        .read_to_end(&mut input)
        .is_err()
        || input.len() > MAX_FRAME_BYTES + 12
    {
        return 65;
    }
    let request: CensusHelperRequest = match decode_frame(&input) {
        Ok(request) => request,
        Err(_) => return 65,
    };
    let response = match execute_request(request) {
        Ok(response) => response,
        Err(_) => return 66,
    };
    let output = match encode_frame(&response) {
        Ok(output) => output,
        Err(_) => return 67,
    };
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&output)
        .and_then(|_| stdout.flush())
        .is_err()
    {
        return 68;
    }
    0
}

struct ProcessSpec {
    executable: PathBuf,
    args: Vec<OsString>,
}

impl ProcessSpec {
    fn production(executable: &Path) -> Self {
        Self {
            executable: executable.to_path_buf(),
            args: vec![OsString::from(INTERNAL_HELPER_ARG)],
        }
    }

    #[cfg(test)]
    fn test_helper(name: &str) -> Self {
        Self {
            executable: std::env::current_exe().expect("test executable"),
            args: vec![
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from(name),
                OsString::from("--nocapture"),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessError {
    Spawn,
    ImageMismatch,
    Timeout,
    Cancelled,
    OutputLimit,
    ExitFailure,
    MalformedOutput,
    #[cfg(test)]
    CleanupUnproven,
}

struct ProcessExchange {
    output: Result<Vec<u8>, ProcessError>,
    #[cfg_attr(not(test), allow(dead_code))]
    pid: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    terminated: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    active_processes_after_wait: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CleanupReport {
    termination_required: bool,
    terminate_succeeded: bool,
    wait_confirmed_exit: bool,
    active_processes: Option<u32>,
    close_succeeded: bool,
}

fn enforce_cleanup_with<F>(report: CleanupReport, fail_stop: F) -> Result<u32, ProcessError>
where
    F: FnOnce() -> ProcessError,
{
    let termination_proven = !report.termination_required || report.terminate_succeeded;
    if termination_proven
        && report.wait_confirmed_exit
        && report.active_processes == Some(0)
        && report.close_succeeded
    {
        Ok(0)
    } else {
        Err(fail_stop())
    }
}

#[cfg(test)]
fn cleanup_fail_stop() -> ProcessError {
    ProcessError::CleanupUnproven
}

#[cfg(not(test))]
fn cleanup_fail_stop() -> ProcessError {
    // A failed cleanup proof means a privileged reader may still be alive. Returning control to
    // Tauri would restore renderer authority while that reader survives, so the only safe
    // production response is process teardown. Windows then closes the KILL_ON_JOB_CLOSE handle.
    std::process::abort()
}

fn cleanup_fail_stop_immediately() {
    #[cfg(not(test))]
    std::process::abort();
}

fn exchange(
    spec: &ProcessSpec,
    expected_image: &PhysicalPathIdentity,
    request_frame: Vec<u8>,
    capability_deadline_ms: u64,
    cancellation: DiscoveryCancellation,
) -> ProcessExchange {
    exchange_with_wall_clock(
        spec,
        expected_image,
        request_frame,
        capability_deadline_ms,
        cancellation,
        unix_ms,
    )
}

fn exchange_with_wall_clock<F>(
    spec: &ProcessSpec,
    expected_image: &PhysicalPathIdentity,
    request_frame: Vec<u8>,
    capability_deadline_ms: u64,
    cancellation: DiscoveryCancellation,
    wall_now_ms: F,
) -> ProcessExchange
where
    F: Fn() -> u64,
{
    let mut empty = ProcessExchange {
        output: Err(ProcessError::Spawn),
        pid: 0,
        terminated: false,
        active_processes_after_wait: 0,
    };
    let remaining_ms = capability_deadline_ms.saturating_sub(wall_now_ms());
    if remaining_ms == 0 {
        empty.output = Err(ProcessError::Timeout);
        return empty;
    }
    let monotonic_deadline = Instant::now() + Duration::from_millis(remaining_ms);
    let image_guard = match open_image_guard(&spec.executable) {
        Ok(file) => file,
        Err(_) => return empty,
    };
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .env_clear()
        .current_dir(spec.executable.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return empty,
    };
    empty.pid = child.id();
    let mut job = match ChildJob::new_and_assign(&child) {
        Ok(job) => job,
        Err(_) => {
            if let Err(error) = cleanup_uncontained_child(&mut child) {
                empty.output = Err(error);
            }
            return empty;
        }
    };
    if !child_image_matches(&child, expected_image) {
        empty.output = Err(ProcessError::ImageMismatch);
        empty.terminated = true;
        match cleanup_job_child(&mut child, &mut job, true) {
            Ok(active) => empty.active_processes_after_wait = active,
            Err(error) => empty.output = Err(error),
        }
        return empty;
    }

    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            empty.terminated = true;
            match cleanup_job_child(&mut child, &mut job, true) {
                Ok(active) => empty.active_processes_after_wait = active,
                Err(error) => empty.output = Err(error),
            }
            return empty;
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_overflow = Arc::new(AtomicBool::new(false));
    let stderr_overflow = Arc::new(AtomicBool::new(false));
    let output_flag = Arc::clone(&stdout_overflow);
    let error_flag = Arc::clone(&stderr_overflow);
    let writer = thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(&request_frame).and_then(|_| stdin.flush())
    });
    let stdout_reader =
        thread::spawn(move || read_capped(stdout, MAX_FRAME_BYTES + 12, output_flag));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_STDERR_BYTES, error_flag));

    let mut status: Option<ExitStatus> = None;
    let mut failure = None;
    loop {
        if cancellation.is_cancelled() {
            failure = Some(ProcessError::Cancelled);
        } else if Instant::now() >= monotonic_deadline || wall_now_ms() >= capability_deadline_ms {
            failure = Some(ProcessError::Timeout);
        } else if stdout_overflow.load(Ordering::Acquire) || stderr_overflow.load(Ordering::Acquire)
        {
            failure = Some(ProcessError::OutputLimit);
        }
        if failure.is_some() {
            empty.terminated = true;
            break;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(_) => {
                failure = Some(ProcessError::ExitFailure);
                empty.terminated = true;
                break;
            }
        }
    }
    match cleanup_job_child(&mut child, &mut job, failure.is_some()) {
        Ok(active) => empty.active_processes_after_wait = active,
        Err(error) => {
            empty.output = Err(error);
            return empty;
        }
    }
    drop(image_guard);
    let writer_ok = writer.join().is_ok_and(|result| result.is_ok());
    let stdout = stdout_reader.join().ok().and_then(Result::ok);
    let _stderr = stderr_reader.join().ok().and_then(Result::ok);

    if let Some(failure) = failure {
        empty.output = Err(failure);
        return empty;
    }
    if !status.is_some_and(|status| status.success()) || !writer_ok {
        empty.output = Err(ProcessError::ExitFailure);
    } else if stdout_overflow.load(Ordering::Acquire) || stderr_overflow.load(Ordering::Acquire) {
        empty.output = Err(ProcessError::OutputLimit);
    } else if let Some(stdout) = stdout {
        empty.output = Ok(stdout);
    } else {
        empty.output = Err(ProcessError::MalformedOutput);
    }
    empty
}

fn cleanup_uncontained_child(child: &mut Child) -> Result<(), ProcessError> {
    let already_exited = child.try_wait().ok().flatten().is_some();
    let terminate_succeeded = already_exited || child.kill().is_ok();
    let wait_confirmed_exit = wait_child_bounded(child, PROCESS_CLEANUP_TIMEOUT);
    if terminate_succeeded && wait_confirmed_exit {
        Ok(())
    } else {
        Err(cleanup_fail_stop())
    }
}

fn cleanup_job_child(
    child: &mut Child,
    job: &mut ChildJob,
    termination_required: bool,
) -> Result<u32, ProcessError> {
    let terminate_succeeded = !termination_required || job.terminate().is_ok();
    let wait_confirmed_exit = wait_child_bounded(child, PROCESS_CLEANUP_TIMEOUT);
    let active_processes = job.active_processes().ok();
    let close_succeeded = job.close().is_ok();
    enforce_cleanup_with(
        CleanupReport {
            termination_required,
            terminate_succeeded,
            wait_confirmed_exit,
            active_processes,
            close_succeeded,
        },
        cleanup_fail_stop,
    )
}

#[cfg(windows)]
fn wait_child_bounded(child: &mut Child, timeout: Duration) -> bool {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    }
    const WAIT_OBJECT_0: u32 = 0;
    let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    let wait =
        unsafe { WaitForSingleObject(child.as_raw_handle() as *mut std::ffi::c_void, timeout_ms) };
    wait == WAIT_OBJECT_0 && child.try_wait().ok().flatten().is_some()
}

#[cfg(not(windows))]
fn wait_child_bounded(_child: &mut Child, _timeout: Duration) -> bool {
    false
}

fn read_capped<R: Read>(
    mut reader: R,
    maximum: usize,
    overflow: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(maximum.min(64 * 1024));
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if output.len().saturating_add(read) > maximum {
            overflow.store(true, Ordering::Release);
            return Ok(output);
        }
        output.extend_from_slice(&chunk[..read]);
    }
    Ok(output)
}

fn open_image_guard(path: &Path) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const GENERIC_READ: u32 = 0x8000_0000;
        const FILE_SHARE_READ: u32 = 0x1;
        return OpenOptions::new()
            .access_mode(GENERIC_READ)
            .share_mode(FILE_SHARE_READ)
            .open(path);
    }
    #[cfg(not(windows))]
    OpenOptions::new().read(true).open(path)
}

#[cfg(windows)]
fn child_image_matches(child: &Child, expected: &PhysicalPathIdentity) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn QueryFullProcessImageNameW(
            process: *mut std::ffi::c_void,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut size = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            child.as_raw_handle() as *mut _,
            0,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    if ok == 0 {
        return false;
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..size as usize]));
    physical_path_identity(&path, PhysicalPathKind::RegularFile)
        .is_ok_and(|actual| actual == *expected)
}

#[cfg(not(windows))]
fn child_image_matches(_child: &Child, _expected: &PhysicalPathIdentity) -> bool {
    false
}

#[cfg(windows)]
struct ChildJob {
    handle: Option<*mut std::ffi::c_void>,
}

#[cfg(windows)]
impl ChildJob {
    fn new_and_assign(child: &Child) -> Result<Self, ()> {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct BasicLimit {
            per_process_user_time_limit: i64,
            per_job_user_time_limit: i64,
            limit_flags: u32,
            minimum_working_set_size: usize,
            maximum_working_set_size: usize,
            active_process_limit: u32,
            affinity: usize,
            priority_class: u32,
            scheduling_class: u32,
        }
        #[repr(C)]
        struct IoCounters {
            read_operation_count: u64,
            write_operation_count: u64,
            other_operation_count: u64,
            read_transfer_count: u64,
            write_transfer_count: u64,
            other_transfer_count: u64,
        }
        #[repr(C)]
        struct ExtendedLimit {
            basic: BasicLimit,
            io: IoCounters,
            process_memory_limit: usize,
            job_memory_limit: usize,
            peak_process_memory_used: usize,
            peak_job_memory_used: usize,
        }
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn CreateJobObjectW(
                attributes: *mut std::ffi::c_void,
                name: *const u16,
            ) -> *mut std::ffi::c_void;
            fn SetInformationJobObject(
                job: *mut std::ffi::c_void,
                class: u32,
                data: *const std::ffi::c_void,
                length: u32,
            ) -> i32;
            fn AssignProcessToJobObject(
                job: *mut std::ffi::c_void,
                process: *mut std::ffi::c_void,
            ) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        const KILL_ON_CLOSE: u32 = 0x0000_2000;
        const ACTIVE_PROCESS: u32 = 0x0000_0008;
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job.is_null() {
            return Err(());
        }
        let info = ExtendedLimit {
            basic: BasicLimit {
                per_process_user_time_limit: 0,
                per_job_user_time_limit: 0,
                limit_flags: KILL_ON_CLOSE | ACTIVE_PROCESS,
                minimum_working_set_size: 0,
                maximum_working_set_size: 0,
                active_process_limit: 1,
                affinity: 0,
                priority_class: 0,
                scheduling_class: 0,
            },
            io: IoCounters {
                read_operation_count: 0,
                write_operation_count: 0,
                other_operation_count: 0,
                read_transfer_count: 0,
                write_transfer_count: 0,
                other_transfer_count: 0,
            },
            process_memory_limit: 0,
            job_memory_limit: 0,
            peak_process_memory_used: 0,
            peak_job_memory_used: 0,
        };
        let configured = unsafe {
            SetInformationJobObject(
                job,
                9,
                &info as *const _ as *const _,
                std::mem::size_of::<ExtendedLimit>() as u32,
            )
        };
        let assigned = if configured != 0 {
            unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as *mut _) }
        } else {
            0
        };
        if configured == 0 || assigned == 0 {
            if unsafe { CloseHandle(job) } == 0 {
                cleanup_fail_stop_immediately();
            }
            return Err(());
        }
        Ok(Self { handle: Some(job) })
    }

    fn handle(&self) -> Result<*mut std::ffi::c_void, ()> {
        self.handle.ok_or(())
    }

    fn terminate(&self) -> Result<(), ()> {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn TerminateJobObject(job: *mut std::ffi::c_void, exit_code: u32) -> i32;
        }
        if unsafe { TerminateJobObject(self.handle()?, 70) } != 0 {
            Ok(())
        } else {
            Err(())
        }
    }

    fn active_processes(&self) -> Result<u32, ()> {
        #[repr(C)]
        #[derive(Default)]
        struct Accounting {
            total_user_time: i64,
            total_kernel_time: i64,
            this_period_total_user_time: i64,
            this_period_total_kernel_time: i64,
            total_page_fault_count: u32,
            total_processes: u32,
            active_processes: u32,
            total_terminated_processes: u32,
        }
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn QueryInformationJobObject(
                job: *mut std::ffi::c_void,
                class: u32,
                data: *mut std::ffi::c_void,
                length: u32,
                returned: *mut u32,
            ) -> i32;
        }
        let mut accounting = Accounting::default();
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle()?,
                1,
                &mut accounting as *mut _ as *mut _,
                std::mem::size_of::<Accounting>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(())
        } else {
            Ok(accounting.active_processes)
        }
    }

    fn close(&mut self) -> Result<(), ()> {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let handle = self.handle.ok_or(())?;
        if unsafe { CloseHandle(handle) } != 0 {
            self.handle = None;
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(windows)]
impl Drop for ChildJob {
    fn drop(&mut self) {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        if let Some(handle) = self.handle.take() {
            if unsafe { CloseHandle(handle) } == 0 {
                cleanup_fail_stop_immediately();
            }
        }
    }
}

#[cfg(not(windows))]
struct ChildJob;

#[cfg(not(windows))]
impl ChildJob {
    fn new_and_assign(_child: &Child) -> Result<Self, ()> {
        Err(())
    }
    fn terminate(&self) -> Result<(), ()> {
        Err(())
    }
    fn active_processes(&self) -> Result<u32, ()> {
        Err(())
    }
    fn close(&mut self) -> Result<(), ()> {
        Err(())
    }
}

fn generate_nonce() -> Result<String, DiscoveryError> {
    generate_nonce_with(fill_os_random)
}

fn generate_nonce_with<F>(mut fill: F) -> Result<String, DiscoveryError>
where
    F: FnMut(&mut [u8]) -> Result<(), DiscoveryError>,
{
    let used = USED_NONCES.get_or_init(|| Mutex::new(BTreeSet::new()));
    for _ in 0..NONCE_GENERATION_ATTEMPTS {
        let mut bytes = [0_u8; 32];
        fill(&mut bytes)?;
        let inserted = {
            let mut guard = used.lock().map_err(|_| DiscoveryError::Unavailable)?;
            if guard.len() >= MAX_NONCES_PER_PROCESS {
                return Err(DiscoveryError::Unavailable);
            }
            guard.insert(bytes)
        };
        if !inserted {
            bytes.fill(0);
            continue;
        }
        let output = nonce_hex(bytes);
        bytes.fill(0);
        return Ok(output);
    }
    Err(DiscoveryError::Unavailable)
}

fn nonce_hex(bytes: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), DiscoveryError> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const SYSTEM_PREFERRED: u32 = 0x0000_0002;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            SYSTEM_PREFERRED,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(DiscoveryError::Unavailable)
    }
}

#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), DiscoveryError> {
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .map_err(|_| DiscoveryError::Unavailable)
}

#[cfg(not(any(windows, unix)))]
fn fill_os_random(_bytes: &mut [u8]) -> Result<(), DiscoveryError> {
    Err(DiscoveryError::Unavailable)
}

fn map_discovery_error(error: DiscoveryError) -> CensusCollectionFailure {
    match error {
        DiscoveryError::Unavailable => CensusCollectionFailure::Unavailable,
        DiscoveryError::Timeout => CensusCollectionFailure::Timeout,
        DiscoveryError::Malformed => CensusCollectionFailure::Malformed,
        DiscoveryError::LimitExceeded => CensusCollectionFailure::LimitExceeded,
        DiscoveryError::IdentityChanged => CensusCollectionFailure::IdentityChanged,
        DiscoveryError::Failed => CensusCollectionFailure::Failed,
    }
}

fn map_process_error(error: ProcessError) -> CensusCollectionFailure {
    match error {
        ProcessError::Timeout | ProcessError::Cancelled => CensusCollectionFailure::Timeout,
        ProcessError::OutputLimit => CensusCollectionFailure::LimitExceeded,
        ProcessError::MalformedOutput => CensusCollectionFailure::Malformed,
        ProcessError::ImageMismatch => CensusCollectionFailure::IdentityChanged,
        ProcessError::Spawn | ProcessError::ExitFailure => CensusCollectionFailure::Failed,
        #[cfg(test)]
        ProcessError::CleanupUnproven => CensusCollectionFailure::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision_assessor::capability::{CapabilityStore, DiscoveryScope};
    use std::sync::atomic::AtomicU64;

    fn cancellation_fixture() -> (Arc<CapabilityStore>, String, DiscoveryCancellation) {
        let store = Arc::new(CapabilityStore::default());
        let issued = store
            .issue(
                DiscoveryScope {
                    run_id: "b04-process-test".into(),
                    registry_generation: 1,
                    repository_census_hash: "a".repeat(64),
                },
                1_000,
                60_000,
            )
            .unwrap();
        let cancellation = store
            .begin_discovery_for_run(&issued.token, "b04-process-test", 1_001)
            .unwrap()
            .cancellation();
        (store, issued.token, cancellation)
    }

    fn cancellation() -> DiscoveryCancellation {
        cancellation_fixture().2
    }

    fn proven_cleanup() -> CleanupReport {
        CleanupReport {
            termination_required: true,
            terminate_succeeded: true,
            wait_confirmed_exit: true,
            active_processes: Some(0),
            close_succeeded: true,
        }
    }

    fn assert_cleanup_reaches_fail_stop(report: CleanupReport) {
        let reached = AtomicBool::new(false);
        let result = enforce_cleanup_with(report, || {
            reached.store(true, Ordering::Release);
            ProcessError::CleanupUnproven
        });
        assert_eq!(result, Err(ProcessError::CleanupUnproven));
        assert!(reached.load(Ordering::Acquire));
    }

    #[test]
    fn terminate_failure_reaches_fail_stop_and_never_acceptance() {
        assert_cleanup_reaches_fail_stop(CleanupReport {
            terminate_succeeded: false,
            ..proven_cleanup()
        });
    }

    #[test]
    fn bounded_wait_timeout_reaches_fail_stop_and_never_acceptance() {
        assert_cleanup_reaches_fail_stop(CleanupReport {
            wait_confirmed_exit: false,
            ..proven_cleanup()
        });
    }

    #[test]
    fn job_query_failure_reaches_fail_stop_and_never_acceptance() {
        assert_cleanup_reaches_fail_stop(CleanupReport {
            active_processes: None,
            ..proven_cleanup()
        });
    }

    #[test]
    fn nonzero_or_unknown_active_job_count_never_accepts() {
        assert_cleanup_reaches_fail_stop(CleanupReport {
            active_processes: Some(1),
            ..proven_cleanup()
        });
        assert_cleanup_reaches_fail_stop(CleanupReport {
            active_processes: Some(u32::MAX),
            ..proven_cleanup()
        });
    }

    #[test]
    fn job_close_failure_reaches_fail_stop_and_never_acceptance() {
        assert_cleanup_reaches_fail_stop(CleanupReport {
            close_succeeded: false,
            ..proven_cleanup()
        });
    }

    #[test]
    fn exact_cleanup_proof_is_the_only_acceptance_shape() {
        assert_eq!(
            enforce_cleanup_with(proven_cleanup(), cleanup_fail_stop),
            Ok(0)
        );
        assert_eq!(
            enforce_cleanup_with(
                CleanupReport {
                    termination_required: false,
                    terminate_succeeded: false,
                    ..proven_cleanup()
                },
                cleanup_fail_stop,
            ),
            Ok(0)
        );
    }

    #[test]
    fn nonce_is_256_bit_hex_and_nonrepeating() {
        let first = generate_nonce().unwrap();
        let second = generate_nonce().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn generated_nonce_collision_is_rejected_before_process_authority() {
        let bytes = [0xd3_u8; 32];
        let first = generate_nonce_with(|output| {
            output.copy_from_slice(&bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(first, nonce_hex(bytes));
        let repeated = generate_nonce_with(|output| {
            output.copy_from_slice(&bytes);
            Ok(())
        });
        assert_eq!(repeated, Err(DiscoveryError::Unavailable));
    }

    #[test]
    fn helper_dispatch_accepts_only_the_one_fixed_internal_argument() {
        assert_eq!(classify_internal_args(&[]), None);
        assert_eq!(
            classify_internal_args(&[OsString::from(INTERNAL_HELPER_ARG)]),
            Some(true)
        );
        assert_eq!(
            classify_internal_args(&[OsString::from(INTERNAL_HELPER_ARG), OsString::from("extra")]),
            Some(false)
        );
        assert_eq!(
            classify_internal_args(&[OsString::from("extra"), OsString::from(INTERNAL_HELPER_ARG)]),
            Some(false)
        );
        let production = ProcessSpec::production(Path::new(r"C:\fixed\planner.exe"));
        assert_eq!(production.args, vec![OsString::from(INTERNAL_HELPER_ARG)]);
    }

    #[cfg(windows)]
    #[test]
    fn timeout_kills_waits_and_leaves_no_job_process_or_reader() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_blocks_and_holds_stdout",
        );
        let image =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        let cancellation = cancellation();
        let result = exchange(&spec, &image, vec![1, 2, 3], unix_ms() + 120, cancellation);
        assert_eq!(result.output, Err(ProcessError::Timeout));
        assert!(result.terminated);
        assert_ne!(result.pid, 0);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[cfg(windows)]
    #[test]
    fn output_flood_kills_waits_and_joins_bounded_readers() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_floods_stdout",
        );
        let image =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        let result = exchange(&spec, &image, vec![1], unix_ms() + 5_000, cancellation());
        assert_eq!(result.output, Err(ProcessError::OutputLimit));
        assert!(result.terminated);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[cfg(windows)]
    #[test]
    fn cancellation_kills_waits_and_leaves_no_job_process_or_reader() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_blocks_and_holds_stdout",
        );
        let image =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        let (store, token, cancellation) = cancellation_fixture();
        let worker = thread::spawn(move || {
            exchange(
                &spec,
                &image,
                vec![1, 2, 3],
                unix_ms() + 10_000,
                cancellation,
            )
        });
        thread::sleep(Duration::from_millis(80));
        store.revoke(&token).unwrap();
        let result = worker.join().unwrap();
        assert_eq!(result.output, Err(ProcessError::Cancelled));
        assert!(result.terminated);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[cfg(windows)]
    #[test]
    fn wall_deadline_kills_even_when_monotonic_budget_remains() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_blocks_and_holds_stdout",
        );
        let image =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        let wall = AtomicU64::new(1_000);
        let result =
            exchange_with_wall_clock(&spec, &image, vec![1], 5_000, cancellation(), || {
                wall.fetch_add(4_000, Ordering::AcqRel)
            });
        assert_eq!(result.output, Err(ProcessError::Timeout));
        assert!(result.terminated);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[cfg(windows)]
    #[test]
    fn stderr_flood_kills_waits_and_joins_bounded_readers() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_floods_stderr",
        );
        let image =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        let result = exchange(&spec, &image, vec![1], unix_ms() + 5_000, cancellation());
        assert_eq!(result.output, Err(ProcessError::OutputLimit));
        assert!(result.terminated);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[cfg(windows)]
    #[test]
    fn image_mismatch_is_rejected_before_payload_is_written() {
        let spec = ProcessSpec::test_helper(
            "collision_assessor::collector_process::tests::helper_blocks_and_holds_stdout",
        );
        let mut wrong =
            physical_path_identity(&spec.executable, PhysicalPathKind::RegularFile).unwrap();
        wrong.file_id[0] ^= 0xff;
        let result = exchange(&spec, &wrong, vec![1], unix_ms() + 5_000, cancellation());
        assert_eq!(result.output, Err(ProcessError::ImageMismatch));
        assert!(result.terminated);
        assert_eq!(result.active_processes_after_wait, 0);
    }

    #[ignore]
    #[test]
    fn helper_blocks_and_holds_stdout() {
        print!("blocked");
        let _ = std::io::stdout().flush();
        thread::sleep(Duration::from_secs(30));
    }

    #[ignore]
    #[test]
    fn helper_floods_stdout() {
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..256 {
            let _ = std::io::stdout().write_all(&chunk);
        }
        thread::sleep(Duration::from_secs(30));
    }

    #[ignore]
    #[test]
    fn helper_floods_stderr() {
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..256 {
            let _ = std::io::stderr().write_all(&chunk);
        }
        thread::sleep(Duration::from_secs(30));
    }
}
