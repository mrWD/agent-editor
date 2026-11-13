//! The iOS simulator and the Android emulator in the Preview.
//!
//! Neither can be put into a page, so the interface shows a picture of the
//! device's screen and asks for a new one about once a second. The core does
//! four things for a device tab: it lists the devices, starts one that is
//! off, opens the app's address on it, and takes pictures of its screen. A
//! tap, a swipe, typed text and the device's buttons are passed on to it, and
//! so is light or dark. All of it goes through the tools the systems come
//! with: `xcrun simctl` for the simulators of Xcode, `adb` and `emulator` for
//! the emulators of Android. A simulator takes no touch through `simctl`:
//! that goes through `idb` when this Mac has it, and otherwise stays in the
//! Simulator's own window.
//!
//! Nothing here runs between two questions of the interface: no loop of the
//! core takes pictures. What a device tab did to a device is undone when the
//! tab is left, the window goes away, the task is archived or the core stops:
//! a simulator or an emulator the core started is shut down, a port it opened
//! on an emulator is closed. A device that was running already is never shut
//! down.

use crate::preview::{Previews, split_address};
use crate::tasks::{Manager, now_ms};
use crate::tools::{self, Ran, Tools, words};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long listing devices, and any other quick question, may take.
const QUICK: Duration = Duration::from_secs(20);
/// A simulator that was never started before takes its time.
const BOOT_LIMIT: Duration = Duration::from_secs(300);
/// So does the first address after a start: the device's browser starts for the first time. (Measured on a
/// busy Mac: a simulator that had just been started for the first time took more than 20 seconds over it.)
const OPEN_LIMIT: Duration = Duration::from_secs(120);
/// One picture of a screen. A simulator on a busy Mac took 9 to 12 seconds for one (measured by hand).
const SCREEN_LIMIT: Duration = Duration::from_secs(30);

#[cfg(not(target_os = "macos"))]
const ONLY_ON_A_MAC: &str = "The iOS simulator runs only on a Mac with Xcode.";
const NEEDS_XCODE: &str = "The iOS simulator needs Xcode on this Mac: install it from the App Store and open it once.";
const NO_SIMULATOR: &str = "Xcode has no iOS simulator yet: add one in Xcode under Window, Devices and Simulators.";
const NEEDS_ADB: &str =
    "The Android emulator needs adb, which comes with Android Studio (SDK Platform-Tools in its SDK Manager).";
const NO_EMULATOR: &str = "No Android emulator is running: start one from the Device Manager of Android Studio.";
const NO_AVD: &str = "Android has no emulator yet: make one in the Device Manager of Android Studio.";
const NEEDS_IDB: &str = "Tapping a simulator from here needs idb on this Mac (brew install facebook/fb/idb-companion, then pipx install fb-idb). Until then, tap and type in the Simulator's own window.";
const NO_PICTURE_YET: &str = "Wait for the first picture of its screen.";
/// An emulator that is off has no serial yet. It is listed under the name the Device Manager gave it, behind this word.
const AVD: &str = "avd:";
/// How often an emulator that is starting is asked whether Android in it is ready.
const BOOT_LOOK: Duration = Duration::from_millis(1500);
/// How long a swipe takes on the device, in milliseconds: slow enough to scroll, too quick to be a long press.
const SWIPE_MS: &str = "250";
/// The longest text that is typed in one go.
const MAX_TYPED: usize = 500;

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
    /// How many pixels the last picture of its screen had. A touch is told as a part of the screen and lands by this.
    size: Option<(u32, u32)>,
    /// How many pixels of that picture make one point of a simulator's screen, once asked.
    scale: Option<f64>,
}

/// Every device a device tab shows, and what the core did to them.
pub struct Devices {
    tools: Tools,
    sessions: Mutex<HashMap<String, Session>>,
    /// The simulators and emulators the core started itself. Only these are ever shut down by it.
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

/// Whether text can be the name of an emulator in the Device Manager: letters, digits, dots, dashes and underscores.
fn is_avd_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|letter| letter.is_ascii_alphanumeric() || "._-".contains(letter))
}

