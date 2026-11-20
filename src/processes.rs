//! Lists running processes so the UI can warn before clearing a game's cache while it is
//! open (the driver keeps those files locked), and learn where executables live for icons.

use std::collections::HashMap;
use std::mem;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// Lowercased executable name (e.g. `eldenring.exe`) → full path when Windows allows us to
/// read it (protected and elevated processes return `None`).
pub type RunningProcesses = HashMap<String, Option<PathBuf>>;

pub fn running_processes() -> RunningProcesses {
    let mut processes = RunningProcesses::new();
    // SAFETY: standard Toolhelp32 enumeration; the snapshot handle is closed before
    // returning and `entry.dwSize` is set as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return processes;
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snapshot, &mut entry);
        while ok != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            let known_path = processes.get(&name).is_some_and(Option::is_some);
            if !known_path {
                processes.insert(name, process_path(entry.th32ProcessID));
            }
            ok = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    processes
}

fn process_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: the handle is closed on every path; the buffer length is passed in and out.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        (ok != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_own_process_and_path() {
        let processes = super::running_processes();
        let own = std::env::current_exe().unwrap();
        let name = own.file_name().unwrap().to_string_lossy().to_lowercase();
        let path = processes.get(&name).expect("own process listed");
        assert!(path.as_ref().is_some_and(|p| p.ends_with(own.file_name().unwrap())));
    }
}
