//! Starting programs through the shell, closing running ones, and reading file versions.
//!
//! `ShellExecuteExW` is the one launcher that honours an executable's manifest. An installer that
//! requires admin gets the UAC prompt, where `CreateProcess` fails with error 740. It is also the
//! only way to ask for elevation outright, which `msiexec` needs for a per-machine install.
//!
//! Running programs close through the Restart Manager, which asks them to quit the way Windows
//! does at shutdown. Apps that hide in the tray on an ordinary close still quit for that.
//!
//! This is the only module in the crate allowed `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GetLastError, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_UNIQUE_PROCESS, RmEndSession, RmRegisterResources, RmShutdown,
    RmStartSession,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SHOW_WINDOW_CMD, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL,
};

/// How a launched program's window opens. Tests open it minimized and unfocused, so a test run
/// never takes focus from the desktop.
const SHOW: SHOW_WINDOW_CMD = if cfg!(test) {
    SW_SHOWMINNOACTIVE
} else {
    SW_SHOWNORMAL
};

/// The Win32 error `ShellExecuteExW` reports when the UAC prompt is declined.
pub const ERROR_CANCELLED: i32 = 1223;

/// Runs `program` with `params` and waits for it to exit, returning its exit code.
///
/// `params` is handed over as one command line, exactly as written. `elevate` asks for admin
/// rights outright; otherwise the program's own manifest decides. `waiting` is called about
/// once a second while the program runs.
pub fn run(
    program: &OsStr,
    params: &str,
    elevate: bool,
    mut waiting: impl FnMut(),
) -> io::Result<u32> {
    let verb = wide(OsStr::new(if elevate { "runas" } else { "open" }));
    let file = wide(program);
    let params = wide(OsStr::new(params));

    // SAFETY: every pointer handed to the shell points into a NUL-terminated buffer that lives
    // until this block ends. The structure is zeroed, which is its documented empty state, and
    // `hProcess` is closed exactly once, after the last call that reads it.
    unsafe {
        // The shell may hand the launch to a COM handler. The result is ignored because a thread
        // that is already initialised reports that and is still fine to use.
        let _ = CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).cast_unsigned(),
        );

        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(u32::MAX);
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SHOW;

        if ShellExecuteExW(&raw mut info) == 0 {
            return Err(io::Error::from_raw_os_error(
                i32::try_from(GetLastError()).unwrap_or(i32::MAX),
            ));
        }
        if info.hProcess.is_null() {
            return Err(io::Error::other("the shell started no process to wait on"));
        }

        while WaitForSingleObject(info.hProcess, 1000) == WAIT_TIMEOUT {
            waiting();
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(info.hProcess, &raw mut code);
        let error = io::Error::last_os_error();
        CloseHandle(info.hProcess);
        if read == 0 {
            return Err(error);
        }
        Ok(code)
    }
}

/// Starts `program` from its own folder and returns without waiting for it.
pub fn start(program: &Path) -> io::Result<()> {
    let file = wide(program.as_os_str());
    let folder = wide(program.parent().unwrap_or(program).as_os_str());
    let verb = wide(OsStr::new("open"));

    // SAFETY: every pointer handed to the shell points into a NUL-terminated buffer that lives
    // until this block ends. No process handle is asked for, so there is none to close.
    unsafe {
        let _ = CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).cast_unsigned(),
        );
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(u32::MAX);
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpDirectory = folder.as_ptr();
        info.nShow = SHOW;
        if ShellExecuteExW(&raw mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Asks the processes to quit, as Windows does at shutdown, and waits until they have.
///
/// A process that refuses, for example to ask about unsaved work, makes this fail.
pub fn close(pids: &[u32]) -> io::Result<()> {
    let processes: Vec<RM_UNIQUE_PROCESS> = pids
        .iter()
        .filter_map(|&pid| {
            start_time(pid).map(|time| RM_UNIQUE_PROCESS {
                dwProcessId: pid,
                ProcessStartTime: time,
            })
        })
        .collect();
    if processes.is_empty() {
        return Ok(());
    }
    let count = u32::try_from(processes.len()).unwrap_or(u32::MAX);
    let check = |code: u32| match code {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(
            i32::try_from(code).unwrap_or(i32::MAX),
        )),
    };

    // SAFETY: the session key buffer has the documented length plus the terminator, the process
    // list outlives the calls that read it, and the session is ended exactly once.
    unsafe {
        let mut session = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        check(RmStartSession(&raw mut session, 0, key.as_mut_ptr()))?;
        let result = check(RmRegisterResources(
            session,
            0,
            std::ptr::null(),
            count,
            processes.as_ptr(),
            0,
            std::ptr::null(),
        ))
        .and_then(|()| check(RmShutdown(session, 0, None)));
        RmEndSession(session);
        result
    }
}

