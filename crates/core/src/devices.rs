//! The iOS simulator and the Android emulator in the Preview.
//!
//! Neither can be put into a page, so the interface shows a picture of the
//! device's screen and asks for a new one about once a second. Tapping and
//! typing happen in the device's own window. The core does three things for a
//! device tab: it lists the devices, opens the app's address on one, and takes
//! pictures of its screen. All of it goes through the tools the systems come
//! with: `xcrun simctl` for the simulators of Xcode, `adb` for the emulators
//! of Android.
//!
//! Nothing here runs between two questions of the interface: no loop of the
//! core takes pictures. What a device tab did to a device is undone when the
//! tab is left, the window goes away, the task is archived or the core stops:
//! a simulator the core started is shut down, a port it opened on an emulator
//! is closed. A simulator that was running already is never shut down.

use crate::preview::{Previews, split_address};
use crate::tasks::{Manager, now_ms};
use crate::tools::{self, Ran, Tools, words};
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long listing devices, and any other quick question, may take.
const QUICK: Duration = Duration::from_secs(20);
/// A simulator that was never started before takes its time.
const BOOT_LIMIT: Duration = Duration::from_secs(300);
/// So does the first address after a start: the device's browser starts for the first time. (Measured on a
/// busy Mac: a simulator that had just been started for the first time took more than 20 seconds over it.)
const OPEN_LIMIT: Duration = Duration::from_secs(120);
/// One picture of a screen.
const SCREEN_LIMIT: Duration = Duration::from_secs(15);

#[cfg(not(target_os = "macos"))]
const ONLY_ON_A_MAC: &str = "The iOS simulator runs only on a Mac with Xcode.";
const NEEDS_XCODE: &str = "The iOS simulator needs Xcode on this Mac: install it from the App Store and open it once.";
const NO_SIMULATOR: &str = "Xcode has no iOS simulator yet: add one in Xcode under Window, Devices and Simulators.";
const NEEDS_ADB: &str =
    "The Android emulator needs adb, which comes with Android Studio (SDK Platform-Tools in its SDK Manager).";
const NO_EMULATOR: &str = "No Android emulator is running: start one from the Device Manager of Android Studio.";

/// Which kind of device a tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Ios,
    Android,
}

impl Kind {
    fn read(params: &Value) -> Result<Kind> {
        match params.get("kind").and_then(Value::as_str) {
            Some("ios") => Ok(Kind::Ios),
            Some("android") => Ok(Kind::Android),
            _ => bail!("Missing parameter: kind (ios or android)"),
        }
    }
}

/// A device as the interface lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    /// What the system's tool calls it: the simulator's UDID, the emulator's serial.
    pub id: String,
    pub name: String,
    /// "iOS 27.0", "Android 15".
    pub system: String,
    /// "running", "off", or "starting" for one that is neither yet.
    pub state: &'static str,
}

/// What the device tab of one task shows.
#[derive(Debug, Clone)]
struct Session {
    kind: Kind,
    id: String,
    name: String,
    /// The port of the app on this computer.
    port: u16,
    /// The window of the interface that asked. 0 when nobody said.
    window: u64,
    /// The core made the emulator pass this port on to this computer, so the core takes that back.
    reversed: bool,
}

/// Every device a device tab shows, and what the core did to them.
pub struct Devices {
    tools: Tools,
    sessions: Mutex<HashMap<String, Session>>,
    /// The simulators the core started itself. Only these are ever shut down by it.
    started: Mutex<HashSet<String>>,
    /// The windows of the interface that are connected right now.
    windows: Mutex<HashSet<u64>>,
    next_window: AtomicU64,
    /// Where `xcrun` is, once Xcode was found behind it: asked once, not for every picture.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    xcrun_found: Mutex<Option<PathBuf>>,
}

/// One window of the interface, for as long as it is connected. When it goes away,
/// the devices its Preview showed are let go, as if their tab had been left.
pub struct Window {
    id: u64,
    devices: Arc<Devices>,
}

impl Window {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.devices.windows.lock().unwrap().remove(&self.id);
        let (devices, id) = (self.devices.clone(), self.id);
        // Letting go asks the system's tools, which takes a moment: not something to do while being dropped.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { devices.close_window(id).await });
        }
    }
}

