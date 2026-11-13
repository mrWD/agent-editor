//! Keeping the computer awake while an agent works.
//!
//! A computer left alone goes to sleep, and an agent in the middle of a turn
//! stops with it. With this setting on, the core runs a small helper of the
//! system for as long as any task's agent is working: `caffeinate -i` on
//! macOS, `systemd-inhibit` on Linux. The helper is the whole mechanism: while
//! it runs the system does not sleep from idleness, and when it ends the
//! system is as it was. Closing the lid still puts the computer to sleep.
//!
//! The helper ends when the last agent stops working, when the setting is
//! turned off and when the core stops. Should the core die without warning,
//! the helper notices that the core is gone and ends by itself.

use crate::agents::find_program;
use crate::tasks::Manager;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// "on" when the user wants the computer kept awake while an agent works.
const SETTING: &str = "keepAwake";

/// The helper and what decides whether it runs.
#[derive(Default)]
pub struct KeepAwake {
    /// The running helper, if any.
    helper: Mutex<Option<Child>>,
    /// The tasks whose agent is working right now.
    working: Mutex<HashSet<String>>,
    /// A command to run in place of the system's own helper. Tests give one.
    stand_in: Mutex<Option<Vec<String>>>,
    /// Set when the core stops: nothing is started after that.
    stopped: AtomicBool,
}

/// How this system is kept awake: the helper's name for the user, and the
/// command that keeps it awake for as long as it runs. `Err` says in words why
/// there is no way.
fn helper_of_this_system() -> Result<(&'static str, Vec<String>), String> {
    let core = std::process::id().to_string();
    let found = |program: &str| {
        find_program(program)
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| format!("The system's own helper for this, {program}, was not found on this computer."))
    };
    if cfg!(target_os = "macos") {
        // `-i`: no sleep from idleness. `-w`: it ends when the core does, however the core went.
        return Ok(("caffeinate -i", vec![found("caffeinate")?, "-i".into(), "-w".into(), core]));
    }
    if cfg!(target_os = "linux") {
        // The lock is held for as long as the command behind it runs, and that
        // command does nothing but wait for the core to be gone.
        let wait = "while kill -0 \"$0\" 2>/dev/null; do sleep 5; done";
        let command = [
            found("systemd-inhibit")?.as_str(),
            "--what=idle:sleep",
            "--who=Agent Editor",
            "--why=An agent is working",
            "sh",
            "-c",
            wait,
            core.as_str(),
        ]
        .map(str::to_string)
        .to_vec();
        return Ok(("systemd-inhibit", command));
    }
    if cfg!(windows) {
        return Err("Not available on Windows yet: the editor has no way to keep Windows awake.".into());
    }
    Err("Not available on this system yet.".into())
}

