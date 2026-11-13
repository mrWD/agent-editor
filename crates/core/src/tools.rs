//! Programs of this computer the core runs for a moment and waits for: a
//! browser that takes one screenshot, `xcrun`, `adb`.
//!
//! Each run has a time limit. When it is over, one way or the other, the whole
//! family of processes the program started is stopped, so a browser's helpers
//! or a script's children never outlive the question they were started for.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// No program run here prints more than a picture of a screen. More than this is cut off.
const MAX_OUTPUT: usize = 48 * 1024 * 1024;
/// What a program complained about is only ever quoted; its end is enough.
const MAX_COMPLAINT: usize = 64 * 1024;
/// How long the output is waited for after the program has ended. A helper that
/// left the family (a daemon) may hold the output open for as long as it lives.
const OUTPUT_PATIENCE: Duration = Duration::from_millis(1500);

/// Where the programs the Preview runs are. `None` means: look for it on this computer.
///
/// Tests name stand-ins here, and so do `AE_BROWSER`, `AE_XCRUN` and `AE_ADB`.
/// A program that is named is the only one used, even when it is not there:
/// a test must never fall through to the real Chrome, Xcode or Android tools.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub browser: Option<PathBuf>,
    pub xcrun: Option<PathBuf>,
    pub adb: Option<PathBuf>,
    /// How long any one run may take, instead of each kind's own limit. For tests of a program that hangs.
    pub patience: Option<Duration>,
}

impl Tools {
    /// What the environment names. Read once, when the core starts.
    pub fn from_env() -> Self {
        let named = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
        Self { browser: named("AE_BROWSER"), xcrun: named("AE_XCRUN"), adb: named("AE_ADB"), patience: None }
    }

    /// The limit of one run: the kind's own, unless a test set another.
    pub(crate) fn limit(&self, usual: Duration) -> Duration {
        self.patience.unwrap_or(usual)
    }
}

/// How a run ended.
#[derive(Debug)]
pub(crate) struct Ran {
    /// The exit code. `None` when the program was stopped, by the time limit or by a signal.
    pub code: Option<i32>,
    /// The time limit ran out and the program was stopped.
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// The last line of a stream that says anything, short enough to quote.
    fn last_line(bytes: &[u8]) -> Option<String> {
        let text = String::from_utf8_lossy(bytes);
        let line = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;
        Some(line.chars().take(300).collect())
    }

    fn said_nothing(&self) -> String {
        match self.code {
            Some(code) => format!("it ended with code {code} and said nothing"),
            None => "it said nothing".into(),
        }
    }

    /// The last thing the program said, on either stream, as one short line to quote.
    pub fn last_words(&self) -> String {
        Self::last_line(&self.stderr).or_else(|| Self::last_line(&self.stdout)).unwrap_or_else(|| self.said_nothing())
    }

    /// The last thing it complained about. For a program whose output is a picture, which is never quoted.
    pub fn complaint(&self) -> String {
        Self::last_line(&self.stderr).unwrap_or_else(|| self.said_nothing())
    }
}

/// Runs a program and waits for it, for `limit` at most.
pub(crate) async fn run(program: &Path, args: Vec<OsString>, limit: Duration) -> Result<Ran> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // What the core was configured with is the core's own business.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("AE_") {
            command.env_remove(&name);
        }
    }
    #[cfg(unix)]
    {
        // The head of a group of its own: the group is how its whole family is stopped.
        command.process_group(0);
        // SAFETY: the function only calls `fcntl`, which is safe between `fork` and `exec`.
        unsafe { command.pre_exec(crate::acp::leave_others_streams) };
    }
    // A program without a window of its own would otherwise flash a console over the desktop app.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let mut child = command.spawn().with_context(|| format!("Could not start {}", program.display()))?;
    let pid = child.id();
    let (stdout, reading_out) = gather(child.stdout.take(), MAX_OUTPUT);
    let (stderr, reading_err) = gather(child.stderr.take(), MAX_COMPLAINT);

    let timed_out = tokio::time::timeout(limit, ended(&mut child, pid)).await.is_err();
    // On Unix the program has not been collected yet, so its number still names its own family:
    // whatever is left of the family is stopped, whether the time ran out or the program ended by itself.
    // On Windows a family is found through its living head, so it is stopped only when that still runs.
    if let Some(pid) = pid
        && (cfg!(unix) || timed_out)
    {
        crate::acp::kill_family(pid);
    }
    if timed_out {
        let _ = child.start_kill();
    }
    let status = child.wait().await.with_context(|| format!("Could not wait for {}", program.display()))?;
    let (stop_out, stop_err) = (reading_out.abort_handle(), reading_err.abort_handle());
    let both = async { tokio::join!(reading_out, reading_err) };
    if tokio::time::timeout(OUTPUT_PATIENCE, both).await.is_err() {
        stop_out.abort();
        stop_err.abort();
    }
    let taken = |kept: Arc<Mutex<Vec<u8>>>| std::mem::take(&mut *kept.lock().unwrap());
    Ok(Ran {
        code: if timed_out { None } else { status.code() },
        timed_out,
        stdout: taken(stdout),
        stderr: taken(stderr),
    })
}

