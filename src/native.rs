use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        JobObjects::*, LibraryLoader::*, Memory::*, Registry::*, SystemInformation::GetTickCount64,
        Threading::*,
    },
    UI::{HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

pub const PAUSED: u32 = 1;
pub const STOP: u32 = 2;
pub const RECOVERING: u32 = 1;
pub const LIVE: u32 = 2;
pub const IDLE: u32 = 3;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn tick() -> u64 {
    unsafe { GetTickCount64() }
}
pub fn home() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
pub fn data_dir() -> PathBuf {
    home().join("data")
}
pub fn runtime() -> PathBuf {
    std::env::var_os("PYRA_RUNTIME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("onnxruntime.dll"))
}

fn require_supported_windows() -> Result<()> {
    #[repr(C)]
    struct Version {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        text: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(version: *mut Version) -> i32;
    }
    let mut version: Version = unsafe { zeroed() };
    version.size = size_of::<Version>() as u32;
    ensure!(
        unsafe { RtlGetVersion(&mut version) } == 0 && version.build >= 19041,
        "Pyra requires Windows 10 version 2004 (build 19041) or newer."
    );
    Ok(())
}

pub fn log(message: &str) {
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    let name = if std::env::args().any(|a| a == "--worker") {
        "worker.log"
    } else {
        "pyra.log"
    };
    let path = dir.join(name);
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = fs::rename(dir.join(format!("{name}.1")), dir.join(format!("{name}.2")));
        let _ = fs::rename(&path, dir.join(format!("{name}.1")));
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} pid={} {message}", tick(), std::process::id());
    }
}

pub fn report_fatal(message: &str) {
    log(message);
    if !std::env::args().any(|a| a == "--worker") {
        unsafe {
            MessageBoxW(
                null_mut(),
                wide(message).as_ptr(),
                wide("Pyra").as_ptr(),
                MB_ICONERROR,
            );
        }
    }
}

#[repr(C)]
pub struct Shared {
    pub heartbeat: AtomicU64,
    pub flags: AtomicU32,
    pub mode: AtomicU32,
    pub threshold: AtomicU32,
    pub width: AtomicU32,
    pub thickness: AtomicU32,
    pub status: AtomicU32,
    pub faces: AtomicU32,
    pub monitors: AtomicU32,
    pub cycle_ms: AtomicU32,
}
impl Shared {
    fn new(c: &Config) -> Self {
        Self {
            heartbeat: AtomicU64::new(tick()),
            flags: AtomicU32::new(0),
            mode: AtomicU32::new(c.mode),
            threshold: AtomicU32::new(c.threshold),
            width: AtomicU32::new(c.width),
            thickness: AtomicU32::new(c.thickness),
            status: AtomicU32::new(RECOVERING),
            faces: AtomicU32::new(0),
            monitors: AtomicU32::new(0),
            cycle_ms: AtomicU32::new(0),
        }
    }
    pub fn read(v: &AtomicU32) -> u32 {
        v.load(Ordering::Relaxed)
    }
    pub fn write(v: &AtomicU32, value: u32) {
        v.store(value, Ordering::Relaxed);
    }
}

pub struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
impl Mapping {
    fn create(name: &str, config: &Config) -> Result<Self> {
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                null(),
                PAGE_READWRITE,
                0,
                size_of::<Shared>() as u32,
                wide(name).as_ptr(),
            );
            ensure!(!handle.is_null(), "create shared worker state");
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<Shared>());
            if view.Value.is_null() {
                CloseHandle(handle);
                anyhow::bail!("map shared state");
            }
            (view.Value as *mut Shared).write(Shared::new(config));
            Ok(Self { handle, view })
        }
    }
    pub fn open(name: &str) -> Result<Self> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide(name).as_ptr());
            ensure!(!handle.is_null(), "open shared worker state");
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<Shared>());
            if view.Value.is_null() {
                CloseHandle(handle);
                anyhow::bail!("map worker state");
            }
            Ok(Self { handle, view })
        }
    }
    pub fn shared(&self) -> &Shared {
        unsafe { &*(self.view.Value as *const Shared) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.handle);
        }
    }
}