/// Reads what `emulator -list-avds` prints: a name on each line. Newer ones also print lines of their
/// own news ("INFO    | Storing crashdata in: …"), which no name looks like.
fn parse_avds(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|line| is_avd_name(line)).map(str::to_string).collect()
}

/// Where the Android tools usually are: where the user says they are, then where Android Studio puts them.
fn android_sdks() -> Vec<PathBuf> {
    let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from));
    let usual = [
        dirs::home_dir().map(|home| home.join("Library/Android/sdk")),
        dirs::home_dir().map(|home| home.join("Android/Sdk")),
        dirs::data_local_dir().map(|data| data.join("Android/Sdk")),
    ];
    named.chain(usual.into_iter().flatten()).collect()
}

/// A place on the screen, told as two parts of it from 0 to 1, in the pixels of a picture of that size.
fn place((width, height): (u32, u32), x: f64, y: f64) -> (u32, u32) {
    let at = |part: f64, whole: u32| {
        let last = whole.saturating_sub(1);
        ((part.clamp(0.0, 1.0) * f64::from(whole)).floor() as u32).min(last)
    };
    (at(x, width), at(y, height))
}

/// What a touch is, as the interface tells it. Places are parts of the screen from 0 to 1.
#[derive(Debug, Clone, PartialEq)]
enum Touch {
    Tap { x: f64, y: f64 },
    Swipe { x: f64, y: f64, x2: f64, y2: f64 },
    Text(String),
    Key(Key),
}

/// The buttons of a device, and the two keys typing cannot do without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Home,
    Back,
    Recent,
    Enter,
    Delete,
}

impl Touch {
    fn read(params: &Value) -> Result<Touch> {
        let part = |name: &str| {
            let given = params.get(name).and_then(Value::as_f64).filter(|part| part.is_finite());
            given.ok_or_else(|| anyhow!("Missing parameter: {name} (a part of the screen from 0 to 1)"))
        };
        match params.get("what").and_then(Value::as_str) {
            Some("tap") => Ok(Touch::Tap { x: part("x")?, y: part("y")? }),
            Some("swipe") => Ok(Touch::Swipe { x: part("x")?, y: part("y")?, x2: part("x2")?, y2: part("y2")? }),
            Some("text") => {
                let text = params.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.is_empty() {
                    bail!("There is nothing to type");
                }
                if text.chars().count() > MAX_TYPED {
                    bail!("That is more than {MAX_TYPED} letters at once. Type it in parts.");
                }
                Ok(Touch::Text(text.to_string()))
            }
            Some("key") => Ok(Touch::Key(match params.get("key").and_then(Value::as_str) {
                Some("home") => Key::Home,
                Some("back") => Key::Back,
                Some("recent") => Key::Recent,
                Some("enter") => Key::Enter,
                Some("delete") => Key::Delete,
                _ => bail!("Missing parameter: key (home, back, recent, enter or delete)"),
            })),
            _ => bail!("Missing parameter: what (tap, swipe, text or key)"),
        }
    }
}

