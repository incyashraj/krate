//! Ending a process tree on Windows without shelling out.
//!
//! `taskkill` did the job until it became a liability: it is an external
//! executable, so security software watches it, and on a locked-down machine
//! the person gets a system dialog about taskkill.exe from an app they only
//! asked to stop a build in (K-177). The API calls below do exactly what
//! `taskkill /PID x /T /F` did -- walk the process table, find the tree under
//! the pid, terminate children first -- inside our own process, where there
//! is nothing for a watchdog to flag and no console to flash.

#![cfg(windows)]

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

/// Terminate `pid` and every descendant, children before parents.
///
/// Best effort by design: a process that already exited, or one we may not
/// touch, is skipped rather than reported -- the callers used `taskkill`'s
/// exit status for nothing, and stopping a build must never grow its own
/// error dialog.
pub fn kill_process_tree(pid: u32) {
    // One snapshot of the whole process table, walked once into
    // (pid, parent) pairs. The tree is computed from the snapshot, so a
    // process spawned after this line survives -- the same race taskkill has.
    let mut table = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot as isize == -1 {
            return;
        }
        let mut entry: PROCESSENTRY32 = core::mem::zeroed();
        entry.dwSize = core::mem::size_of::<PROCESSENTRY32>() as u32;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                table.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }

    // Depth-first: collect the tree, then terminate leaves upward so a
    // parent cannot respawn a child we already ended.
    let mut doomed = vec![pid];
    let mut index = 0;
    while index < doomed.len() {
        let parent = doomed[index];
        for (child, child_parent) in &table {
            if *child_parent == parent && !doomed.contains(child) {
                doomed.push(*child);
            }
        }
        index += 1;
    }
    for target in doomed.iter().rev() {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, *target);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}