/// Reads what `xcrun simctl list devices available -j` prints: the simulators that can run iOS.
fn parse_simulators(text: &str) -> Result<Vec<Device>> {
    let listed: Value = serde_json::from_str(text).map_err(|_| anyhow!("its list of simulators could not be read"))?;
    let by_system =
        listed.get("devices").and_then(Value::as_object).ok_or_else(|| anyhow!("its answer lists no devices"))?;
    let mut devices = Vec::new();
    for (runtime, list) in by_system {
        // "com.apple.CoreSimulator.SimRuntime.iOS-27-0" is iOS 27.0. Watches and TVs have no browser to open an address in.
        let short = runtime.rsplit('.').next().unwrap_or(runtime);
        let Some(version) = short.strip_prefix("iOS-") else { continue };
        let system = format!("iOS {}", version.replace('-', "."));
        for device in list.as_array().into_iter().flatten() {
            let text = |key: &str| device.get(key).and_then(Value::as_str);
            let (Some(id), Some(name)) = (text("udid"), text("name")) else { continue };
            if device.get("isAvailable").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let state = match text("state") {
                Some("Booted") => "running",
                Some("Shutdown") => "off",
                _ => "starting",
            };
            devices.push(Device { id: id.into(), name: name.into(), system: system.clone(), state });
        }
    }
    // What runs already comes first, then the newest system, then by name.
    let version =
        |device: &Device| device.system.split([' ', '.']).filter_map(|n| n.parse::<u32>().ok()).collect::<Vec<_>>();
    devices
        .sort_by(|a, b| (a.state != "running", version(b), &a.name).cmp(&(b.state != "running", version(a), &b.name)));
    Ok(devices)
}

/// Reads what `adb devices -l` prints: each emulator's serial, its state and the model it says it is.
/// A phone on a cable is left out: this tab is about emulators, and a phone is not the core's to open pages on.
fn parse_emulators(text: &str) -> Vec<(String, String, Option<String>)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let (serial, state) = (parts.next()?, parts.next()?);
            let model = parts.find_map(|part| part.strip_prefix("model:")).map(|model| model.replace('_', " "));
            serial.starts_with("emulator-").then(|| (serial.to_string(), state.to_string(), model))
        })
        .collect()
}

/// Whether `adb reverse --list` says the device already passes this port on to this computer.
fn passes_on(listed: &str, port: u16) -> bool {
    let wanted = format!("tcp:{port}");
    // Each line: who made the rule, the port on the device, the port here.
    listed.lines().any(|line| line.split_whitespace().nth(1) == Some(wanted.as_str()))
}

/// Text as one word for the shell of an Android device. `adb shell` joins its words with spaces and hands
/// them to that shell, where `&` and `;` in an address would otherwise end the command.
fn device_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

impl Devices {
    pub fn new(tools: Tools) -> Self {
        Self {
            tools,
            sessions: Mutex::default(),
            started: Mutex::default(),
            windows: Mutex::default(),
            // 0 is "nobody said which window".
            next_window: AtomicU64::new(1),
            xcrun_found: Mutex::default(),
        }
    }

    /// Notes a window of the interface that has just connected.
    pub fn window(self: &Arc<Self>) -> Window {
        let id = self.next_window.fetch_add(1, Ordering::SeqCst);
        self.windows.lock().unwrap().insert(id);
        Window { id, devices: self.clone() }
    }

    async fn xcrun(&self) -> Result<PathBuf> {
        match &self.tools.xcrun {
            // A program that was named is the only one used, there or not.
            Some(named) => named.is_file().then(|| named.clone()).ok_or_else(|| anyhow!(NEEDS_XCODE)),
            None => self.xcrun_of_xcode().await,
        }
    }

