//! Idle process-memory reclamation after terminal output bursts.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct MemoryTrimDiagnostics {
    pub enabled: bool,
    pub idle_delay_ms: u64,
    pub gpu_trim_available: bool,
    pub trim_count: u64,
    pub last_gpu_trimmed: bool,
    pub last_reclaimed_bytes: Option<u64>,
    pub last_working_set_bytes: Option<u64>,
}

#[cfg(windows)]
mod platform {
    use super::MemoryTrimDiagnostics;
    use std::collections::HashSet;
    use std::ffi::{c_void, CStr};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows::core::{IUnknown, Interface};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice3;

    const STARTUP_TRIM_DELAY: Duration = Duration::from_secs(2);
    const IDLE_TRIM_DELAY: Duration = Duration::from_secs(10);
    const MIN_WORKING_SET_BYTES: u64 = 24 * 1024 * 1024;

    #[derive(Debug)]
    struct IdleTrimState {
        dense_tabs: HashSet<String>,
        started_at: Instant,
        startup_trim_pending: bool,
        last_activity: Instant,
        trim_requested: bool,
        trim_count: u64,
        last_gpu_trimmed: bool,
        last_reclaimed_bytes: Option<u64>,
        last_working_set_bytes: Option<u64>,
    }

    impl IdleTrimState {
        fn new(now: Instant) -> Self {
            Self {
                dense_tabs: HashSet::new(),
                started_at: now,
                startup_trim_pending: true,
                last_activity: now,
                trim_requested: true,
                trim_count: 0,
                last_gpu_trimmed: false,
                last_reclaimed_bytes: None,
                last_working_set_bytes: None,
            }
        }

        fn observe(&mut self, tab_id: &str, dense: bool, now: Instant) {
            self.last_activity = now;
            self.trim_requested = true;
            if dense {
                self.dense_tabs.insert(tab_id.to_string());
            } else {
                self.dense_tabs.remove(tab_id);
            }
        }

        fn forget(&mut self, tab_id: &str, now: Instant) {
            self.last_activity = now;
            self.trim_requested = true;
            self.dense_tabs.remove(tab_id);
        }

        fn request(&mut self, now: Instant) {
            self.last_activity = now;
            self.trim_requested = true;
        }

        fn wait_before_trim(&self, now: Instant) -> Option<Duration> {
            if !self.trim_requested {
                return None;
            }
            // Reclaim renderer initialization pages promptly even when the user
            // opens a dense TUI before the normal idle timer can fire. This is a
            // one-time startup trim; later trims still wait for every dense tab
            // to become sparse so btop does not incur recurring page faults.
            if self.startup_trim_pending {
                return Some(
                    STARTUP_TRIM_DELAY
                        .checked_sub(now.saturating_duration_since(self.started_at))
                        .unwrap_or_default(),
                );
            }
            if !self.dense_tabs.is_empty() {
                return None;
            }
            Some(
                IDLE_TRIM_DELAY
                    .checked_sub(now.saturating_duration_since(self.last_activity))
                    .unwrap_or_default(),
            )
        }

        fn claim_trim(&mut self) {
            self.trim_requested = false;
            self.startup_trim_pending = false;
        }
    }

    struct SharedState {
        state: Mutex<IdleTrimState>,
        wake: Condvar,
    }

    static SHARED: OnceLock<Arc<SharedState>> = OnceLock::new();
    static DXGI_DEVICE: OnceLock<Mutex<Option<IDXGIDevice3>>> = OnceLock::new();

