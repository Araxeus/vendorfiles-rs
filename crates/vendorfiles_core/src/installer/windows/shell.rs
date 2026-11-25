//! Starting a program through the shell, and waiting for its exit code.
//!
//! `ShellExecuteExW` is the one launcher that honours an executable's manifest. An installer that
//! requires admin gets the UAC prompt, where `CreateProcess` fails with error 740. It is also the
//! only way to ask for elevation outright, which `msiexec` needs for a per-machine install.
//!
//! This is the only module in the crate allowed `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// The Win32 error `ShellExecuteExW` reports when the UAC prompt is declined.
pub const ERROR_CANCELLED: i32 = 1223;

/// Runs `program` with `params` and waits for it to exit, returning its exit code.
///
/// `params` is handed over as one command line, exactly as written. `elevate` asks for admin
/// rights outright; otherwise the program's own manifest decides.
pub fn run(program: &OsStr, params: &str, elevate: bool) -> io::Result<u32> {
    let wide = |text: &OsStr| text.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
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
        info.nShow = SW_SHOWNORMAL;

        if ShellExecuteExW(&raw mut info) == 0 {
            return Err(io::Error::from_raw_os_error(
                i32::try_from(GetLastError()).unwrap_or(i32::MAX),
            ));
        }
        if info.hProcess.is_null() {
            return Err(io::Error::other("the shell started no process to wait on"));
        }

        WaitForSingleObject(info.hProcess, INFINITE);
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