    /// The `xcrun` of this Mac, once it is known that Xcode with its simulators stands behind it.
    ///
    /// On a Mac without Apple's developer tools, `/usr/bin/xcrun` is a stub that puts a dialog on the
    /// screen offering to install them. Looking at a tab must not do that, so the question goes to
    /// `xcode-select` first, which only answers.
    #[cfg(target_os = "macos")]
    async fn xcrun_of_xcode(&self) -> Result<PathBuf> {
        if let Some(known) = self.xcrun_found.lock().unwrap().clone() {
            return Ok(known);
        }
        let xcrun = PathBuf::from("/usr/bin/xcrun");
        let asked = tools::run(Path::new("/usr/bin/xcode-select"), words(["-p"]), QUICK).await;
        let developer = asked.ok().filter(Ran::ok).map(|ran| PathBuf::from(ran.text().trim()));
        if !xcrun.is_file() || !developer.is_some_and(|folder| folder.join("usr/bin/simctl").is_file()) {
            bail!(NEEDS_XCODE);
        }
        *self.xcrun_found.lock().unwrap() = Some(xcrun.clone());
        Ok(xcrun)
    }

    #[cfg(not(target_os = "macos"))]
    async fn xcrun_of_xcode(&self) -> Result<PathBuf> {
        bail!(ONLY_ON_A_MAC)
    }

