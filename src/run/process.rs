#![allow(non_camel_case_types, non_snake_case, clippy::upper_case_acronyms)]

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MonitorExit {
    ProcessExited(u32),
    ReloadRequested,
    Interrupted,
}

pub(super) fn shutdown_aviutl2(exe: &Path) -> anyhow::Result<usize> {
    let Ok(exe) = std::fs::canonicalize(exe) else {
        return Ok(0);
    };

    let timeout = u32::try_from(SHUTDOWN_TIMEOUT.as_millis())?;
    let snapshot = UniqueHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if !snapshot.is_valid() {
        return Ok(0);
    }

    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = DWORD::try_from(std::mem::size_of::<PROCESSENTRY32W>())?;

    let mut result = unsafe { Process32FirstW(snapshot.raw(), &raw mut entry) };
    let mut closed = 0;

    while result != 0 {
        let exe_name = String::from_utf16_lossy(
            &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len())],
        );

        if exe_name.eq_ignore_ascii_case("aviutl2.exe") {
            let process = UniqueHandle(unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    0,
                    entry.th32ProcessID,
                )
            });

            if process.is_valid()
                && let Some(proc_path) = query_process_path(process.raw())
                && std::fs::canonicalize(&proc_path).is_ok_and(|p| p == exe)
            {
                tracing::info!(
                    "Requesting AviUtl ExEdit2 to shut down (PID: {}, '{}')",
                    entry.th32ProcessID,
                    proc_path.display()
                );

                request_close(entry.th32ProcessID)?;
                if unsafe { WaitForSingleObject(process.raw(), timeout) } != WAIT_OBJECT_0 {
                    tracing::warn!(
                        "graceful shutdown timed out for AviUtl ExEdit2; terminating process (PID: {})",
                        entry.th32ProcessID
                    );
                    unsafe {
                        TerminateProcess(process.raw(), 0);
                        WaitForSingleObject(process.raw(), 3000);
                    }
                }
                closed += 1;
            }
        }

        result = unsafe { Process32NextW(snapshot.raw(), &raw mut entry) };
    }

    Ok(closed)
}

struct StatusBar {
    height: SHORT,
}

impl StatusBar {
    fn start() -> Option<Self> {
        use std::io::IsTerminal;
        if !std::io::stderr().is_terminal() {
            return None;
        }

        let handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut mode = 0;
        if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
            return None;
        }

        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0
            && unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) } == 0
        {
            return None;
        }

        let mut csbi = unsafe { std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>() };
        if unsafe { GetConsoleScreenBufferInfo(handle, &raw mut csbi) } == 0 {
            return None;
        }

        let height = csbi.srWindow.Bottom - csbi.srWindow.Top + 1;
        if height < 4 {
            return None;
        }

        eprint!("\x1b[1;{}r", height - 1);
        eprint!("\x1b[{height};1H Ctrl+C: Stop | Ctrl+R / r+Enter: Reload\x1b[K");
        eprint!("\x1b[{};1H", height - 1);
        let _ = std::io::stderr().flush();

        Some(Self { height })
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        eprint!("\x1b[r\x1b[{};1H\x1b[2K", self.height);
        let _ = std::io::stderr().flush();
    }
}

pub(super) fn run_process(exe: &Path, is_running: &AtomicBool) -> anyhow::Result<MonitorExit> {
    let _status_bar = if let Some(bar) = StatusBar::start() {
        Some(bar)
    } else {
        tracing::info!("Press 'Ctrl+C' to stop, 'Ctrl+R' or 'r + Enter' to reload");
        None
    };

    let child = AviUtl2Process::spawn(exe)?;
    let exit = child.monitor(is_running)?;
    match exit {
        MonitorExit::ProcessExited(_) => {}
        MonitorExit::Interrupted => {
            tracing::info!("Stopping AviUtl ExEdit2");
            child.stop()?;
        }
        MonitorExit::ReloadRequested => {
            tracing::info!("Reloading AviUtl ExEdit2");
            child.stop()?;
        }
    }
    Ok(exit)
}