pub struct Job(HANDLE);
impl Job {
    fn new() -> Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(null(), null());
            ensure!(!handle.is_null(), "create worker job");
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
                ) != 0,
                "set worker lifetime limits"
            );
            Ok(job)
        }
    }
    fn assign(&self, child: &Child) -> Result<()> {
        unsafe {
            use std::os::windows::io::AsRawHandle;
            ensure!(
                AssignProcessToJobObject(self.0, child.as_raw_handle()) != 0,
                "assign worker to supervisor job: {}",
                std::io::Error::last_os_error()
            );
            Ok(())
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    mode: u32,
    threshold: u32,
    width: u32,
    thickness: u32,
    startup: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            mode: 0,
            threshold: 65,
            width: 100,
            thickness: 100,
            startup: false,
        }
    }
}
impl Config {
    fn load() -> Self {
        let mut c = Self::default();
        if let Ok(text) = fs::read_to_string(data_dir().join("settings.ini")) {
            for line in text.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let Ok(n) = value.trim().parse::<u32>() else {
                    continue;
                };
                match key.trim() {
                    "mode" => c.mode = n.min(2),
                    "threshold" => c.threshold = n.clamp(45, 90),
                    "width" => c.width = n.clamp(80, 170),
                    "thickness" => c.thickness = n.clamp(70, 200),
                    "startup" => c.startup = n != 0,
                    _ => {}
                }
            }
        }
        c
    }
    fn save(&self) -> Result<()> {
        fs::create_dir_all(data_dir())?;
        let path = data_dir().join("settings.ini");
        let tmp = data_dir().join("settings.tmp");
        fs::write(
            &tmp,
            format!(
                "mode={}\nthreshold={}\nwidth={}\nthickness={}\nstartup={}\n",
                self.mode,
                self.threshold,
                self.width,
                self.thickness,
                u8::from(self.startup)
            ),
        )?;
        // MoveFileExW replaces atomically on Windows (std::fs::rename cannot
        // replace an existing destination there on all supported configurations).
        unsafe {
            use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};
            ensure!(
                MoveFileExW(
                    wide(&tmp.to_string_lossy()).as_ptr(),
                    wide(&path.to_string_lossy()).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING
                ) != 0,
                "save settings: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
}

const TRAY_MESSAGE: u32 = WM_APP + 1;
const ID_PAUSE: usize = 10;
const ID_QUIT: usize = 11;
const ID_STARTUP: usize = 12;
const ID_LOGS: usize = 13;
const ID_ABOUT: usize = 14;
static mut APP: *mut Controller = null_mut();
static IN_CALLBACK: AtomicBool = AtomicBool::new(false);

struct Controller {
    hwnd: HWND,
    mapping: Mapping,
    mapping_name: String,
    config: Config,
    child: Option<Child>,
    job: Option<Job>,
    started: Instant,
    next_start: Instant,
    last_tick: u64,
    failures: u32,
    paused: bool,
    stopping: bool,
    icon: HICON,
}
impl Controller {
    fn shared(&self) -> &Shared {
        self.mapping.shared()
    }
    fn start_worker(&mut self) -> Result<()> {
        self.shared().heartbeat.store(tick(), Ordering::Relaxed);
        Shared::write(&self.shared().flags, if self.paused { PAUSED } else { 0 });
        Shared::write(&self.shared().status, RECOVERING);
        let job = Job::new()?;
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--worker", &self.mapping_name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("start inference worker")?;
        if let Err(e) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        log(&format!("worker started pid={}", child.id()));
        self.child = Some(child);
        self.job = Some(job);
        self.started = Instant::now();
        Ok(())
    }
    fn kill_worker(&mut self) {
        // Closing the job reaps the worker even when inference is stuck in native code.
        self.job.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn save(&self) {
        let s = self.shared();
        Shared::write(&s.mode, self.config.mode);
        Shared::write(&s.threshold, self.config.threshold);
        Shared::write(&s.width, self.config.width);
        Shared::write(&s.thickness, self.config.thickness);
        if let Err(e) = self.config.save() {
            log(&format!("settings: {e:#}"));
        }
    }
    fn update(&mut self) {
        let now = tick();
        // Sleep/resume can advance uptime before the worker gets a scheduling
        // quantum. Give it a fresh recovery window instead of restarting at wake.
        if now.saturating_sub(self.last_tick) > 5000 {
            self.shared().heartbeat.store(now, Ordering::Relaxed);
        }
        self.last_tick = now;
        let dead = self
            .child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_some());
        let stalled = now.saturating_sub(self.shared().heartbeat.load(Ordering::Relaxed)) > 15_000;
        let recycle = self.started.elapsed() > Duration::from_secs(12 * 3600);
        if self.child.is_some() && (dead || stalled || recycle) {
            log(&format!(
                "worker recovery dead={dead} stalled={stalled} scheduled={recycle}"
            ));
            self.failures = if self.started.elapsed() < Duration::from_secs(60) {
                self.failures.saturating_add(1)
            } else {
                0
            };
            self.kill_worker();
            Shared::write(&self.shared().faces, 0);
            Shared::write(&self.shared().status, RECOVERING);
            self.next_start =
                Instant::now() + Duration::from_secs((1u64 << self.failures.min(5)).min(30));
        }
        if self.child.is_none()
            && Instant::now() >= self.next_start
            && !self.stopping
            && let Err(e) = self.start_worker()
        {
            log(&format!("worker spawn: {e:#}"));
            self.next_start = Instant::now() + Duration::from_secs(10);
        }
        self.tray(NIM_MODIFY);
    }
    fn status(&self) -> String {
        if self.paused {
            return "Pyra — paused".into();
        }
        let s = self.shared();
        if Shared::read(&s.status) == RECOVERING {
            return "Pyra — recovering capture/detection".into();
        }
        format!(
            "Pyra — {} · {} faces · {} screens · {} ms",
            if Shared::read(&s.status) == IDLE {
                "idle"
            } else {
                "live"
            },
            Shared::read(&s.faces),
            Shared::read(&s.monitors),
            Shared::read(&s.cycle_ms)
        )
    }
    fn tray(&self, action: u32) {
        unsafe {
            let mut n: NOTIFYICONDATAW = zeroed();
            n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            n.hWnd = self.hwnd;
            n.uID = 1;
            n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
            n.uCallbackMessage = TRAY_MESSAGE;
            n.hIcon = self.icon;
            let text = wide(&self.status());
            let len = text.len().min(n.szTip.len() - 1);
            n.szTip[..len].copy_from_slice(&text[..len]);
            Shell_NotifyIconW(action, &n);
        }
    }
    fn toggle(&mut self) {
        self.paused = !self.paused;
        Shared::write(&self.shared().flags, if self.paused { PAUSED } else { 0 });
        self.tray(NIM_MODIFY);
    }
    fn menu(&mut self) {
        unsafe {
            let menu = CreatePopupMenu();
            let add = |m, id, text: &str, checked: bool| {
                AppendMenuW(
                    m,
                    MF_STRING | if checked { MF_CHECKED } else { 0 },
                    id,
                    wide(text).as_ptr(),
                );
            };
            AppendMenuW(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                wide(&self.status()).as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            add(
                menu,
                ID_PAUSE,
                if self.paused {
                    "Resume eye bars\tCtrl+Alt+B"
                } else {
                    "Pause eye bars\tCtrl+Alt+B"
                },
                false,
            );
            let mode = CreatePopupMenu();
            for (i, text) in ["Balanced", "More detail", "Low power"].iter().enumerate() {
                add(mode, 100 + i, text, self.config.mode as usize == i);
            }
            AppendMenuW(menu, MF_POPUP, mode as usize, wide("Performance").as_ptr());
            for (base, label, values, current) in [
                (
                    200,
                    "Bar width",
                    &[90, 100, 120, 145][..],
                    self.config.width,
                ),
                (
                    300,
                    "Bar thickness",
                    &[80, 100, 130, 165][..],
                    self.config.thickness,
                ),
                (400, "Sensitivity", &[80, 65, 50][..], self.config.threshold),
            ] {
                let submenu = CreatePopupMenu();
                for (i, &v) in values.iter().enumerate() {
                    let text = if base == 400 {
                        [
                            "Conservative",
                            "Balanced",
                            "Catch more (more false positives)",
                        ][i]
                            .into()
                    } else {
                        format!("{v}%")
                    };
                    add(submenu, base + i, &text, v == current);
                }
                AppendMenuW(menu, MF_POPUP, submenu as usize, wide(label).as_ptr());
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            add(
                menu,
                ID_STARTUP,
                "Start when I sign in",
                self.config.startup,
            );
            add(menu, ID_LOGS, "Open diagnostic folder", false);
            add(menu, ID_ABOUT, "About Pyra", false);
            add(menu, ID_QUIT, "Quit Pyra", false);
            let mut point: POINT = zeroed();
            GetCursorPos(&mut point);
            SetForegroundWindow(self.hwnd);
            let choice = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                self.hwnd,
                null(),
            );
            DestroyMenu(menu);
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            self.action(choice as usize);
        }
    }
    fn action(&mut self, id: usize) {
        match id {
            ID_PAUSE => self.toggle(),
            ID_QUIT => unsafe {
                PostQuitMessage(0);
            },
            100..=102 => self.config.mode = (id - 100) as u32,
            200..=203 => self.config.width = [90, 100, 120, 145][id - 200],
            300..=303 => self.config.thickness = [80, 100, 130, 165][id - 300],
            400..=402 => self.config.threshold = [80, 65, 50][id - 400],
            ID_STARTUP => {
                let desired = !self.config.startup;
                match startup(desired) {
                    Ok(()) => self.config.startup = desired,
                    Err(e) => report_fatal(&format!("Could not update sign-in startup: {e:#}")),
                }
            }
            ID_LOGS => unsafe {
                ShellExecuteW(
                    self.hwnd,
                    wide("open").as_ptr(),
                    wide(&data_dir().to_string_lossy()).as_ptr(),
                    null(),
                    null(),
                    SW_SHOWNORMAL,
                );
            },
            ID_ABOUT => unsafe {
                MessageBoxW(self.hwnd, wide("Pyra 0.2.1\nLocal automatic eye bars across all displays.\n\nCtrl+Alt+B: pause/resume\nCtrl+Alt+O: tray menu\n\nNo screenshots are saved or uploaded.\nSmall/profile faces and fast cuts may be missed.\nUse borderless/windowed mode for fullscreen apps.\n\nRust + native Windows APIs + ONNX Runtime / YuNet.\nGPL-3.0. See README for testing and limits.").as_ptr(), wide("About Pyra").as_ptr(), MB_OK | MB_ICONINFORMATION);
            },
            _ => return,
        }
        self.save();
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stopping = true;
        unsafe {
            KillTimer(self.hwnd, 1);
            UnregisterHotKey(self.hwnd, 1);
            UnregisterHotKey(self.hwnd, 2);
            APP = null_mut();
        }
        self.kill_worker();
        self.tray(NIM_DELETE);
        unsafe {
            DestroyIcon(self.icon);
            DestroyWindow(self.hwnd);
        }
        log("Pyra stopped cleanly");
    }
}

fn startup(enabled: bool) -> Result<()> {
    unsafe {
        let mut key = null_mut();
        ensure!(
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key
            ) == ERROR_SUCCESS,
            "open sign-in startup key"
        );
        let name = wide("Pyra");
        let result = if enabled {
            let value = wide(&format!(
                "\"{}\" --background",
                std::env::current_exe()?.display()
            ));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } else {
            let code = RegDeleteValueW(key, name.as_ptr());
            if code == ERROR_FILE_NOT_FOUND {
                ERROR_SUCCESS
            } else {
                code
            }
        };
        RegCloseKey(key);
        ensure!(
            result == ERROR_SUCCESS,
            "change sign-in startup: error {result}"
        );
        Ok(())
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_CLOSE {
            EndMenu();
            PostQuitMessage(0);
            return 0;
        }
        // TrackPopupMenu runs a nested Windows message loop. Suppress reentrant
        // callbacks so it cannot alias Controller's active mutable reference.
        if IN_CALLBACK.swap(true, Ordering::Relaxed) {
            return DefWindowProcW(hwnd, message, w, l);
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                IN_CALLBACK.store(false, Ordering::Relaxed);
            }
        }
        let _reset = Reset;
        if !APP.is_null() {
            match message {
                WM_TIMER => {
                    (&mut *APP).update();
                    return 0;
                }
                WM_HOTKEY => {
                    if w == 1 {
                        (&mut *APP).toggle();
                    } else {
                        (&mut *APP).menu();
                    }
                    return 0;
                }
                TRAY_MESSAGE => {
                    if [WM_RBUTTONUP, WM_LBUTTONUP, WM_CONTEXTMENU].contains(&(l as u32)) {
                        (&mut *APP).menu();
                    }
                    return 0;
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}

fn icon() -> HICON {
    unsafe {
        // 16x16 black bar on a lavender tile. BGRA color bitmap + opaque AND mask.
        let mut pixels = [0xffb8aaffu32; 256];
        for y in 5..11 {
            for x in 1..15 {
                pixels[y * 16 + x] = 0xff000000;
            }
        }
        let mask = [0u8; 32];
        CreateIcon(
            GetModuleHandleW(null()),
            16,
            16,
            1,
            32,
            mask.as_ptr(),
            pixels.as_ptr().cast(),
        )
    }
}

pub fn run() -> Result<()> {
    require_supported_windows()?;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--status" || a == "--quit") {
        unsafe {
            let hwnd = FindWindowW(wide("Pyra.Supervisor").as_ptr(), null());
            ensure!(!hwnd.is_null(), "Pyra is not running");
            if args.iter().any(|a| a == "--quit") {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            } else {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let mapping = Mapping::open(&format!("Local\\Pyra.State.{pid}"))?;
                let s = mapping.shared();
                println!(
                    "pid={pid} status={} paused={} faces={} monitors={} cycle_ms={} heartbeat_age_ms={}",
                    Shared::read(&s.status),
                    Shared::read(&s.flags) & PAUSED != 0,
                    Shared::read(&s.faces),
                    Shared::read(&s.monitors),
                    Shared::read(&s.cycle_ms),
                    tick().saturating_sub(s.heartbeat.load(Ordering::Relaxed))
                );
            }
        }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--worker") {
        let name = args.get(i + 1).context("missing worker mapping name")?;
        return crate::worker::run(name);
    }
    if args.iter().any(|a| a == "--diagnostics") {
        let monitors = crate::capture::monitors()?;
        let mut detector = crate::detection::Detector::new(&runtime())?;
        let pixels = vec![0u8; 640 * 640 * 4];
        let start = Instant::now();
        let faces = detector.detect(
            &crate::detection::Image {
                pixels: &pixels,
                width: 640,
                height: 640,
            },
            crate::detection::Region {
                x: 0,
                y: 0,
                w: 640,
                h: 640,
            },
            0.65,
        )?;
        log(&format!(
            "diagnostics: displays={monitors:?}, blank faces={}, cold inference={:?}",
            faces.len(),
            start.elapsed()
        ));
        return Ok(());
    }
    ensure!(
        runtime().is_file(),
        "onnxruntime.dll is missing. Keep it beside Pyra.exe."
    );
    fs::create_dir_all(data_dir()).context("Pyra needs write access to its data folder")?;
    unsafe {
        let mutex = CreateMutexW(null(), 0, wide("Local\\Pyra.SingleInstance.v1").as_ptr());
        ensure!(!mutex.is_null(), "create instance mutex");
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(mutex);
            return Ok(());
        }
        let instance = GetModuleHandleW(null());
        let class = wide("Pyra.Supervisor");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        ensure!(RegisterClassW(&wc) != 0, "register tray window");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("Pyra").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        ensure!(!hwnd.is_null(), "create supervisor window");
        let config = Config::load();
        let mapping_name = format!("Local\\Pyra.State.{}", std::process::id());
        let mapping = Mapping::create(&mapping_name, &config)?;
        let mut controller = Box::new(Controller {
            hwnd,
            mapping,
            mapping_name,
            config,
            child: None,
            job: None,
            started: Instant::now(),
            next_start: Instant::now(),
            last_tick: tick(),
            failures: 0,
            paused: false,
            stopping: false,
            icon: icon(),
        });
        APP = &mut *controller;
        controller.tray(NIM_ADD);
        for (id, key) in [(1, b'B'), (2, b'O')] {
            if RegisterHotKey(hwnd, id, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, key as u32) == 0 {
                log(&format!(
                    "Ctrl+Alt+{} unavailable; use tray menu",
                    key as char
                ));
            }
        }
        let taskbar_message = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        controller.start_worker()?;
        SetTimer(hwnd, 1, 500, None);
        log("Pyra supervisor started");
        if !args.iter().any(|a| a == "--background") {
            let mut notice: NOTIFYICONDATAW = zeroed();
            notice.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            notice.hWnd = hwnd;
            notice.uID = 1;
            notice.uFlags = NIF_INFO;
            let title = wide("Pyra is running");
            notice.szInfoTitle[..title.len()].copy_from_slice(&title);
            let text = wide(
                "Eye bars are automatic. Click the tray icon for settings. Ctrl+Alt+B pauses or resumes.",
            );
            notice.szInfo[..text.len()].copy_from_slice(&text);
            notice.dwInfoFlags = NIIF_INFO;
            Shell_NotifyIconW(NIM_MODIFY, &notice);
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == taskbar_message {
                controller.tray(NIM_ADD);
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(controller);
        CloseHandle(mutex);
    }
    Ok(())
}