    fn adb(&self) -> Result<PathBuf> {
        if let Some(named) = &self.tools.adb {
            return named.is_file().then(|| named.clone()).ok_or_else(|| anyhow!(NEEDS_ADB));
        }
        let file = if cfg!(windows) { "adb.exe" } else { "adb" };
        // Where Android Studio puts the tools when nobody says otherwise, and where the user says it did.
        let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
            .into_iter()
            .filter_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from));
        let usual = [
            dirs::home_dir().map(|home| home.join("Library/Android/sdk")),
            dirs::home_dir().map(|home| home.join("Android/Sdk")),
            dirs::data_local_dir().map(|data| data.join("Android/Sdk")),
        ];
        let places = named.chain(usual.into_iter().flatten()).map(|sdk| sdk.join("platform-tools").join(file));
        which::which("adb").ok().or_else(|| tools::first_file(places)).ok_or_else(|| anyhow!(NEEDS_ADB))
    }

    async fn ask_xcrun(&self, args: Vec<OsString>, limit: Duration) -> Result<Ran> {
        tools::run(&self.xcrun().await?, args, self.tools.limit(limit)).await
    }

    /// Asks `adb` about one emulator.
    async fn ask_adb<const N: usize>(&self, serial: &str, rest: [&str; N], limit: Duration) -> Result<Ran> {
        let mut args = words(["-s", serial]);
        args.extend(words(rest));
        tools::run(&self.adb()?, args, self.tools.limit(limit)).await
    }

    /// The simulators or emulators there are. An error says, in one sentence, what is needed to have any.
    async fn list(&self, kind: Kind) -> Result<Vec<Device>> {
        let devices = match kind {
            Kind::Ios => {
                let ran = self.ask_xcrun(words(["simctl", "list", "devices", "available", "-j"]), QUICK).await?;
                // Xcode's command line tools alone have `xcrun` and no simulators.
                let listed = if ran.ok() { parse_simulators(&ran.text()) } else { Err(anyhow!(ran.last_words())) };
                listed.map_err(|said| anyhow!("{NEEDS_XCODE} (Asked for its simulators, xcrun said: {said}.)"))?
            }
            Kind::Android => {
                let ran = tools::run(&self.adb()?, words(["devices", "-l"]), self.tools.limit(QUICK)).await?;
                if !ran.ok() {
                    bail!("adb did not list the devices. It said: {}", ran.last_words());
                }
                let mut devices = Vec::new();
                for (serial, state, model) in parse_emulators(&ran.text()) {
                    if state != "device" {
                        let name = model.unwrap_or_else(|| serial.clone());
                        devices.push(Device { id: serial, name, system: "Android".into(), state: "starting" });
                        continue;
                    }
                    // The name the user gave it in the Device Manager, and the Android it runs.
                    let said = |ran: Result<Ran>| {
                        let ran = ran.ok().filter(Ran::ok)?;
                        let text = ran.text();
                        text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
                    };
                    let name = said(self.ask_adb(&serial, ["emu", "avd", "name"], QUICK).await)
                        .filter(|name| name != "OK")
                        .map(|name| name.replace('_', " "))
                        .or(model)
                        .unwrap_or_else(|| serial.clone());
                    let release =
                        said(self.ask_adb(&serial, ["shell", "getprop", "ro.build.version.release"], QUICK).await);
                    let system = release.map_or("Android".into(), |release| format!("Android {release}"));
                    devices.push(Device { id: serial, name, system, state: "running" });
                }
                devices
            }
        };
        if devices.is_empty() {
            bail!(if kind == Kind::Ios { NO_SIMULATOR } else { NO_EMULATOR });
        }
        Ok(devices)
    }

    /// Opens an address on a device and makes it the device this task's tab shows.
    async fn open(&self, task_id: &str, window: u64, kind: Kind, id: &str, address: &str) -> Result<Value> {
        let (target, path) = split_address(address)?;
        let before = self.sessions.lock().unwrap().get(task_id).cloned();
        // Another device was on show, or the same one for an app on another port: that is over first.
        if before.as_ref().is_some_and(|before| before.kind != kind || before.id != id || before.port != target.port) {
            self.close(task_id).await;
        }
        let listed = self.list(kind).await?;
        let device = listed
            .into_iter()
            .find(|device| device.id == id)
            .ok_or_else(|| anyhow!("That device is not there any more. Look again for the ones that are."))?;
        let name = device.name.clone();
        let shown = format!("{}{}", target.authority(), if path == "/" { "" } else { &path });
        let mut session = Session {
            kind,
            id: id.to_string(),
            name: name.clone(),
            port: target.port,
            window,
            reversed: self.sessions.lock().unwrap().get(task_id).is_some_and(|before| before.reversed),
        };

        match kind {
            Kind::Ios => {
                if device.state != "running" {
                    // `-b` starts a simulator that is off, and the command returns when it is ready for an address.
                    let ran = self.ask_xcrun(words(["simctl", "bootstatus", id, "-b"]), BOOT_LIMIT).await;
                    if let Some(said) = failure(&ran) {
                        bail!("{name} did not start. xcrun said: {said}");
                    }
                    // One that somebody else was starting at that moment is theirs, not ours.
                    if device.state == "off" {
                        self.started.lock().unwrap().insert(id.to_string());
                    }
                }
                // The simulator shares this computer's network: the app's own address works in it as it is.
                let url = format!("{}{path}", target.origin());
                let ran = self.ask_xcrun(words(["simctl", "openurl", id, &url]), OPEN_LIMIT).await;
                if let Some(said) = failure(&ran) {
                    // Whatever the tab started is not left running behind a failure.
                    self.let_go(session).await;
                    bail!("{name} did not open {shown}. xcrun said: {said}");
                }
            }
            Kind::Android => {
                if device.state != "running" {
                    bail!("{name} is still starting. Wait for its home screen, then look again.");
                }
                // "localhost" on the emulator is the emulator itself. The port is passed on to this computer,
                // unless a rule for it is there already: that one is somebody else's and stays.
                let port = target.port.to_string();
                let rule = format!("tcp:{port}");
                if !session.reversed {
                    let listed = self.ask_adb(id, ["reverse", "--list"], QUICK).await?;
                    if !passes_on(&listed.text(), target.port) {
                        let ran = self.ask_adb(id, ["reverse", &rule, &rule], QUICK).await;
                        if let Some(said) = failure(&ran) {
                            bail!("{name} could not be given a way to port {port} of this computer. adb said: {said}");
                        }
                        session.reversed = true;
                    }
                }
                let url = device_word(&format!("http://localhost:{port}{path}"));
                let view = ["shell", "am", "start", "-a", "android.intent.action.VIEW", "-d", &url];
                let ran = self.ask_adb(id, view, OPEN_LIMIT).await;
                // `am` reports some failures in words only, with a code that says all is well.
                let complaint = ran
                    .as_ref()
                    .ok()
                    .filter(|ran| ran.text().contains("Error:"))
                    .map(|ran| ran.text().trim().to_string());
                if let Some(said) = failure(&ran).or(complaint) {
                    self.let_go(session).await;
                    bail!("{name} did not open {shown}. adb said: {said}");
                }
            }
        }

        let started_here = kind == Kind::Ios && self.started.lock().unwrap().contains(id);
        // The window that asked went away while the device was starting: nobody is looking at it.
        if window != 0 && !self.windows.lock().unwrap().contains(&window) {
            self.let_go(session).await;
            bail!("The window that asked for {name} is gone");
        }
        self.sessions.lock().unwrap().insert(task_id.to_string(), session);
        Ok(json!({ "kind": kind, "id": id, "name": name, "address": shown, "startedHere": started_here }))
    }

    /// A picture of the screen of the device this task shows. `have` is the mark of the picture the
    /// interface has: when the screen still looks the same, only that is said and no picture is sent again.
    /// `data_dir` is where the core keeps its own files: a simulator's picture passes through one.
    async fn screen(&self, task_id: &str, have: Option<&str>, data_dir: &Path) -> Result<Value> {
        let session = self.sessions.lock().unwrap().get(task_id).cloned();
        let session = session.ok_or_else(|| anyhow!("No device is on show for this task. Pick one again."))?;
        let (ran, picture) = match session.kind {
            Kind::Ios => {
                // `simctl` says that "-" for the file means its output. The one of Xcode 27.2 writes a file
                // called "-" into the folder it runs in and prints nothing, so it is given a file and that is read.
                let scratch = crate::shots::Scratch::new(data_dir, "screens")?;
                let file = scratch.0.join("screen.png");
                let mut args = words(["simctl", "io", &session.id, "screenshot", "--type=png"]);
                args.push(file.as_os_str().to_owned());
                let ran = self.ask_xcrun(args, SCREEN_LIMIT).await?;
                (ran, tokio::fs::read(&file).await.unwrap_or_default())
            }
            Kind::Android => {
                let mut ran = self.ask_adb(&session.id, ["exec-out", "screencap", "-p"], SCREEN_LIMIT).await?;
                let picture = std::mem::take(&mut ran.stdout);
                (ran, picture)
            }
        };
        if !ran.ok() || crate::shots::png_size(&picture).is_none() {
            let said = if ran.timed_out { "it did not answer in time".to_string() } else { ran.complaint() };
            bail!("{} gave no picture of its screen: {said}. Is it still running?", session.name);
        }
        let mut hasher = std::hash::DefaultHasher::new();
        picture.hash(&mut hasher);
        let mark = format!("{:016x}", hasher.finish());
        if have == Some(mark.as_str()) {
            return Ok(json!({ "at": now_ms(), "mark": mark, "same": true }));
        }
        let png = base64::engine::general_purpose::STANDARD.encode(&picture);
        Ok(json!({ "at": now_ms(), "mark": mark, "png": png }))
    }

    /// Brings the window of the device forward, where one can tap and type.
    async fn front(&self, task_id: &str) -> Result<()> {
        let session = self.sessions.lock().unwrap().get(task_id).cloned();
        let session = session.ok_or_else(|| anyhow!("No device is on show for this task. Pick one again."))?;
        if session.kind != Kind::Ios {
            bail!("The emulator's window cannot be brought forward from here. Look for it among your windows.");
        }
        // `open` asks the system for the Simulator app; `xcrun` finds `open` like any other tool.
        let args = words(["open", "-a", "Simulator", "--args", "-CurrentDeviceUDID", &session.id]);
        if let Some(said) = failure(&self.ask_xcrun(args, QUICK).await) {
            bail!("The Simulator window could not be brought forward. It said: {said}");
        }
        Ok(())
    }

    /// Undoes what the tab of a device did to it. The session is already out of the list.
    async fn let_go(&self, session: Session) {
        match session.kind {
            Kind::Ios => {
                // Another task still shows this simulator: it stays on until that one lets go too.
                let shown = |other: &Session| other.kind == Kind::Ios && other.id == session.id;
                if self.sessions.lock().unwrap().values().any(shown) {
                    return;
                }
                // Only a simulator the core started is the core's to shut down.
                if self.started.lock().unwrap().remove(&session.id) {
                    let ran = self.ask_xcrun(words(["simctl", "shutdown", &session.id]), QUICK).await;
                    if let Some(said) = failure(&ran) {
                        tracing::debug!("Could not shut down {}: {said}", session.name);
                    }
                }
            }
            Kind::Android => {
                if !session.reversed {
                    return;
                }
                // Another task shows the same app on the same emulator: the way to the port is now that one's to take back.
                let inherited = {
                    let mut sessions = self.sessions.lock().unwrap();
                    let same = |other: &&mut Session| {
                        other.kind == Kind::Android && other.id == session.id && other.port == session.port
                    };
                    sessions.values_mut().find(same).map(|heir| heir.reversed = true).is_some()
                };
                if inherited {
                    return;
                }
                let rule = format!("tcp:{}", session.port);
                let ran = self.ask_adb(&session.id, ["reverse", "--remove", &rule], QUICK).await;
                if let Some(said) = failure(&ran) {
                    tracing::debug!("Could not close port {} on {}: {said}", session.port, session.name);
                }
            }
        }
    }

    /// Lets go of the device a task shows. Called when its tab is left and when the task is finished.
    pub async fn close(&self, task_id: &str) {
        let session = self.sessions.lock().unwrap().remove(task_id);
        if let Some(session) = session {
            self.let_go(session).await;
        }
    }

    /// Lets go of every device a window showed. Called when the window is gone.
    async fn close_window(&self, window: u64) {
        let tasks: Vec<String> = {
            let sessions = self.sessions.lock().unwrap();
            sessions.iter().filter(|(_, session)| session.window == window).map(|(task, _)| task.clone()).collect()
        };
        for task_id in tasks {
            self.close(&task_id).await;
        }
    }

    /// Lets go of every device. Called when the core shuts down.
    pub async fn close_all(&self) {
        let tasks: Vec<String> = self.sessions.lock().unwrap().keys().cloned().collect();
        for task_id in tasks {
            self.close(&task_id).await;
        }
    }
}