    fn dxgi_device() -> &'static Mutex<Option<IDXGIDevice3>> {
        DXGI_DEVICE.get_or_init(|| Mutex::new(None))
    }

    fn shared() -> &'static Arc<SharedState> {
        SHARED.get_or_init(|| {
            let shared = Arc::new(SharedState {
                state: Mutex::new(IdleTrimState::new(Instant::now())),
                wake: Condvar::new(),
            });
            let worker_state = shared.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("terminal-memory-trim".to_string())
                .spawn(move || trim_worker(worker_state))
            {
                tracing::warn!(%error, "failed to start idle memory trimmer");
            }
            shared
        })
    }

    pub(crate) fn initialize() {
        let _ = shared();
    }

    pub(crate) fn record_terminal_activity(tab_id: &str, dense: bool) {
        let shared = shared();
        shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observe(tab_id, dense, Instant::now());
        shared.wake.notify_one();
    }

    pub(crate) fn forget_terminal(tab_id: &str) {
        let shared = shared();
        shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .forget(tab_id, Instant::now());
        shared.wake.notify_one();
    }

    pub(crate) fn request_idle_trim() {
        let shared = shared();
        shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .request(Instant::now());
        shared.wake.notify_one();
    }

    pub(crate) fn clear_graphics_device() {
        *dxgi_device()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Capture ANGLE's underlying D3D11 device while its EGL context is current.
    /// The queried IDXGIDevice3 owns its own COM reference and is free-threaded.
    pub(crate) unsafe fn register_angle_graphics_device(
        get_proc_address: &dyn Fn(&CStr) -> *const c_void,
    ) -> bool {
        type EglGetCurrentDisplay = unsafe extern "system" fn() -> *mut c_void;
        type EglQueryDisplayAttrib = unsafe extern "system" fn(*mut c_void, i32, *mut isize) -> u32;
        type EglQueryDeviceAttrib = unsafe extern "system" fn(*mut c_void, i32, *mut isize) -> u32;

        let get_current_display = get_proc_address(c"eglGetCurrentDisplay");
        let query_display_attrib = get_proc_address(c"eglQueryDisplayAttribEXT");
        let query_device_attrib = get_proc_address(c"eglQueryDeviceAttribEXT");
        if get_current_display.is_null()
            || query_display_attrib.is_null()
            || query_device_attrib.is_null()
        {
            return false;
        }

        let get_current_display: EglGetCurrentDisplay =
            unsafe { std::mem::transmute(get_current_display) };
        let query_display_attrib: EglQueryDisplayAttrib =
            unsafe { std::mem::transmute(query_display_attrib) };
        let query_device_attrib: EglQueryDeviceAttrib =
            unsafe { std::mem::transmute(query_device_attrib) };

        const EGL_DEVICE_EXT: i32 = 0x322C;
        const EGL_D3D11_DEVICE_ANGLE: i32 = 0x33A1;
        let display = unsafe { get_current_display() };
        if display.is_null() {
            return false;
        }
        let mut egl_device = 0isize;
        if unsafe { query_display_attrib(display, EGL_DEVICE_EXT, &mut egl_device) } == 0
            || egl_device == 0
        {
            return false;
        }
        let mut d3d11_device = 0isize;
        if unsafe {
            query_device_attrib(
                egl_device as *mut c_void,
                EGL_D3D11_DEVICE_ANGLE,
                &mut d3d11_device,
            )
        } == 0
            || d3d11_device == 0
        {
            return false;
        }

        let raw = d3d11_device as *mut c_void;
        let Some(unknown) = (unsafe { IUnknown::from_raw_borrowed(&raw) }) else {
            return false;
        };
        let Ok(device) = unknown.cast::<IDXGIDevice3>() else {
            return false;
        };
        *dxgi_device()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(device);
        true
    }

    pub(crate) fn diagnostics() -> MemoryTrimDiagnostics {
        let Some(shared) = SHARED.get() else {
            return MemoryTrimDiagnostics {
                enabled: true,
                idle_delay_ms: IDLE_TRIM_DELAY.as_millis() as u64,
                ..MemoryTrimDiagnostics::default()
            };
        };
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        MemoryTrimDiagnostics {
            enabled: true,
            idle_delay_ms: IDLE_TRIM_DELAY.as_millis() as u64,
            gpu_trim_available: dxgi_device()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            trim_count: state.trim_count,
            last_gpu_trimmed: state.last_gpu_trimmed,
            last_reclaimed_bytes: state.last_reclaimed_bytes,
            last_working_set_bytes: state.last_working_set_bytes,
        }
    }

    fn trim_worker(shared: Arc<SharedState>) {
        loop {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                match state.wait_before_trim(Instant::now()) {
                    None => {
                        state = shared
                            .wake
                            .wait(state)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                    Some(wait) if !wait.is_zero() => {
                        let (next, _) = shared
                            .wake
                            .wait_timeout(state, wait)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state = next;
                    }
                    Some(_) => {
                        state.claim_trim();
                        break;
                    }
                }
            }
            drop(state);

            let Some(result) = trim_process_memory() else {
                continue;
            };
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.trim_count = state.trim_count.saturating_add(1);
            state.last_gpu_trimmed = result.gpu_trimmed;
            state.last_reclaimed_bytes = Some(result.before.saturating_sub(result.after));
            state.last_working_set_bytes = Some(result.after);
            tracing::debug!(
                before_bytes = result.before,
                after_bytes = result.after,
                heap_optimized = result.heap_optimized,
                gpu_trimmed = result.gpu_trimmed,
                "trimmed idle terminal working set"
            );
        }
    }

    struct TrimResult {
        before: u64,
        after: u64,
        heap_optimized: bool,
        gpu_trimmed: bool,
    }

    #[repr(C)]
    struct HeapOptimizeResourcesInformation {
        version: u32,
        flags: u32,
    }

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn HeapSetInformation(
            heap: isize,
            information_class: u32,
            information: *const c_void,
            information_length: usize,
        ) -> i32;
        fn SetProcessWorkingSetSize(process: isize, minimum: usize, maximum: usize) -> i32;
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(process: isize, counters: *mut u8, size: u32) -> i32;
    }

    fn working_set_bytes() -> Option<u64> {
        let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut ProcessMemoryCounters).cast(),
                counters.cb,
            )
        };
        (ok != 0).then_some(counters.working_set_size as u64)
    }

    fn trim_process_memory() -> Option<TrimResult> {
        let before = working_set_bytes()?;
        if before < MIN_WORKING_SET_BYTES {
            return None;
        }

        let gpu_trimmed = trim_graphics_memory();
        let information = HeapOptimizeResourcesInformation {
            version: 1,
            flags: 0,
        };
        let heap_optimized = unsafe {
            HeapSetInformation(
                0,
                3,
                (&information as *const HeapOptimizeResourcesInformation).cast(),
                std::mem::size_of::<HeapOptimizeResourcesInformation>(),
            ) != 0
        };
        let working_set_trimmed =
            unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) != 0 };
        if !working_set_trimmed {
            tracing::debug!(
                heap_optimized,
                "Windows refused to trim the idle terminal working set"
            );
            return None;
        }

        std::thread::sleep(Duration::from_millis(50));
        let after = working_set_bytes().unwrap_or(before);
        Some(TrimResult {
            before,
            after,
            heap_optimized,
            gpu_trimmed,
        })
    }

    fn trim_graphics_memory() -> bool {
        let device = dxgi_device()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(device) = device else {
            return false;
        };
        unsafe { device.Trim() };
        true
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dense_terminal_blocks_trim_until_it_becomes_sparse() {
            let start = Instant::now();
            let mut state = IdleTrimState::new(start);
            state.claim_trim();
            state.observe("a", true, start + Duration::from_secs(1));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(30)),
                None
            );

            state.observe("a", false, start + Duration::from_secs(31));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(35)),
                Some(Duration::from_secs(6))
            );
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(41)),
                Some(Duration::ZERO)
            );
        }

        #[test]
        fn another_dense_tab_keeps_process_trim_deferred() {
            let start = Instant::now();
            let mut state = IdleTrimState::new(start);
            state.claim_trim();
            state.observe("a", true, start);
            state.observe("b", true, start);
            state.forget("a", start + Duration::from_secs(1));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(20)),
                None
            );
            state.forget("b", start + Duration::from_secs(21));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(31)),
                Some(Duration::ZERO)
            );
        }

        #[test]
        fn explicit_request_restarts_the_idle_delay() {
            let start = Instant::now();
            let mut state = IdleTrimState::new(start);
            state.claim_trim();

            state.request(start + Duration::from_secs(20));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(25)),
                Some(Duration::from_secs(5))
            );
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(30)),
                Some(Duration::ZERO)
            );
        }

        #[test]
        fn one_time_startup_trim_is_not_blocked_by_dense_terminal() {
            let start = Instant::now();
            let mut state = IdleTrimState::new(start);
            state.observe("a", true, start + Duration::from_secs(1));

            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(1)),
                Some(Duration::from_secs(1))
            );
            assert_eq!(
                state.wait_before_trim(start + STARTUP_TRIM_DELAY),
                Some(Duration::ZERO)
            );

            state.claim_trim();
            state.observe("a", true, start + Duration::from_secs(3));
            assert_eq!(
                state.wait_before_trim(start + Duration::from_secs(30)),
                None
            );
        }
    }
}

#[cfg(windows)]
pub(crate) use platform::{
    diagnostics, forget_terminal, initialize, record_terminal_activity, request_idle_trim,
};

#[cfg(windows)]
pub(crate) use platform::{clear_graphics_device, register_angle_graphics_device};

#[cfg(not(windows))]
pub(crate) fn initialize() {}

#[cfg(not(windows))]
pub(crate) fn record_terminal_activity(_tab_id: &str, _dense: bool) {}

#[cfg(not(windows))]
pub(crate) fn forget_terminal(_tab_id: &str) {}

#[cfg(not(windows))]
pub(crate) fn request_idle_trim() {}

#[cfg(not(windows))]
pub(crate) fn diagnostics() -> MemoryTrimDiagnostics {
    MemoryTrimDiagnostics::default()
}
