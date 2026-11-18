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

/// The folder of what is in front on the terminal, or of the shell.
pub fn folder(master: i32, shell: i32) -> Option<std::path::PathBuf> {
    foreground(master).and_then(folder_of).or_else(|| folder_of(shell))
}

/// A tab's name: the program in front, or the shell's folder, the home folder as `~`.
pub fn describe(master: i32, shell: i32) -> String {
    let front = foreground(master).unwrap_or(shell);
    let name = name_of(front).unwrap_or_default();
    let name = name.trim_start_matches('-').to_string();
    if !name.is_empty() && !SHELLS.contains(&name.as_str()) {
        // tmux names its client "tmux: client (/tmp/...)": the tab is named tmux.
        return known(&name, NOT_ASKED_FOR).unwrap_or(&name).to_string();
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

/// The processes whose parent is `pid`.
#[cfg(target_os = "macos")]
fn children(pid: i32) -> Vec<i32> {
    let mut pids = vec![0i32; 256];
    let n = unsafe { libc::proc_listchildpids(pid, pids.as_mut_ptr() as *mut _, (pids.len() * std::mem::size_of::<i32>()) as i32) };
    pids.truncate(n.max(0) as usize);
    pids.retain(|&p| p > 0);
    pids
}

/// The processes whose parent is `pid`: each process's parent is the fourth field of its
/// `stat`, after the name in brackets (which can hold anything, brackets too).
#[cfg(not(target_os = "macos"))]
fn children(pid: i32) -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&child| {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else { return false };
            let after = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
            after.split_whitespace().nth(1).and_then(|p| p.parse::<i32>().ok()) == Some(pid)
        })
        .collect()
}

/// What runs under a shell and is not a reason to ask before closing it: a multiplexer's
/// client, whose session goes on without it (as Terminal on a Mac has it), and a prompt's own
/// helper.
const NOT_ASKED_FOR: &[&str] = &["tmux", "screen", "gitstatusd"];

/// Which of `names` a process's name is: the name itself, or one it starts with and goes on
/// from after something that is not a letter or digit -- tmux's client calls itself "tmux:
/// client (/tmp/...)", and the kernel's 15 bytes make gitstatusd-linux-x86_64
/// "gitstatusd-linu".
fn known<'a>(name: &str, names: &[&'a str]) -> Option<&'a str> {
    names.iter().copied().find(|n| name.strip_prefix(n).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric())))
}

/// The programs running in a tab beyond its idle shell, by name -- what closing the tab
/// would end. `tab` is the tab's own process (`spatiquill --tab`), whose child is the shell,
/// or on a Mac `login` and the shell under it. Empty when the shell waits at its prompt.
pub fn running(tab: i32) -> Vec<String> {
    let mut names = Vec::new();
    for top in children(tab) {
        let mut shell = top;
        if name_of(shell).as_deref() == Some("login") {
            if let [only] = children(shell)[..] {
                shell = only;
            }
        }
        let name = name_of(shell).unwrap_or_default().trim_start_matches('-').to_string();
        // The shell replaced by a program (`exec`), or something that is no shell at all.
        if !name.is_empty() && name != "login" && !SHELLS.contains(&name.as_str()) {
            names.push(name);
            continue;
        }
        for child in children(shell) {
            let name = name_of(child).unwrap_or_default();
            if !name.is_empty() && known(&name, NOT_ASKED_FOR).is_none() {
                names.push(name);
            }
        }
    }
    names
}

/// Names as a sentence's list: "sleep", "sleep and vim", "sleep, vim and 2 more".
pub fn listed(names: &[String]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for n in names {
        if !unique.contains(&n.as_str()) {
            unique.push(n);
        }
    }
    match unique.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_runs_under_a_shell_is_found() {
        // A shell of our own, at its prompt, and then with a program running in it.
        let me = std::process::id() as i32;
        let mut idle = std::process::Command::new("sh").arg("-c").arg("read x").stdin(std::process::Stdio::piped()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(children(me).contains(&(idle.id() as i32)));
        assert!(children(idle.id() as i32).is_empty());
        let _ = idle.kill();
        let _ = idle.wait();
        let mut busy = std::process::Command::new("sh").arg("-c").arg("sleep 30; true").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let pids = children(busy.id() as i32);
        let under: Vec<String> = pids.iter().copied().filter_map(name_of).collect();
        for pid in pids {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert_eq!(under, vec!["sleep".to_string()]);
        let _ = busy.kill();
        let _ = busy.wait();
    }

    #[test]
    fn helpers_are_known_by_the_names_they_take() {
        assert_eq!(known("tmux", NOT_ASKED_FOR), Some("tmux"));
        assert_eq!(known("tmux: client (/", NOT_ASKED_FOR), Some("tmux"));
        assert_eq!(known("gitstatusd-linu", NOT_ASKED_FOR), Some("gitstatusd"));
        assert_eq!(known("screen", NOT_ASKED_FOR), Some("screen"));
        // Another program whose name starts the same is another program.
        assert_eq!(known("tmuxinator", NOT_ASKED_FOR), None);
        assert_eq!(known("screenfetch", NOT_ASKED_FOR), None);
        assert_eq!(known("vim", NOT_ASKED_FOR), None);
    }

    #[test]
    fn names_read_as_a_list() {
        let n = |v: &[&str]| listed(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["sleep"]), "sleep");
        assert_eq!(n(&["sleep", "sleep"]), "sleep");
        assert_eq!(n(&["sleep", "vim"]), "sleep and vim");
        assert_eq!(n(&["sleep", "vim", "top", "less"]), "sleep, vim and 2 more");
    }
}
