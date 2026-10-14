//! Which agent or terminal a command belongs to.
//!
//! An agent's shell tool starts a fresh shell for every command and may reset the working folder each time, so neither the
//! folder nor the shell identifies the agent. The process that started the shell does: the nearest ancestor that is not a
//! shell (`claude.exe`, the terminal program, an editor). `mysid set` pins are filed under it, so two agents that share a
//! folder never overwrite each other's choice, and one agent's choice survives the folder reset.

use std::collections::HashMap;

/// Shells and launchers that sit between a session and the command.
const SKIP: &[&str] = &["cmd", "bash", "sh", "zsh", "fish", "dash", "pwsh", "powershell", "conhost", "wsl", "env", "sudo", "mysid", "git-bash", "login", "su"];

/// pid -> (parent pid, lower-case process name without `.exe`).
#[cfg(windows)]
fn process_table() -> HashMap<u32, (u32, String)> {
    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        ppid: u32,
        pri: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    let mut table = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(2, 0); // TH32CS_SNAPPROCESS
        if snap == -1 || snap == 0 {
            return table;
        }
        let mut e: ProcessEntry32W = std::mem::zeroed();
        e.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            let name = String::from_utf16_lossy(&e.exe[..len]).to_lowercase();
            table.insert(e.pid, (e.ppid, name.trim_end_matches(".exe").to_string()));
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    table
}

#[cfg(not(windows))]
fn process_table() -> HashMap<u32, (u32, String)> {
    let mut table = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for entry in rd.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) else { continue };
            // "pid (comm) S ppid ...": the name is in parentheses and may itself contain spaces.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let name = stat[open + 1..close].to_lowercase();
            let ppid = stat[close + 1..].split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            table.insert(pid, (ppid, name));
        }
    }
    table
}

/// The session key (`claude:18892`) of this process, from the nearest ancestor that is not a shell.
pub fn current() -> Option<String> {
    let table = process_table();
    let mut pid = std::process::id();
    for _ in 0..16 {
        let (ppid, _) = table.get(&pid)?;
        let parent = *ppid;
        let (_, name) = table.get(&parent)?;
        if !SKIP.contains(&name.as_str()) {
            return Some(format!("{}:{}", name, parent));
        }
        pid = parent;
    }
    None
}

/// Whether the session named by `key` (`claude:18892`) is still running: a pin of a dead session is dropped.
pub fn alive(key: &str, table: &HashMap<u32, (u32, String)>) -> bool {
    let Some((name, pid)) = key.rsplit_once(':') else { return true };
    let Ok(pid) = pid.parse::<u32>() else { return true };
    table.get(&pid).map_or(false, |(_, n)| n == name)
}

/// The process table, for `alive`.
pub fn table() -> HashMap<u32, (u32, String)> {
    process_table()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_session_and_it_is_alive() {
        let key = current().expect("a test run has a parent process");
        assert!(key.contains(':'), "{}", key);
        assert!(alive(&key, &table()));
        assert!(!alive("nothing:4000000000", &table()));
    }
}