/// What went wrong with a run, in the tool's own words, or `None` when it went well.
fn failure(ran: &Result<Ran>) -> Option<String> {
    match ran {
        Ok(ran) if ran.ok() => None,
        Ok(ran) if ran.timed_out => Some("it did not answer in time".into()),
        Ok(ran) => Some(ran.last_words()),
        Err(error) => Some(format!("{error:#}")),
    }
}

/// The commands of the device tabs. `kind` is "ios" or "android".
///
/// - `devices.list { taskId, kind }` gives `{ kind, needs, devices: [{ id, name, system, state }] }`.
///   `needs` is one sentence that says what is missing when there is nothing to list; `state` is
///   "running", "off" or "starting".
/// - `devices.open { taskId, kind, id, address }` starts the device when it is off, opens the address
///   on it and gives `{ kind, id, name, address, startedHere }`. Asked again, it opens the address again.
/// - `devices.screen { taskId, have? }` gives `{ at, mark, png }`, a PNG in base64, or
///   `{ at, mark, same: true }` when the screen is the picture marked `have`.
/// - `devices.front { taskId }` brings the Simulator window forward. `devices.close { taskId }` lets the device go.
pub async fn command(previews: &Previews, manager: &Manager, method: &str, params: &Value) -> Result<Value> {
    let text =
        |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
    let task_id = text("taskId")?;
    manager.task(task_id)?;
    let devices = &previews.devices;
    match method {
        "devices.list" => {
            let kind = Kind::read(params)?;
            Ok(match devices.list(kind).await {
                Ok(listed) => json!({ "kind": kind, "needs": null, "devices": listed }),
                // Nothing to list is not a failure of the command: the tab says what is needed.
                Err(needs) => json!({ "kind": kind, "needs": format!("{needs:#}"), "devices": [] }),
            })
        }
        "devices.open" => {
            // The door tells which window asks, so the device can be let go when that window is gone.
            let window = params.get("window").and_then(Value::as_u64).unwrap_or(0);
            devices.open(task_id, window, Kind::read(params)?, text("id")?, text("address")?).await
        }
        "devices.screen" => {
            devices.screen(task_id, params.get("have").and_then(Value::as_str), manager.data_dir()).await
        }
        "devices.front" => devices.front(task_id).await.map(|()| json!({})),
        "devices.close" => {
            devices.close(task_id).await;
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simulators_of_xcode_are_read_and_the_running_ones_come_first() {
        let listed = r#"{ "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-18-6": [
                { "udid": "AAAA", "name": "iPhone 16", "state": "Shutdown", "isAvailable": true }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                { "udid": "BBBB", "name": "iPhone 17 Pro", "state": "Shutdown", "isAvailable": true },
                { "udid": "CCCC", "name": "iPad Air", "state": "Booted", "isAvailable": true },
                { "udid": "DDDD", "name": "iPhone 17", "state": "Booting", "isAvailable": true },
                { "udid": "EEEE", "name": "Broken", "state": "Shutdown", "isAvailable": false }
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-12-0": [
                { "udid": "FFFF", "name": "Apple Watch", "state": "Booted", "isAvailable": true }
            ]
        } }"#;
        let devices = parse_simulators(listed).unwrap();
        let told: Vec<_> =
            devices.iter().map(|d| (d.id.as_str(), d.name.as_str(), d.system.as_str(), d.state)).collect();
        assert_eq!(
            told,
            vec![
                ("CCCC", "iPad Air", "iOS 27.0", "running"),
                ("DDDD", "iPhone 17", "iOS 27.0", "starting"),
                ("BBBB", "iPhone 17 Pro", "iOS 27.0", "off"),
                ("AAAA", "iPhone 16", "iOS 18.6", "off"),
            ]
        );
        // This is what Xcode prints when it has a system and no simulator for it.
        let none = r#"{ "devices" : { "com.apple.CoreSimulator.SimRuntime.iOS-27-0" : [ ] } }"#;
        assert!(parse_simulators(none).unwrap().is_empty());
        assert!(parse_simulators("xcrun: error: unable to find utility \"simctl\"").is_err());
        assert!(parse_simulators("{}").is_err());
    }

    #[test]
    fn only_emulators_are_read_from_the_list_of_adb() {
        let listed = "* daemon started successfully\nList of devices attached\n\
            emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\n\
            1eaed69e               device usb:1-1 product:1920N_RU model:vivo_1920 device:1920 transport_id:2\n\
            emulator-5556          offline transport_id:3\n\n";
        assert_eq!(
            parse_emulators(listed),
            vec![
                ("emulator-5554".to_string(), "device".to_string(), Some("sdk gphone64 arm64".to_string())),
                ("emulator-5556".to_string(), "offline".to_string(), None),
            ],
            "a phone on a cable is not an emulator"
        );
        assert!(parse_emulators("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn a_port_that_is_passed_on_already_is_seen() {
        let listed = "UsbFfs tcp:3000 tcp:3000\nUsbFfs tcp:8081 tcp:8081\n";
        assert!(passes_on(listed, 3000));
        assert!(passes_on(listed, 8081));
        assert!(!passes_on(listed, 300), "a port is compared whole");
        assert!(!passes_on("", 3000));
    }

    #[test]
    fn an_address_stays_one_word_in_the_shell_of_the_device() {
        assert_eq!(device_word("http://localhost:3000/a?b=1&c=2;d"), "'http://localhost:3000/a?b=1&c=2;d'");
        assert_eq!(device_word("http://localhost:3000/it's"), r"'http://localhost:3000/it'\''s'");
        assert_eq!(device_word("$(reboot)`x`"), "'$(reboot)`x`'");
    }

    #[tokio::test]
    async fn a_tool_that_was_named_is_the_only_one_used() {
        let missing = PathBuf::from("/nonexistent/ae-test/tool");
        let devices = Devices::new(Tools { xcrun: Some(missing.clone()), adb: Some(missing), ..Tools::default() });
        assert_eq!(devices.xcrun().await.unwrap_err().to_string(), NEEDS_XCODE);
        assert_eq!(devices.adb().unwrap_err().to_string(), NEEDS_ADB);
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn the_simulator_is_not_looked_for_where_it_cannot_be() {
        let devices = Devices::new(Tools::default());
        assert_eq!(devices.xcrun().await.unwrap_err().to_string(), ONLY_ON_A_MAC);
    }
}
