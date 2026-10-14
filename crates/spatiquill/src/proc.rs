//! What a tab is doing, for its name on the bar: the program in front, or, when that is the
//! shell, the folder it is in.

/// The process group in front on the terminal `master`.
fn foreground(master: i32) -> Option<i32> {
    let group = unsafe { libc::tcgetpgrp(master) };
    (group > 0).then_some(group)
}

#[cfg(target_os = "macos")]
fn name_of(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr() as *mut _, buffer.len() as u32) };
    if n <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buffer[..n as usize]).into_owned();
    path.rsplit('/').next().map(str::to_string)
}

#[cfg(not(target_os = "macos"))]
fn name_of(pid: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
}

#[cfg(target_os = "macos")]
fn folder_of(pid: i32) -> Option<std::path::PathBuf> {
    // proc_pidinfo(PROC_PIDVNODEPATHINFO): the current directory's vnode path comes first
    // after a vnode_info_path's 152-byte vnode_info.
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    let mut info = vec![0u8; 2352];
    let n = unsafe { libc::proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, info.as_mut_ptr() as *mut _, info.len() as i32) };
    if n <= 0 {
        return None;
    }
    let path = &info[152..152 + 1024];
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    Some(std::path::PathBuf::from(String::from_utf8_lossy(&path[..end]).into_owned()))
}

#[cfg(not(target_os = "macos"))]
fn folder_of(pid: i32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

const SHELLS: &[&str] = &["zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "nu", "login", "mksh"];

/// A tab's name: the program in front, or the shell's folder, the home folder as `~`.
pub fn describe(master: i32, shell: i32) -> String {
    let front = foreground(master).unwrap_or(shell);
    let name = name_of(front).unwrap_or_default();
    let name = name.trim_start_matches('-').to_string();
    if !name.is_empty() && !SHELLS.contains(&name.as_str()) {
        return name;
    }
    // The shell is in front: where it is. Its own folder, which `login` does not share.
    let folder = folder_of(front).or_else(|| folder_of(shell));
    let Some(folder) = folder else { return name };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if home.as_deref() == Some(folder.as_path()) {
        return "~".to_string();
    }
    folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.display().to_string())
}