/// Reads a stream of a program into memory as it comes, up to `most` bytes; the rest is thrown away,
/// so a program that prints without end still ends. What was read can be taken at any moment.
fn gather<R: AsyncRead + Unpin + Send + 'static>(
    stream: Option<R>,
    most: usize,
) -> (Arc<Mutex<Vec<u8>>>, JoinHandle<()>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let into = kept.clone();
    let reading = tokio::spawn(async move {
        let Some(mut stream) = stream else { return };
        let mut buffer = vec![0u8; 64 * 1024];
        while let Ok(n) = stream.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            let mut kept = into.lock().unwrap();
            let room = most.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..n.min(room)]);
        }
    });
    (kept, reading)
}

/// Waits until the program has ended, without collecting it. Until it is collected the system
/// keeps its number for it, so "the group of that number" cannot have become somebody else's.
#[cfg(unix)]
async fn ended(child: &mut Child, pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let seen = tokio::task::spawn_blocking(move || {
        loop {
            // SAFETY: `waitid` only writes into the structure it is given; WNOWAIT leaves the program to be collected later.
            let answer = unsafe {
                let mut info: libc::siginfo_t = std::mem::zeroed();
                libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT)
            };
            if answer == 0 {
                return true;
            }
            // A signal that arrived meanwhile only interrupts the wait.
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return false;
            }
        }
    })
    .await;
    // The system would not say (a process that set itself up never to hear of its children): wait the plain way.
    if !matches!(seen, Ok(true)) {
        let _ = child.wait().await;
    }
}

#[cfg(not(unix))]
async fn ended(child: &mut Child, _pid: Option<u32>) {
    let _ = child.wait().await;
}

/// The first of `places` that is a file.
pub(crate) fn first_file(places: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    places.into_iter().find(|place| place.is_file())
}

/// Words for the command line of a program, from plain text.
pub(crate) fn words<const N: usize>(list: [&str; N]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> (PathBuf, Vec<OsString>) {
        (PathBuf::from("/bin/sh"), words(["-c", script]))
    }

    #[tokio::test]
    async fn a_program_is_run_and_what_it_printed_is_kept_apart() {
        let (program, args) = sh("printf 'out'; printf 'first\\n  last words  \\n\\n' >&2; exit 3");
        let ran = run(&program, args, Duration::from_secs(10)).await.unwrap();
        assert_eq!((ran.code, ran.timed_out, ran.ok()), (Some(3), false, false));
        assert_eq!(ran.text(), "out");
        assert_eq!(ran.last_words(), "last words");

        let (program, args) = sh("printf 'only this'");
        let quiet = run(&program, args, Duration::from_secs(10)).await.unwrap();
        assert!(quiet.ok());
        assert_eq!(quiet.last_words(), "only this", "with nothing on the error stream the output is quoted");
        let (program, args) = sh("exit 7");
        assert_eq!(
            run(&program, args, Duration::from_secs(10)).await.unwrap().last_words(),
            "it ended with code 7 and said nothing"
        );
        assert!(run(Path::new("/nonexistent/ae-test/tool"), vec![], Duration::from_secs(1)).await.is_err());
    }

    fn lives(pid: i32) -> bool {
        // SAFETY: signal 0 sends nothing; it only asks whether the process is there.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    async fn gone(pid: i32) -> bool {
        for _ in 0..100 {
            if !lives(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_program_that_takes_too_long_is_stopped_with_everything_it_started() {
        // A child of the program that would sleep for ten minutes, and the program itself waiting for it.
        let (program, args) = sh("sleep 600 & echo $!; wait");
        let started = std::time::Instant::now();
        let ran = run(&program, args, Duration::from_millis(400)).await.unwrap();
        assert!(ran.timed_out && !ran.ok() && ran.code.is_none());
        assert!(started.elapsed() < Duration::from_secs(8), "the limit is kept: {:?}", started.elapsed());
        let child: i32 = ran.text().trim().parse().expect("the program said its child's number");
        assert!(gone(child).await, "the child of a stopped program is stopped too");
    }

    #[tokio::test]
    async fn what_a_program_leaves_behind_is_stopped_when_it_ends_by_itself() {
        // The program ends at once and leaves a child that holds on to nothing of ours.
        let (program, args) = sh("sleep 600 >/dev/null 2>&1 & echo $!");
        let ran = run(&program, args, Duration::from_secs(10)).await.unwrap();
        assert!(ran.ok());
        let child: i32 = ran.text().trim().parse().unwrap();
        assert!(gone(child).await, "nothing of the family outlives the run");
    }

    #[tokio::test]
    async fn output_held_open_by_someone_who_left_the_family_does_not_hold_the_answer() {
        // A helper that moves into a session of its own, as a daemon does, and keeps the output open for
        // eight seconds. The program waits until the helper has moved, says "done" and ends.
        let script = r#"perl -e 'use POSIX; pipe(R, W); if (fork) { close W; <R>; exit } POSIX::setsid(); close R; syswrite(W, "x"); close W; sleep 8'; printf 'done'"#;
        let (program, args) = sh(script);
        let started = std::time::Instant::now();
        let ran = run(&program, args, Duration::from_secs(10)).await.unwrap();
        assert!(ran.ok());
        assert_eq!(ran.text(), "done");
        let took = started.elapsed();
        assert!(took >= OUTPUT_PATIENCE, "the helper really held the output open: {took:?}");
        assert!(took < Duration::from_secs(6), "and the answer did not wait for the helper to end: {took:?}");
    }
}