/// When a process started, which tells it apart from a later one given the same id.
fn start_time(pid: u32) -> Option<FILETIME> {
    // SAFETY: the process handle is checked before use and closed once. The four times are
    // plain out-parameters.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let read = GetProcessTimes(
            process,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        );
        CloseHandle(process);
        (read != 0).then_some(created)
    }
}

/// The product version in an executable's version resource, as its four numbers.
#[must_use]
pub fn file_version(path: &Path) -> Option<[u16; 4]> {
    let file = wide(path.as_os_str());
    let root = wide(OsStr::new("\\"));

    // SAFETY: the version block is sized by the call that reports its size, and the fixed info
    // pointer is checked for null and length before it is read. The block is a byte buffer, so
    // the read makes no assumption about alignment.
    unsafe {
        let size = GetFileVersionInfoSizeW(file.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; usize::try_from(size).ok()?];
        if GetFileVersionInfoW(file.as_ptr(), 0, size, block.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut info = std::ptr::null_mut();
        let mut length = 0u32;
        let found = VerQueryValueW(
            block.as_ptr().cast(),
            root.as_ptr(),
            &raw mut info,
            &raw mut length,
        );
        if found == 0
            || info.is_null()
            || usize::try_from(length).ok()? < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let fixed = std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>());
        let high = |word: u32| u16::try_from(word >> 16).unwrap_or(u16::MAX);
        let low = |word: u32| u16::try_from(word & 0xFFFF).unwrap_or(u16::MAX);
        Some([
            high(fixed.dwProductVersionMS),
            low(fixed.dwProductVersionMS),
            high(fixed.dwProductVersionLS),
            low(fixed.dwProductVersionLS),
        ])
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// Every running process, as `(id, parent id)`.
#[must_use]
pub fn processes() -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked before use and closed once. The entry is sized as
    // the API requires before the first call.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
            ..PROCESSENTRY32W::default()
        };
        let mut more = Process32FirstW(snapshot, &raw mut entry) != 0;
        while more {
            found.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &raw mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found
}

/// The executable a running process was started from, when this process may ask.
#[must_use]
pub fn image_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: the process handle is checked before use and closed once. The buffer's length is
    // passed in and the written length read back, so the slice never runs past it.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &raw mut length,
        );
        CloseHandle(process);
        (read != 0).then(|| {
            let length = usize::try_from(length).unwrap_or(0).min(buffer.len());
            PathBuf::from(OsString::from_wide(&buffer[..length]))
        })
    }
}

/// Quotes one argument for a Windows command line, the way `CommandLineToArgvW` reads it back.
#[must_use]
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            c => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn only_what_needs_quoting_is_quoted() {
        assert_eq!(quote(r"C:\setup.exe"), r"C:\setup.exe");
        assert_eq!(
            quote(r"C:\Users\A B\setup.msi"),
            r#""C:\Users\A B\setup.msi""#
        );
        assert_eq!(quote(r"C:\A B\"), r#""C:\A B\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(""), r#""""#);
    }
}