pub(super) fn wait_for_reload(is_running: &AtomicBool) -> MonitorExit {
    let mut line = 0;
    while is_running.load(Ordering::SeqCst) {
        if has_reload_key(&mut line) {
            return MonitorExit::ReloadRequested;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    MonitorExit::Interrupted
}

struct AviUtl2Process {
    handle: UniqueHandle,
    pid: u32,
}

impl AviUtl2Process {
    fn spawn(exe: &Path) -> anyhow::Result<Self> {
        use std::os::windows::ffi::OsStrExt;

        let dir = exe
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut startup = unsafe { std::mem::zeroed::<STARTUPINFOW>() };
        startup.cb = DWORD::try_from(std::mem::size_of::<STARTUPINFOW>())?;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_SHOWNOACTIVATE;
        let mut info = unsafe { std::mem::zeroed::<PROCESS_INFORMATION>() };

        if unsafe {
            CreateProcessW(
                exe.as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>()
                    .as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                DEBUG_ONLY_THIS_PROCESS,
                std::ptr::null_mut(),
                dir.as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>()
                    .as_ptr(),
                &raw mut startup,
                &raw mut info,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }

        unsafe {
            CloseHandle(info.hThread);
        }

        Ok(Self {
            handle: UniqueHandle(info.hProcess),
            pid: info.dwProcessId,
        })
    }

    fn monitor(&self, is_running: &AtomicBool) -> anyhow::Result<MonitorExit> {
        let process_handle = self.handle.raw();
        let pid = self.pid;
        let mut event = unsafe { std::mem::zeroed::<DEBUG_EVENT>() };
        let mut line = 0;

        while is_running.load(Ordering::SeqCst) {
            if has_reload_key(&mut line) {
                return Ok(MonitorExit::ReloadRequested);
            }

            if unsafe { WaitForDebugEventEx(&raw mut event, MONITOR_TIMEOUT) } == 0 {
                if unsafe { WaitForSingleObject(process_handle, 0) } == WAIT_OBJECT_0 {
                    let mut code = 0;
                    unsafe { GetExitCodeProcess(process_handle, &raw mut code) };
                    return Ok(MonitorExit::ProcessExited(code));
                }
                continue;
            }

            let exit_code = process_event(&event, process_handle)?;
            if has_reload_key(&mut line) {
                return Ok(MonitorExit::ReloadRequested);
            }
            if event.dwProcessId == pid
                && let Some(code) = exit_code
            {
                return Ok(MonitorExit::ProcessExited(code));
            }
        }

        Ok(MonitorExit::Interrupted)
    }

    fn stop(&self) -> anyhow::Result<()> {
        let pid = self.pid;
        let process_handle = self.handle.raw();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let mut event = unsafe { std::mem::zeroed::<DEBUG_EVENT>() };

        request_close(pid)?;

        while Instant::now() < deadline {
            if unsafe { WaitForDebugEventEx(&raw mut event, 100) } != 0
                && process_event(&event, process_handle)?.is_some()
            {
                return Ok(());
            }
        }

        tracing::warn!("graceful shutdown timed out for AviUtl ExEdit2; terminating process (PID: {pid})");
        let detached = unsafe { DebugActiveProcessStop(pid) != 0 };
        if unsafe { TerminateProcess(process_handle, 0) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        if !detached {
            while unsafe { WaitForDebugEventEx(&raw mut event, 100) } != 0 {
                if process_event(&event, process_handle)?.is_some() {
                    break;
                }
            }
        }

        if unsafe { WaitForSingleObject(process_handle, INFINITE) } != WAIT_OBJECT_0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

fn request_close(pid: u32) -> anyhow::Result<()> {
    let pid = LPARAM::try_from(pid)?;
    unsafe {
        EnumWindows(Some(close_window), pid);
    }
    Ok(())
}

unsafe extern "system" fn close_window(hwnd: HWND, lParam: LPARAM) -> BOOL {
    let mut pid = 0;
    if unsafe { GetWindowThreadProcessId(hwnd, &raw mut pid) } != 0
        && LPARAM::try_from(pid).ok() == Some(lParam)
        && unsafe { IsWindowVisible(hwnd) } != 0
        && unsafe { GetWindow(hwnd, GW_OWNER).is_null() }
    {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        return 0;
    }
    1
}

fn has_reload_key(line: &mut u8) -> bool {
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if input.is_null() || input == INVALID_HANDLE_VALUE {
        return false;
    }

    read_console_reload(input, line).unwrap_or_else(|| read_pipe_reload(input, line))
}

fn read_console_reload(input: HANDLE, line: &mut u8) -> Option<bool> {
    let mut records = [unsafe { std::mem::zeroed::<INPUT_RECORD>() }; INPUT_RECORDS];
    let mut count = 0;
    if unsafe {
        PeekConsoleInputW(
            input,
            records.as_mut_ptr(),
            u32::try_from(records.len()).unwrap_or(u32::MAX),
            &raw mut count,
        )
    } == 0
    {
        return None;
    }

    loop {
        if count == 0 {
            return Some(false);
        }

        let mut read = 0;
        if unsafe {
            ReadConsoleInputW(
                input,
                records.as_mut_ptr(),
                u32::try_from(records.len()).unwrap_or(u32::MAX),
                &raw mut read,
            )
        } == 0
        {
            return Some(false);
        }

        let read = usize::try_from(read).unwrap_or_default().min(records.len());
        for record in &records[..read] {
            if record.EventType != KEY_EVENT {
                continue;
            }

            let key = unsafe { record.Event.KeyEvent };
            if key.bKeyDown == 0 {
                continue;
            }

            if key.wVirtualKeyCode == VK_R && key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
                return Some(true);
            }

            match key.wVirtualKeyCode {
                VK_R if *line == 0 => *line = 1,
                VK_RETURN => {
                    if *line == 1 {
                        *line = 0;
                        return Some(true);
                    }
                    *line = 0;
                }
                VK_SHIFT | VK_CONTROL | VK_MENU => {}
                _ => *line = 2,
            }
        }

        if read < records.len() {
            return Some(false);
        }

        count = 0;
        if unsafe {
            PeekConsoleInputW(
                input,
                records.as_mut_ptr(),
                u32::try_from(records.len()).unwrap_or(u32::MAX),
                &raw mut count,
            )
        } == 0
        {
            return Some(false);
        }
    }
}

fn read_pipe_reload(input: HANDLE, line: &mut u8) -> bool {
    let mut bytes = [0_u8; INPUT_BYTES];
    let mut count = 0;
    if unsafe {
        PeekNamedPipe(
            input,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            &raw mut count,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
        || count == 0
    {
        return false;
    }

    let mut read = 0;
    if unsafe {
        ReadFile(
            input,
            bytes.as_mut_ptr().cast(),
            count,
            &raw mut read,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return false;
    }

    let read = usize::try_from(read).unwrap_or_default().min(bytes.len());
    for byte in &bytes[..read] {
        match byte {
            0x12 => return true,
            b'\r' | b'\n' => {
                if *line == 1 {
                    *line = 0;
                    return true;
                }
                *line = 0;
            }
            b'r' if *line == 0 => *line = 1,
            _ => *line = 2,
        }
    }

    false
}

fn process_event(event: &DEBUG_EVENT, process_handle: HANDLE) -> anyhow::Result<Option<u32>> {
    if event.dwDebugEventCode == OUTPUT_DEBUG_STRING_EVENT {
        log_output(event, process_handle);
    }

    close_event_file(event);

    let exit_code = if event.dwDebugEventCode == EXIT_PROCESS_DEBUG_EVENT {
        Some(unsafe { event.u.ExitProcess.dwExitCode })
    } else {
        None
    };

    let status = if event.dwDebugEventCode == EXCEPTION_DEBUG_EVENT
        && unsafe { event.u.Exception.ExceptionRecord.ExceptionCode } != STATUS_BREAKPOINT
    {
        DBG_EXCEPTION_NOT_HANDLED
    } else {
        DBG_CONTINUE
    };
    if unsafe { ContinueDebugEvent(event.dwProcessId, event.dwThreadId, status) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(exit_code)
}

fn close_event_file(event: &DEBUG_EVENT) {
    let file = unsafe {
        match event.dwDebugEventCode {
            CREATE_PROCESS_DEBUG_EVENT => event.u.CreateProcessInfo.hFile,
            LOAD_DLL_DEBUG_EVENT => event.u.LoadDll.hFile,
            _ => std::ptr::null_mut(),
        }
    };

    if !file.is_null() && file != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(file);
        }
    }
}

fn log_output(event: &DEBUG_EVENT, process_handle: HANDLE) {
    let output = unsafe { event.u.DebugString };
    if output.lpDebugStringData.is_null() || output.nDebugStringLength == 0 {
        return;
    }

    let mut bytes = vec![0; usize::from(output.nDebugStringLength)];
    let mut read = 0;
    if unsafe {
        ReadProcessMemory(
            process_handle,
            output.lpDebugStringData.cast(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &raw mut read,
        )
    } == 0
    {
        return;
    }

    bytes.truncate(read);

    let text = if output.fUnicode != 0 {
        if !bytes.as_chunks::<2>().1.is_empty() {
            tracing::warn!("invalid Unicode debug output: odd byte count");
        }
        String::from_utf16_lossy(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|chunk| u16::from_le_bytes(*chunk))
                .collect::<Vec<_>>(),
        )
    } else {
        let Ok(text) = decode_ansi(&bytes) else {
            tracing::warn!("failed to decode non-Unicode debug output");
            return;
        };
        text
    };

    for line in text.trim_end_matches('\0').lines() {
        eprintln!("{}", colorize_debug_line(line));
    }
}

fn colorize_debug_line(line: &str) -> String {
    use std::io::IsTerminal;
    use std::sync::LazyLock;

    static USE_COLOR: LazyLock<bool> =
        LazyLock::new(|| std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none());

    if !*USE_COLOR {
        return line.to_string();
    }

    if line.contains("[ERROR]") {
        format!("\x1b[31m{line}\x1b[0m")
    } else if line.contains("[WARN]") {
        format!("\x1b[38;5;208m{line}\x1b[0m")
    } else if line.contains("[VERBOSE]") {
        format!("\x1b[2m{line}\x1b[0m")
    } else if line.contains("[SCRIPT]") || line.contains("[PLUGIN]") {
        format!("\x1b[32m{line}\x1b[0m")
    } else {
        line.to_string()
    }
}

fn decode_ansi(bytes: &[u8]) -> anyhow::Result<String> {
    let len = i32::try_from(bytes.len())?;
    let size = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr().cast(), len, std::ptr::null_mut(), 0) };
    if size == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mut wide = vec![0; usize::try_from(size)?];
    let size = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr().cast(), len, wide.as_mut_ptr(), size) };
    if size == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    wide.truncate(usize::try_from(size)?);
    Ok(String::from_utf16_lossy(&wide))
}

fn query_process_path(process: HANDLE) -> Option<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let mut capacity = 260_u32;
    let mut path_buf = vec![0_u16; capacity as usize];

    loop {
        let mut size = capacity;
        if unsafe { QueryFullProcessImageNameW(process, 0, path_buf.as_mut_ptr(), &raw mut size) } != 0 {
            return Some(std::path::PathBuf::from(OsString::from_wide(
                &path_buf[..size as usize],
            )));
        }

        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || capacity >= 32768 {
            return None;
        }

        capacity = capacity.saturating_mul(2).min(32768);
        path_buf.resize(capacity as usize, 0);
    }
}

struct UniqueHandle(HANDLE);

impl UniqueHandle {
    fn is_valid(&self) -> bool {
        !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for UniqueHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

type BOOL = i32;
type BYTE = u8;
type DWORD = u32;
type HANDLE = *mut core::ffi::c_void;
type HWND = HANDLE;
type LPARAM = isize;
type LPBYTE = *mut BYTE;
type LPCCH = *const i8;
type LPCVOID = *const core::ffi::c_void;
type LPCWSTR = *const u16;
type LPDWORD = *mut DWORD;
type LPOVERLAPPED = *mut core::ffi::c_void;
type LPSECURITY_ATTRIBUTES = *mut core::ffi::c_void;
type LPSTR = *mut i8;
type LPTHREAD_START_ROUTINE = Option<unsafe extern "system" fn(LPVOID) -> DWORD>;
type LPVOID = *mut core::ffi::c_void;
type LPWSTR = *mut u16;
type PDWORD = *mut DWORD;
type SHORT = i16;
type SIZE_T = usize;
type UINT = u32;
type WNDENUMPROC = Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>;
type WORD = u16;
type WPARAM = usize;

const INVALID_HANDLE_VALUE: HANDLE = (-1isize) as HANDLE;
const MAX_PATH: usize = 260;
const EXCEPTION_MAXIMUM_PARAMETERS: usize = 15;
const TH32CS_SNAPPROCESS: DWORD = 0x0000_0002;
const PROCESS_QUERY_LIMITED_INFORMATION: DWORD = 0x1000;
const PROCESS_TERMINATE: DWORD = 0x0001;
const PROCESS_SYNCHRONIZE: DWORD = 0x0010_0000;
const WAIT_OBJECT_0: DWORD = 0;
const DEBUG_ONLY_THIS_PROCESS: DWORD = 0x0000_0002;
const EXCEPTION_DEBUG_EVENT: DWORD = 1;
const CREATE_PROCESS_DEBUG_EVENT: DWORD = 3;
const EXIT_PROCESS_DEBUG_EVENT: DWORD = 5;
const LOAD_DLL_DEBUG_EVENT: DWORD = 6;
const OUTPUT_DEBUG_STRING_EVENT: DWORD = 8;
const STATUS_BREAKPOINT: DWORD = 0x8000_0003;
const DBG_CONTINUE: DWORD = 0x0001_0002;
const DBG_EXCEPTION_NOT_HANDLED: DWORD = 0x8001_0001;
const WM_CLOSE: UINT = 0x0010;
const GW_OWNER: UINT = 4;
const SW_SHOWNOACTIVATE: WORD = 4;
const STARTF_USESHOWWINDOW: DWORD = 0x0000_0001;
const INFINITE: DWORD = u32::MAX;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const CP_ACP: UINT = 0;
const ERROR_INSUFFICIENT_BUFFER: DWORD = 122;
const STD_INPUT_HANDLE: DWORD = (-10_i32).cast_unsigned();
const STD_ERROR_HANDLE: DWORD = (-12_i32).cast_unsigned();
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: DWORD = 0x0004;
const KEY_EVENT: WORD = 0x0001;
const MONITOR_TIMEOUT: DWORD = 50;
const INPUT_RECORDS: usize = 64;
const INPUT_BYTES: usize = 256;
const VK_R: WORD = 0x0052;
const VK_RETURN: WORD = 0x000D;
const VK_SHIFT: WORD = 0x0010;
const VK_CONTROL: WORD = 0x0011;
const VK_MENU: WORD = 0x0012;
const LEFT_CTRL_PRESSED: DWORD = 0x0008;
const RIGHT_CTRL_PRESSED: DWORD = 0x0004;

#[repr(C)]
struct PROCESSENTRY32W {
    dwSize: DWORD,
    cntUsage: DWORD,
    th32ProcessID: DWORD,
    th32DefaultHeapID: usize,
    th32ModuleID: DWORD,
    cntThreads: DWORD,
    th32ParentProcessID: DWORD,
    pcPriClassBase: i32,
    dwFlags: DWORD,
    szExeFile: [u16; MAX_PATH],
}

#[repr(C)]
struct STARTUPINFOW {
    cb: DWORD,
    lpReserved: LPWSTR,
    lpDesktop: LPWSTR,
    lpTitle: LPWSTR,
    dwX: DWORD,
    dwY: DWORD,
    dwXSize: DWORD,
    dwYSize: DWORD,
    dwXCountChars: DWORD,
    dwYCountChars: DWORD,
    dwFillAttribute: DWORD,
    dwFlags: DWORD,
    wShowWindow: WORD,
    cbReserved2: WORD,
    lpReserved2: LPBYTE,
    hStdInput: HANDLE,
    hStdOutput: HANDLE,
    hStdError: HANDLE,
}

#[repr(C)]
struct PROCESS_INFORMATION {
    hProcess: HANDLE,
    hThread: HANDLE,
    dwProcessId: DWORD,
    dwThreadId: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct OUTPUT_DEBUG_STRING_INFO {
    lpDebugStringData: LPSTR,
    fUnicode: WORD,
    nDebugStringLength: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EXCEPTION_RECORD {
    ExceptionCode: DWORD,
    ExceptionFlags: DWORD,
    ExceptionRecord: *mut EXCEPTION_RECORD,
    ExceptionAddress: LPVOID,
    NumberParameters: DWORD,
    ExceptionInformation: [usize; EXCEPTION_MAXIMUM_PARAMETERS],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EXCEPTION_DEBUG_INFO {
    ExceptionRecord: EXCEPTION_RECORD,
    dwFirstChance: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CREATE_PROCESS_DEBUG_INFO {
    hFile: HANDLE,
    hProcess: HANDLE,
    hThread: HANDLE,
    lpBaseOfImage: LPVOID,
    dwDebugInfoFileOffset: DWORD,
    nDebugInfoSize: DWORD,
    lpThreadLocalBase: LPVOID,
    lpStartAddress: LPTHREAD_START_ROUTINE,
    lpImageName: LPVOID,
    fUnicode: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EXIT_PROCESS_DEBUG_INFO {
    dwExitCode: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LOAD_DLL_DEBUG_INFO {
    hFile: HANDLE,
    lpBaseOfDll: LPVOID,
    dwDebugInfoFileOffset: DWORD,
    nDebugInfoSize: DWORD,
    lpImageName: LPVOID,
    fUnicode: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
union DEBUG_EVENT_UNION {
    Exception: EXCEPTION_DEBUG_INFO,
    CreateProcessInfo: CREATE_PROCESS_DEBUG_INFO,
    ExitProcess: EXIT_PROCESS_DEBUG_INFO,
    LoadDll: LOAD_DLL_DEBUG_INFO,
    DebugString: OUTPUT_DEBUG_STRING_INFO,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DEBUG_EVENT {
    dwDebugEventCode: DWORD,
    dwProcessId: DWORD,
    dwThreadId: DWORD,
    u: DEBUG_EVENT_UNION,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KEY_EVENT_RECORD {
    bKeyDown: BOOL,
    wRepeatCount: WORD,
    wVirtualKeyCode: WORD,
    wVirtualScanCode: WORD,
    uChar: u16,
    dwControlKeyState: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MOUSE_EVENT_RECORD {
    dwMousePosition: DWORD,
    dwButtonState: DWORD,
    dwControlKeyState: DWORD,
    dwEventFlags: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WINDOW_BUFFER_SIZE_RECORD {
    dwSize: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MENU_EVENT_RECORD {
    dwCommandId: UINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FOCUS_EVENT_RECORD {
    bSetFocus: BOOL,
}

#[repr(C)]
#[derive(Clone, Copy)]
union INPUT_RECORD_EVENT {
    KeyEvent: KEY_EVENT_RECORD,
    MouseEvent: MOUSE_EVENT_RECORD,
    WindowBufferSizeEvent: WINDOW_BUFFER_SIZE_RECORD,
    MenuEvent: MENU_EVENT_RECORD,
    FocusEvent: FOCUS_EVENT_RECORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct INPUT_RECORD {
    EventType: WORD,
    Event: INPUT_RECORD_EVENT,
}

type LPDEBUG_EVENT = *mut DEBUG_EVENT;
type LPPROCESS_INFORMATION = *mut PROCESS_INFORMATION;
type LPPROCESSENTRY32W = *mut PROCESSENTRY32W;
type LPSTARTUPINFOW = *mut STARTUPINFOW;
type PINPUT_RECORD = *mut INPUT_RECORD;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct COORD {
    X: SHORT,
    Y: SHORT,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SMALL_RECT {
    Left: SHORT,
    Top: SHORT,
    Right: SHORT,
    Bottom: SHORT,
}

#[repr(C)]
struct CONSOLE_SCREEN_BUFFER_INFO {
    dwSize: COORD,
    dwCursorPosition: COORD,
    wAttributes: WORD,
    srWindow: SMALL_RECT,
    dwMaximumWindowSize: COORD,
}
type PCONSOLE_SCREEN_BUFFER_INFO = *mut CONSOLE_SCREEN_BUFFER_INFO;

unsafe extern "system" {
    fn CreateProcessW(
        lpApplicationName: LPCWSTR,
        lpCommandLine: LPWSTR,
        lpProcessAttributes: LPSECURITY_ATTRIBUTES,
        lpThreadAttributes: LPSECURITY_ATTRIBUTES,
        bInheritHandles: BOOL,
        dwCreationFlags: DWORD,
        lpEnvironment: LPVOID,
        lpCurrentDirectory: LPCWSTR,
        lpStartupInfo: LPSTARTUPINFOW,
        lpProcessInformation: LPPROCESS_INFORMATION,
    ) -> BOOL;
    fn CreateToolhelp32Snapshot(dwFlags: DWORD, th32ProcessID: DWORD) -> HANDLE;
    fn Process32FirstW(hSnapshot: HANDLE, lppe: LPPROCESSENTRY32W) -> BOOL;
    fn Process32NextW(hSnapshot: HANDLE, lppe: LPPROCESSENTRY32W) -> BOOL;
    fn OpenProcess(dwDesiredAccess: DWORD, bInheritHandle: BOOL, dwProcessId: DWORD) -> HANDLE;
    fn QueryFullProcessImageNameW(hProcess: HANDLE, dwFlags: DWORD, lpExeName: LPWSTR, lpdwSize: PDWORD) -> BOOL;
    fn TerminateProcess(hProcess: HANDLE, uExitCode: UINT) -> BOOL;
    fn GetExitCodeProcess(hProcess: HANDLE, lpExitCode: LPDWORD) -> BOOL;
    fn CloseHandle(hObject: HANDLE) -> BOOL;
    fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: DWORD) -> DWORD;
    fn EnumWindows(lpEnumFunc: WNDENUMPROC, lParam: LPARAM) -> BOOL;
    fn GetWindowThreadProcessId(hWnd: HWND, lpdwProcessId: LPDWORD) -> DWORD;
    fn GetWindow(hWnd: HWND, uCmd: UINT) -> HWND;
    fn IsWindowVisible(hWnd: HWND) -> BOOL;
    fn PostMessageW(hWnd: HWND, Msg: UINT, wParam: WPARAM, lParam: LPARAM) -> BOOL;
    fn GetStdHandle(nStdHandle: DWORD) -> HANDLE;
    fn PeekConsoleInputW(
        hConsoleInput: HANDLE,
        lpBuffer: PINPUT_RECORD,
        nLength: DWORD,
        lpNumberOfEventsRead: LPDWORD,
    ) -> BOOL;
    fn ReadConsoleInputW(
        hConsoleInput: HANDLE,
        lpBuffer: PINPUT_RECORD,
        nLength: DWORD,
        lpNumberOfEventsRead: LPDWORD,
    ) -> BOOL;
    fn PeekNamedPipe(
        hNamedPipe: HANDLE,
        lpBuffer: LPVOID,
        nBufferSize: DWORD,
        lpBytesRead: LPDWORD,
        lpTotalBytesAvail: LPDWORD,
        lpBytesLeftThisMessage: LPDWORD,
    ) -> BOOL;
    fn ReadFile(
        hFile: HANDLE,
        lpBuffer: LPVOID,
        nNumberOfBytesToRead: DWORD,
        lpNumberOfBytesRead: LPDWORD,
        lpOverlapped: LPOVERLAPPED,
    ) -> BOOL;
    fn WaitForDebugEventEx(lpDebugEvent: LPDEBUG_EVENT, dwMilliseconds: DWORD) -> BOOL;
    fn ContinueDebugEvent(dwProcessId: DWORD, dwThreadId: DWORD, dwContinueStatus: DWORD) -> BOOL;
    fn MultiByteToWideChar(
        CodePage: UINT,
        dwFlags: DWORD,
        lpMultiByteStr: LPCCH,
        cbMultiByte: i32,
        lpWideCharStr: LPWSTR,
        cchWideChar: i32,
    ) -> i32;
    fn ReadProcessMemory(
        hProcess: HANDLE,
        lpBaseAddress: LPCVOID,
        lpBuffer: LPVOID,
        nSize: SIZE_T,
        lpNumberOfBytesRead: *mut SIZE_T,
    ) -> BOOL;
    fn DebugActiveProcessStop(dwProcessId: DWORD) -> BOOL;
    fn GetLastError() -> DWORD;
    fn GetConsoleMode(hConsoleHandle: HANDLE, lpMode: LPDWORD) -> BOOL;
    fn SetConsoleMode(hConsoleHandle: HANDLE, dwMode: DWORD) -> BOOL;
    fn GetConsoleScreenBufferInfo(
        hConsoleOutput: HANDLE,
        lpConsoleScreenBufferInfo: PCONSOLE_SCREEN_BUFFER_INFO,
    ) -> BOOL;
}