fn start(command: &[String]) -> std::io::Result<Child> {
    let mut helper = Command::new(&command[0]);
    helper.args(&command[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A group of its own, so that stopping the helper stops what it started too.
        helper.process_group(0);
        // It lives long: it must not hold a stream some other command of the core is waiting on.
        // SAFETY: the function only calls `fcntl`, which is safe between `fork` and `exec`.
        unsafe { helper.pre_exec(crate::acp::leave_others_streams) };
    }
    helper.spawn()
}

fn end(mut helper: Child) {
    #[cfg(unix)]
    crate::acp::kill_family(helper.id());
    let _ = helper.kill();
    let _ = helper.wait();
}

/// True when there is a helper and it still runs. One that ended by itself is as good as none.
fn still_runs(helper: &mut Option<Child>) -> bool {
    if helper.as_mut().is_some_and(|child| child.try_wait().ok().flatten().is_some()) {
        *helper = None;
    }
    helper.is_some()
}

impl KeepAwake {
    /// True while the helper runs.
    fn active(&self) -> bool {
        still_runs(&mut self.helper.lock().unwrap())
    }

    /// Starts or ends the helper so that it runs exactly when `wanted`. Returns true when that changed anything.
    fn settle(&self, wanted: bool, command: impl FnOnce() -> Option<Vec<String>>) -> bool {
        // Held from the look to the start: two who ask at once must not start two helpers.
        let mut helper = self.helper.lock().unwrap();
        let running = still_runs(&mut helper);
        match (wanted && !self.stopped.load(Ordering::SeqCst), running) {
            (true, false) => {
                let Some(command) = command().filter(|command| !command.is_empty()) else { return false };
                match start(&command) {
                    Ok(child) => *helper = Some(child),
                    Err(error) => {
                        tracing::warn!("Could not keep the computer awake with {}: {error}", command[0]);
                        return false;
                    }
                }
                true
            }
            (false, true) => {
                if let Some(child) = helper.take() {
                    end(child);
                }
                true
            }
            _ => false,
        }
    }

    /// Ends the helper for good. Called when the core stops.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(child) = self.helper.lock().unwrap().take() {
            end(child);
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Manager {
    fn keep_awake_on(&self) -> bool {
        self.store().setting(SETTING).ok().flatten().as_deref() == Some("on")
    }

    /// The command that keeps the computer awake, or why there is none.
    fn awake_command(&self) -> Result<(String, Vec<String>), String> {
        if let Some(command) = self.awake.stand_in.lock().unwrap().clone() {
            return Ok((command.join(" "), command));
        }
        helper_of_this_system().map(|(name, command)| (name.to_string(), command))
    }

    /// Runs `command` in place of the system's own helper. For tests: they must
    /// not keep the machine they run on awake, and must be able to see the helper.
    pub fn keep_awake_with(&self, command: Vec<String>) {
        *self.awake.stand_in.lock().unwrap() = Some(command);
    }

    /// The setting as Settings shows it: whether it is on, whether this system
    /// has a way (`note` says why not), what the helper is called and whether
    /// it runs right now.
    pub fn keep_awake(&self) -> Value {
        let way = self.awake_command();
        json!({
            "on": self.keep_awake_on(),
            "available": way.is_ok(),
            "how": way.as_ref().ok().map(|(name, _)| name),
            "note": way.as_ref().err(),
            "active": self.awake.active()
        })
    }

    pub fn set_keep_awake(&self, on: bool) -> Result<()> {
        if on && let Err(why) = self.awake_command() {
            bail!("{why}");
        }
        self.store().set_setting(SETTING, on.then_some("on"))?;
        self.settle_awake();
        Ok(())
    }

    /// Takes note that a task's agent started or stopped working.
    pub(crate) fn agent_works(&self, task_id: &str, working: bool) {
        {
            let mut tasks = self.awake.working.lock().unwrap();
            let changed = if working { tasks.insert(task_id.to_string()) } else { tasks.remove(task_id) };
            if !changed {
                return;
            }
        }
        self.settle_awake();
    }

    /// The helper runs while the setting is on and some agent is working, and at no other time.
    fn settle_awake(&self) {
        let wanted = !self.awake.working.lock().unwrap().is_empty() && self.keep_awake_on();
        let changed = self.awake.settle(wanted, || self.awake_command().ok().map(|(_, command)| command));
        if changed {
            // Settings says whether the helper runs right now.
            self.broadcast("settings.changed", json!({ "agents": false }));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 sends nothing; it only asks whether the process is there.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn the_helper_runs_exactly_while_it_is_wanted_and_never_after_the_stop() {
        let awake = KeepAwake::default();
        let sleeper = || Some(vec!["/bin/sh".to_string(), "-c".to_string(), "exec sleep 600".to_string()]);
        assert!(!awake.active());
        assert!(!awake.settle(false, sleeper), "nothing to end");

        assert!(awake.settle(true, sleeper));
        assert!(!awake.settle(true, sleeper), "one helper is enough");
        let pid = awake.helper.lock().unwrap().as_ref().unwrap().id();
        assert!(awake.active() && alive(pid));

        assert!(awake.settle(false, sleeper));
        assert!(!awake.active() && !alive(pid), "the helper is gone, not merely forgotten");

        // After the core's stop nothing is started any more, whoever asks.
        assert!(awake.settle(true, sleeper));
        let pid = awake.helper.lock().unwrap().as_ref().unwrap().id();
        awake.stop();
        assert!(!alive(pid));
        assert!(!awake.settle(true, sleeper));
        assert!(!awake.active());
    }

    #[test]
    fn a_helper_that_ended_by_itself_or_cannot_be_started_is_no_helper() {
        let awake = KeepAwake::default();
        assert!(awake.settle(true, || Some(vec!["/bin/sh".into(), "-c".into(), "true".into()])));
        // Give it the moment it needs to end.
        for _ in 0..200 {
            if !awake.active() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!awake.active());
        assert!(!awake.settle(true, || Some(vec!["/nonexistent/ae-test/caffeinate".into()])));
        assert!(!awake.settle(true, || None), "no way to keep this system awake");
        assert!(!awake.active());
    }

    #[test]
    fn this_system_names_its_helper_or_says_why_it_has_none() {
        match helper_of_this_system() {
            Ok((name, command)) => {
                assert!(name.starts_with("caffeinate") || name.starts_with("systemd-inhibit"));
                // The helper is told which process to watch, so it can end with it: the core's own.
                assert!(command.contains(&std::process::id().to_string()));
            }
            Err(why) => assert!(why.contains("not found") || why.contains("Not available"), "{why}"),
        }
    }
}