/// Text for `input text` on an Android device: it reads `%s` as a space and takes plain Latin letters only.
fn android_text(text: &str) -> Result<String> {
    if !text.chars().all(|letter| letter == ' ' || letter.is_ascii_graphic()) {
        bail!(
            "An emulator takes only plain Latin letters, digits and signs typed from here. Type the rest in its own window."
        );
    }
    Ok(device_word(&text.replace(' ', "%s")))
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
        let places = android_sdks().into_iter().map(|sdk| sdk.join("platform-tools").join(file));
        which::which("adb").ok().or_else(|| tools::first_file(places)).ok_or_else(|| anyhow!(NEEDS_ADB))
    }

    /// The program that lists the emulators that are off and starts one, when this computer has it.
    /// Without it the emulators that run are still shown; the ones that are off are just not listed.
    fn emulator(&self) -> Option<PathBuf> {
        if let Some(named) = &self.tools.emulator {
            return named.is_file().then(|| named.clone());
        }
        // An `adb` that was named is a stand-in: nothing real is looked for next to it.
        if self.tools.adb.is_some() {
            return None;
        }
        let file = if cfg!(windows) { "emulator.exe" } else { "emulator" };
        let places = android_sdks().into_iter().map(|sdk| sdk.join("emulator").join(file));
        which::which("emulator").ok().or_else(|| tools::first_file(places))
    }

    /// `idb`, which taps and types on a simulator, when this Mac has it.
    fn idb(&self) -> Option<PathBuf> {
        if let Some(named) = &self.tools.idb {
            return named.is_file().then(|| named.clone());
        }
        if self.tools.xcrun.is_some() {
            return None;
        }
        // Where Homebrew and pipx put it: a desktop app is not started with the PATH of a terminal.
        let usual = [
            Some(PathBuf::from("/opt/homebrew/bin/idb")),
            Some(PathBuf::from("/usr/local/bin/idb")),
            dirs::home_dir().map(|home| home.join(".local/bin/idb")),
        ];
        which::which("idb").ok().or_else(|| tools::first_file(usual.into_iter().flatten()))
    }

    /// Whether the picture of a device of this kind can be tapped from here, and what is needed when not.
    fn touch(&self, kind: Kind) -> (bool, Option<&'static str>) {
        match kind {
            Kind::Android => (true, None),
            Kind::Ios if self.idb().is_some() => (true, None),
            Kind::Ios => (false, Some(NEEDS_IDB)),
        }
    }

    /// The emulators the Device Manager knows, by name. None when the program that starts them is not here.
    async fn avds(&self) -> Vec<String> {
        let Some(emulator) = self.emulator() else { return Vec::new() };
        let ran = tools::run(&emulator, words(["-list-avds"]), self.tools.limit(QUICK)).await;
        ran.ok().filter(Ran::ok).map(|ran| parse_avds(&ran.text())).unwrap_or_default()
    }

    /// The first line a device answered a question with, when it answered.
    fn said(ran: Result<Ran>) -> Option<String> {
        let ran = ran.ok().filter(Ran::ok)?;
        let text = ran.text();
        text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
    }

    /// The running emulator of that name: what adb calls it, and whether Android in it is ready.
    async fn emulator_named(&self, name: &str) -> Result<Option<(String, bool)>> {
        let ran = tools::run(&self.adb()?, words(["devices", "-l"]), self.tools.limit(QUICK)).await?;
        for (serial, state, _) in parse_emulators(&ran.text()) {
            if state != "device" {
                continue;
            }
            if Self::said(self.ask_adb(&serial, ["emu", "avd", "name"], QUICK).await).as_deref() != Some(name) {
                continue;
            }
            let ready = Self::said(self.ask_adb(&serial, ["shell", "getprop", "sys.boot_completed"], QUICK).await);
            return Ok(Some((serial, ready.as_deref() == Some("1"))));
        }
        Ok(None)
    }

    /// Starts an emulator that is off and waits until Android in it is ready. Gives what adb calls it.
    async fn start_emulator(&self, name: &str) -> Result<String> {
        let shown = name.replace('_', " ");
        let emulator = self.emulator().ok_or_else(|| anyhow!(NO_EMULATOR))?;
        let mut program = tools::start_apart(&emulator, words(["-avd", name]))
            .with_context(|| format!("{shown} could not be started"))?;
        let limit = self.tools.limit(BOOT_LIMIT);
        let until = Instant::now() + limit;
        let started = loop {
            match self.emulator_named(name).await {
                Ok(Some((serial, true))) => break Ok(serial),
                Ok(_) => {}
                Err(error) => break Err(error),
            }
            // Its program ended and no emulator of that name is there: it will not come.
            if let Ok(Some(ended)) = program.try_wait()
                && !ended.success()
            {
                let code = ended.code().map_or("no code".into(), |code| format!("code {code}"));
                break Err(anyhow!(
                    "{shown} did not start: the emulator's program ended with {code}. Start it from the Device Manager of Android Studio to see why."
                ));
            }
            if Instant::now() >= until {
                break Err(anyhow!("{shown} did not start in time. Look for its window: it may still be starting."));
            }
            tokio::time::sleep(BOOT_LOOK.min(limit / 20)).await;
        };
        // The program goes on for as long as the emulator's window: it is collected when it ends, not waited for here.
        std::thread::spawn(move || program.wait());
        started
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
                // The names the Device Manager gave the ones that run, to tell them from the ones that are off.
                let mut running = Vec::new();
                for (serial, state, model) in parse_emulators(&ran.text()) {
                    if state != "device" {
                        let name = model.unwrap_or_else(|| serial.clone());
                        devices.push(Device { id: serial, name, system: "Android".into(), state: "starting" });
                        continue;
                    }
                    // The name the user gave it in the Device Manager, and the Android it runs.
                    let given = Self::said(self.ask_adb(&serial, ["emu", "avd", "name"], QUICK).await)
                        .filter(|name| name != "OK");
                    running.extend(given.clone());
                    let name = given.map(|name| name.replace('_', " ")).or(model).unwrap_or_else(|| serial.clone());
                    let release = Self::said(
                        self.ask_adb(&serial, ["shell", "getprop", "ro.build.version.release"], QUICK).await,
                    );
                    let system = release.map_or("Android".into(), |release| format!("Android {release}"));
                    devices.push(Device { id: serial, name, system, state: "running" });
                }
                // The ones that are off: they can be started from here.
                for name in self.avds().await {
                    if !running.contains(&name) {
                        let id = format!("{AVD}{name}");
                        devices.push(Device {
                            id,
                            name: name.replace('_', " "),
                            system: "Android".into(),
                            state: "off",
                        });
                    }
                }
                devices
            }
        };
        if devices.is_empty() {
            bail!(match kind {
                Kind::Ios => NO_SIMULATOR,
                Kind::Android if self.emulator().is_some() => NO_AVD,
                Kind::Android => NO_EMULATOR,
            });
        }
        Ok(devices)
    }

    /// Opens an address on a device and makes it the device this task's tab shows.
    async fn open(&self, task_id: &str, window: u64, kind: Kind, id: &str, address: &str) -> Result<Value> {
        let (target, path) = split_address(address)?;
        // An emulator that is off is asked for by its name in the Device Manager. One of that name that
        // runs by now is that emulator (the tab asks again on Reload); otherwise it is started below.
        let mut id = id.to_string();
        let mut to_start = None;
        if kind == Kind::Android
            && let Some(name) = id.strip_prefix(AVD).map(str::to_string)
        {
            if !is_avd_name(&name) {
                bail!("That device is not there any more. Look again for the ones that are.");
            }
            match self.emulator_named(&name).await? {
                Some((serial, true)) => id = serial,
                _ => to_start = Some(name),
            }
        }
        let before = self.sessions.lock().unwrap().get(task_id).cloned();
        // Another device was on show, or the same one for an app on another port: that is over first.
        let other = |before: &Session| before.kind != kind || before.id != id || before.port != target.port;
        if before.as_ref().is_some_and(other) {
            self.close(task_id).await;
        }
        if let Some(name) = to_start {
            id = self.start_emulator(&name).await?;
            self.started.lock().unwrap().insert(id.clone());
        }
        let id = id.as_str();
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
            size: before.as_ref().filter(|before| before.id == id).and_then(|before| before.size),
            scale: before.as_ref().filter(|before| before.id == id).and_then(|before| before.scale),
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
                    self.let_go(session).await;
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
                            self.let_go(session).await;
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

        let started_here = self.started.lock().unwrap().contains(id);
        let (touch, touch_needs) = self.touch(kind);
        // The window that asked went away while the device was starting: nobody is looking at it.
        if window != 0 && !self.windows.lock().unwrap().contains(&window) {
            self.let_go(session).await;
            bail!("The window that asked for {name} is gone");
        }
        self.sessions.lock().unwrap().insert(task_id.to_string(), session);
        Ok(json!({
            "kind": kind, "id": id, "name": name, "address": shown, "startedHere": started_here,
            "touch": touch, "touchNeeds": touch_needs
        }))
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
        let size = crate::shots::png_size(&picture).filter(|_| ran.ok());
        let Some(size) = size else {
            let said = if ran.timed_out { "it did not answer in time".to_string() } else { ran.complaint() };
            bail!("{} gave no picture of its screen: {said}. Is it still running?", session.name);
        };
        // A touch is told as a part of the screen: it lands by the size of the picture that was seen.
        if let Some(kept) = self.sessions.lock().unwrap().get_mut(task_id)
            && kept.id == session.id
        {
            kept.size = Some(size);
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

    /// Passes a touch on to the device a task shows: a tap or a swipe on its picture, typed text, a button.
    async fn input(&self, task_id: &str, touch: Touch) -> Result<()> {
        let session = self.sessions.lock().unwrap().get(task_id).cloned();
        let session = session.ok_or_else(|| anyhow!("No device is on show for this task. Pick one again."))?;
        let name = session.name.clone();
        // What the core itself will not pass on is said as it is; what the device refused, in its words.
        let ran = match session.kind {
            Kind::Android => self.touch_emulator(&session, &touch).await?,
            Kind::Ios => self.touch_simulator(task_id, &session, &touch).await?,
        };
        if let Some(said) = failure(&Ok(ran)) {
            bail!("{name} did not take that. It said: {said}");
        }
        Ok(())
    }

    async fn touch_emulator(&self, session: &Session, touch: &Touch) -> Result<Ran> {
        let size = || session.size.ok_or_else(|| anyhow!(NO_PICTURE_YET));
        let id = session.id.as_str();
        match touch {
            Touch::Tap { x, y } => {
                let (x, y) = place(size()?, *x, *y);
                self.ask_adb(id, ["shell", "input", "tap", &x.to_string(), &y.to_string()], QUICK).await
            }
            Touch::Swipe { x, y, x2, y2 } => {
                let ((x, y), (x2, y2)) = (place(size()?, *x, *y), place(size()?, *x2, *y2));
                let [x, y, x2, y2] = [x, y, x2, y2].map(|at| at.to_string());
                self.ask_adb(id, ["shell", "input", "swipe", &x, &y, &x2, &y2, SWIPE_MS], QUICK).await
            }
            Touch::Text(text) => self.ask_adb(id, ["shell", "input", "text", &android_text(text)?], QUICK).await,
            Touch::Key(key) => {
                let code = match key {
                    Key::Home => "KEYCODE_HOME",
                    Key::Back => "KEYCODE_BACK",
                    Key::Recent => "KEYCODE_APP_SWITCH",
                    Key::Enter => "KEYCODE_ENTER",
                    Key::Delete => "KEYCODE_DEL",
                };
                self.ask_adb(id, ["shell", "input", "keyevent", code], QUICK).await
            }
        }
    }

    async fn ask_idb<const N: usize>(&self, udid: &str, what: [&str; N], rest: &[String]) -> Result<Ran> {
        let idb = self.idb().ok_or_else(|| anyhow!(NEEDS_IDB))?;
        let mut args = words(what);
        args.extend(words(["--udid", udid]));
        args.extend(rest.iter().map(OsString::from));
        tools::run(&idb, args, self.tools.limit(QUICK)).await
    }

    /// How many pixels of a simulator's picture make one point of its screen. `idb` counts in points.
    async fn points_of(&self, task_id: &str, session: &Session, size: (u32, u32)) -> f64 {
        if let Some(known) = session.scale {
            return known;
        }
        let described = self.ask_idb(&session.id, ["describe"], &["--json".to_string()]).await;
        let long_side = described
            .ok()
            .filter(Ran::ok)
            .and_then(|ran| serde_json::from_str::<Value>(&ran.text()).ok())
            .and_then(|said| {
                let screen = said.get("screen_dimensions")?;
                let side = |name: &str| screen.get(name).and_then(Value::as_f64);
                Some(side("width_points")?.max(side("height_points")?))
            })
            .filter(|points| *points >= 1.0);
        let scale = match long_side {
            Some(points) => f64::from(size.0.max(size.1)) / points,
            // It would not say: an iPad draws two pixels to a point, an iPhone of these years three.
            None if session.name.contains("iPad") => 2.0,
            None => 3.0,
        };
        if let Some(kept) = self.sessions.lock().unwrap().get_mut(task_id)
            && kept.id == session.id
        {
            kept.scale = Some(scale);
        }
        scale
    }

    async fn touch_simulator(&self, task_id: &str, session: &Session, touch: &Touch) -> Result<Ran> {
        if self.idb().is_none() {
            bail!(NEEDS_IDB);
        }
        let id = session.id.as_str();
        let size = || session.size.ok_or_else(|| anyhow!(NO_PICTURE_YET));
        match touch {
            Touch::Tap { x, y } => {
                let scale = self.points_of(task_id, session, size()?).await;
                let (x, y) = place(size()?, *x, *y);
                let at = [x, y].map(|pixel| (f64::from(pixel) / scale).round().to_string());
                self.ask_idb(id, ["ui", "tap"], &at).await
            }
            Touch::Swipe { x, y, x2, y2 } => {
                let scale = self.points_of(task_id, session, size()?).await;
                let ((x, y), (x2, y2)) = (place(size()?, *x, *y), place(size()?, *x2, *y2));
                let mut rest = [x, y, x2, y2].map(|pixel| (f64::from(pixel) / scale).round().to_string()).to_vec();
                rest.extend(["--duration".to_string(), "0.25".to_string()]);
                self.ask_idb(id, ["ui", "swipe"], &rest).await
            }
            Touch::Text(text) => self.ask_idb(id, ["ui", "text"], std::slice::from_ref(text)).await,
            Touch::Key(Key::Home) => self.ask_idb(id, ["ui", "button"], &["HOME".to_string()]).await,
            // The codes of a keyboard: 40 is Return, 42 is the key that deletes backwards.
            Touch::Key(Key::Enter) => self.ask_idb(id, ["ui", "key"], &["40".to_string()]).await,
            Touch::Key(Key::Delete) => self.ask_idb(id, ["ui", "key"], &["42".to_string()]).await,
            Touch::Key(Key::Back | Key::Recent) => bail!("An iPhone has no such button. Swipe on its picture instead."),
        }
    }

    /// Makes the device a task shows light or dark, the way its own settings would.
    async fn scheme(&self, task_id: &str, dark: bool) -> Result<()> {
        let session = self.sessions.lock().unwrap().get(task_id).cloned();
        let session = session.ok_or_else(|| anyhow!("No device is on show for this task. Pick one again."))?;
        let ran = match session.kind {
            Kind::Ios => {
                let look = if dark { "dark" } else { "light" };
                self.ask_xcrun(words(["simctl", "ui", &session.id, "appearance", look]), QUICK).await
            }
            Kind::Android => {
                let night = if dark { "yes" } else { "no" };
                self.ask_adb(&session.id, ["shell", "cmd", "uimode", "night", night], QUICK).await
            }
        };
        if let Some(said) = failure(&ran) {
            bail!("{} did not change how it looks. It said: {said}", session.name);
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
                // Another task still shows this emulator: it stays on until that one lets go too.
                let still_shown = {
                    let mut sessions = self.sessions.lock().unwrap();
                    // One that shows the same app there: the way to the port is now that one's to take back.
                    if session.reversed {
                        let same = |other: &&mut Session| {
                            other.kind == Kind::Android && other.id == session.id && other.port == session.port
                        };
                        if let Some(heir) = sessions.values_mut().find(same) {
                            heir.reversed = true;
                            return;
                        }
                    }
                    sessions.values().any(|other| other.kind == Kind::Android && other.id == session.id)
                };
                // An emulator the core started is the core's to shut down, and with it goes every way to a port.
                if !still_shown && self.started.lock().unwrap().remove(&session.id) {
                    let ran = self.ask_adb(&session.id, ["emu", "kill"], QUICK).await;
                    if let Some(said) = failure(&ran) {
                        tracing::debug!("Could not shut down {}: {said}", session.name);
                    }
                    return;
                }
                if !session.reversed {
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
///   An emulator that is off is listed with the id `avd:<its name>`; started, its id is what adb calls it.
///   `touch` says whether its picture can be tapped from here, and `touchNeeds` what is missing when not.
/// - `devices.input { taskId, what, … }` passes a touch on: `tap { x, y }`, `swipe { x, y, x2, y2 }` (places
///   are parts of the screen from 0 to 1), `text { text }`, `key { key }` with "home", "back", "recent",
///   "enter" or "delete".
/// - `devices.scheme { taskId, scheme }` makes the device "light" or "dark".
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
        "devices.input" => devices.input(task_id, Touch::read(params)?).await.map(|()| json!({})),
        "devices.scheme" => match text("scheme")? {
            "light" => devices.scheme(task_id, false).await.map(|()| json!({})),
            "dark" => devices.scheme(task_id, true).await.map(|()| json!({})),
            _ => bail!("Missing parameter: scheme (light or dark)"),
        },
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

    #[test]
    fn the_names_of_emulators_are_read_without_the_programs_own_news() {
        let printed = "INFO    | Storing crashdata in: /tmp/android/emu-crash.db, detection is enabled\n\
            Pixel_9_API_35\r\nMedium.Tablet-API_34\n\nWARNING | something else\n";
        assert_eq!(parse_avds(printed), vec!["Pixel_9_API_35", "Medium.Tablet-API_34"]);
        assert!(parse_avds("").is_empty());
        assert!(is_avd_name("Pixel_9") && !is_avd_name("") && !is_avd_name("a b") && !is_avd_name("x;reboot"));
    }

    #[test]
    fn a_part_of_the_screen_is_a_pixel_of_its_picture() {
        assert_eq!(place((1080, 2400), 0.0, 0.0), (0, 0));
        assert_eq!(place((1080, 2400), 0.5, 0.25), (540, 600));
        assert_eq!(place((1080, 2400), 1.0, 1.0), (1079, 2399), "the far edge is the last pixel");
        assert_eq!(place((1080, 2400), -3.0, 9.0), (0, 2399), "a place beyond the screen is its edge");
        assert_eq!(place((0, 0), 0.5, 0.5), (0, 0));
    }

    #[test]
    fn a_touch_is_read_whole_or_not_at_all() {
        let read = |params: Value| Touch::read(&params);
        assert_eq!(read(json!({ "what": "tap", "x": 0.5, "y": 1 })).unwrap(), Touch::Tap { x: 0.5, y: 1.0 });
        assert_eq!(
            read(json!({ "what": "swipe", "x": 0.1, "y": 0.2, "x2": 0.3, "y2": 0.4 })).unwrap(),
            Touch::Swipe { x: 0.1, y: 0.2, x2: 0.3, y2: 0.4 }
        );
        assert_eq!(read(json!({ "what": "text", "text": "hi" })).unwrap(), Touch::Text("hi".into()));
        assert_eq!(read(json!({ "what": "key", "key": "recent" })).unwrap(), Touch::Key(Key::Recent));
        assert!(read(json!({ "what": "tap", "x": "0.5", "y": 1 })).is_err(), "a place is a number");
        assert!(read(json!({ "what": "swipe", "x": 0.1, "y": 0.2 })).is_err());
        assert!(read(json!({ "what": "text" })).is_err());
        assert!(read(json!({ "what": "key", "key": "volume" })).is_err());
        assert!(read(json!({})).is_err());
    }

    #[test]
    fn typed_text_for_an_emulator_keeps_its_spaces_and_stays_one_word() {
        assert_eq!(android_text("shop at 5").unwrap(), "'shop%sat%s5'");
        assert_eq!(android_text("a'b; $(x)").unwrap(), r"'a'\''b;%s$(x)'");
        assert!(android_text("привет").is_err());
        assert!(android_text("line\nbreak").is_err(), "Enter is a key of its own");
    }

    #[tokio::test]
    async fn nothing_real_is_looked_for_next_to_a_tool_that_was_named() {
        let named = PathBuf::from("/nonexistent/ae-test/tool");
        let devices = Devices::new(Tools { xcrun: Some(named.clone()), adb: Some(named.clone()), ..Tools::default() });
        assert_eq!(devices.emulator(), None);
        assert_eq!(devices.idb(), None);
        assert_eq!(devices.touch(Kind::Ios), (false, Some(NEEDS_IDB)));
        assert_eq!(devices.touch(Kind::Android), (true, None));
        // One that was named and is not there is not there.
        let devices = Devices::new(Tools { emulator: Some(named.clone()), idb: Some(named), ..Tools::default() });
        assert_eq!((devices.emulator(), devices.idb()), (None, None));
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
