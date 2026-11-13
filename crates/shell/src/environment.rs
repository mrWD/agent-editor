//! The PATH the user's own shell has.
//!
//! An app opened from the Dock or a launcher is given the system's bare PATH.
//! The folders a shell adds in its start files, where `npx`, `gh`, `glab` and
//! `docker` usually live, are missing from it. The core would then find none of
//! them and call every real agent "not installed". So before the core starts,
//! the app asks the user's shell for its PATH and takes it.
//!
//! Windows has no such gap: there every program gets the user's full PATH.

/// Takes over the PATH of the user's shell, unless the app was started from a
/// terminal and has that PATH already.
///
/// Must be called first thing in `main`, while this is the only thread.
pub fn adopt_shell_path() {
    #[cfg(unix)]
    unix::adopt();
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::io::{IsTerminal, Read};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Printed around the PATH, to find it between whatever the shell's start files print.
    const BEFORE: &str = "__AE_PATH_STARTS__";
    const AFTER: &str = "__AE_PATH_ENDS__";

    /// Start files are usually done in well under a second. A shell that takes
    /// longer than this is stuck, and the app must open anyway.
    const PATIENCE: Duration = Duration::from_secs(5);

    pub fn adopt() {
        // Started from a terminal: the environment is the shell's own already.
        if std::io::stderr().is_terminal() || std::io::stdin().is_terminal() {
            return;
        }
        let Some(shell) = std::env::var_os("SHELL").map(PathBuf::from).filter(|shell| shell.is_absolute()) else {
            return;
        };
        let Some(from_shell) = path_of_shell(&shell, PATIENCE) else {
            tracing::warn!(
                "Could not ask {} for its PATH. Agents installed there may show as missing.",
                shell.display()
            );
            return;
        };
        let merged = merge(&from_shell, std::env::var_os("PATH").as_deref());
        // SAFETY: called at the very start of `main`. The only other thread this process
        // has had is the one that read the shell's answer, and it never touches the environment.
        unsafe { std::env::set_var("PATH", &merged) };
        tracing::info!("Programs are looked for where {} looks for them", shell.display());
    }

    /// Asks a shell, started the way a terminal starts it, what its PATH is.
    /// `None` when it does not answer within `patience`.
    pub fn path_of_shell(shell: &Path, patience: Duration) -> Option<String> {
        // `-l` and `-i` make the shell read the same start files it reads in a
        // terminal; people set their PATH in either kind. `printenv` rather than
        // `echo $PATH`, because not every shell keeps its PATH as one string.
        let ask = format!("echo {BEFORE}; printenv PATH; echo {AFTER}");
        let mut child = Command::new(shell)
            .args(["-ilc", &ask])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut output = child.stdout.take()?;

        // Read on the side, and only up to the closing mark: a start file may
        // leave a helper running that keeps the output open for good.
        let (answer, answered) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let mut piece = [0u8; 4096];
            while let Ok(read) = output.read(&mut piece) {
                if read == 0 {
                    break;
                }
                text.push_str(&String::from_utf8_lossy(&piece[..read]));
                if text.contains(AFTER) {
                    break;
                }
            }
            let _ = answer.send(text);
        });

        let text = answered.recv_timeout(patience).ok();
        // Answered or not, the shell has nothing left to do.
        let _ = child.kill();
        let _ = child.wait();
        between_marks(&text?)
    }

    /// The PATH out of everything the shell printed.
    pub fn between_marks(text: &str) -> Option<String> {
        // The last opening mark: a start file that echoes commands would show the question itself first.
        let start = text.rfind(BEFORE)? + BEFORE.len();
        let end = start + text[start..].find(AFTER)?;
        let path = text[start..end].trim();
        (!path.is_empty()).then(|| path.to_string())
    }

    /// The shell's folders first, then whatever this process was given that the shell does not have.
    pub fn merge(from_shell: &str, given: Option<&std::ffi::OsStr>) -> OsString {
        let mut folders: Vec<PathBuf> = Vec::new();
        let given = given.map(|given| std::env::split_paths(given).collect::<Vec<_>>()).unwrap_or_default();
        for folder in std::env::split_paths(from_shell).chain(given) {
            if !folder.as_os_str().is_empty() && !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        // Joining fails only for a folder with a colon in its name; then the shell's own answer is used as it is.
        std::env::join_paths(folders).unwrap_or_else(|_| OsString::from(from_shell))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A stand-in for a shell: a script that gets the same arguments a real shell would.
        fn fake_shell(name: &str, script: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("ae-shell-env-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("shell");
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[test]
        fn the_path_is_found_between_whatever_the_start_files_print() {
            let printed = format!("Welcome back!\n{BEFORE}\n/opt/homebrew/bin:/usr/bin:/bin\n{AFTER}\nbye\n");
            assert_eq!(between_marks(&printed), Some("/opt/homebrew/bin:/usr/bin:/bin".into()));
            // A shell that echoes its commands shows the question first; the answer is after the last mark.
            let echoed = format!("echo {BEFORE}; printenv PATH; echo {AFTER}\n{BEFORE}\n/usr/bin\n{AFTER}\n");
            assert_eq!(between_marks(&echoed), Some("/usr/bin".into()));
        }

        #[test]
        fn no_marks_or_an_empty_path_is_no_answer() {
            assert_eq!(between_marks("command not found: printenv"), None);
            assert_eq!(between_marks(&format!("{BEFORE}\n\n{AFTER}")), None);
            assert_eq!(between_marks(&format!("{BEFORE}\n/usr/bin")), None);
        }

        #[test]
        fn the_shells_folders_come_first_and_nothing_is_listed_twice() {
            let merged = merge("/opt/homebrew/bin:/usr/bin:/bin", Some(std::ffi::OsStr::new("/usr/bin:/bin:/app/bin")));
            assert_eq!(merged, OsString::from("/opt/homebrew/bin:/usr/bin:/bin:/app/bin"));
            assert_eq!(merge("/usr/bin::/bin", None), OsString::from("/usr/bin:/bin"));
        }

        #[test]
        fn a_shell_is_asked_the_way_a_terminal_starts_it() {
            // The stand-in sets a PATH the way a start file would, prints a greeting, then runs what it was asked.
            let shell = fake_shell(
                "asks",
                r#"[ "$1" = "-ilc" ] || exit 1
echo "Last login: today"
PATH="/from/start/file:/usr/bin:/bin"; export PATH
eval "$2""#,
            );
            assert_eq!(path_of_shell(&shell, Duration::from_secs(10)), Some("/from/start/file:/usr/bin:/bin".into()));
        }

        #[test]
        fn a_helper_that_keeps_the_output_open_does_not_hold_the_answer_back() {
            let shell = fake_shell("helper", r#"sleep 8 & eval "$2""#);
            let started = std::time::Instant::now();
            assert!(path_of_shell(&shell, Duration::from_secs(10)).is_some());
            assert!(started.elapsed() < Duration::from_secs(4), "the answer did not wait for the helper");
        }

        #[test]
        fn a_shell_that_hangs_is_given_up_on() {
            // `exec`, so that giving up on the shell ends the sleep too and leaves nothing behind.
            let shell = fake_shell("hangs", "exec sleep 20");
            let started = std::time::Instant::now();
            assert_eq!(path_of_shell(&shell, Duration::from_millis(300)), None);
            assert!(started.elapsed() < Duration::from_secs(5), "the app opens anyway");
        }

        #[test]
        fn a_shell_that_is_not_there_is_no_answer() {
            assert_eq!(path_of_shell(Path::new("/nonexistent/ae-test/shell"), Duration::from_secs(1)), None);
        }
    }
}
