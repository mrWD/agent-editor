//! The iOS simulator and the Android emulator in the Preview.
//!
//! `xcrun`, `adb`, `emulator` and `idb` are stand-ins: shell scripts that
//! answer like the real tools and write down what they were asked. A
//! "simulator" is a line in a file; none is ever started. The tests look at what the core asked of the
//! tools, and above all at what it undid afterwards.
#![cfg(unix)]

mod common;

use ae_core::preview::Previews;
use ae_core::server::{AppState, dispatch};
use ae_core::tools::Tools;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The smallest PNG there is: what the stand-ins print as the screen of a device.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const XCRUN: &str = r#"here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/xcrun.log"
state_of() { cat "$here/$1.state" 2>/dev/null || echo Shutdown; }
case "$1 $2" in
  "simctl list")
    if [ -f "$here/no-simctl" ]; then
      echo 'xcrun: error: unable to find utility "simctl", not a developer tool or in PATH' >&2; exit 72
    fi
    printf '{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-27-0":['
    sep=""
    while IFS='|' read -r udid name; do
      [ -n "$udid" ] || continue
      printf '%s{"udid":"%s","name":"%s","state":"%s","isAvailable":true}' "$sep" "$udid" "$name" "$(state_of "$udid")"
      sep=","
    done < "$here/simulators.txt"
    printf ']}}\n' ;;
  "simctl bootstatus")
    if [ -f "$here/will-not-start" ]; then echo "Unable to boot device in current state: Creating" >&2; exit 149; fi
    echo Booted > "$here/$3.state" ;;
  "simctl shutdown") echo Shutdown > "$here/$3.state" ;;
  "simctl openurl")
    if [ -f "$here/will-not-open" ]; then echo "The request to open the address was denied" >&2; exit 4; fi
    echo "$4" > "$here/$3.url" ;;
  "simctl io")
    [ "$(state_of "$3")" = Booted ] || { echo "Unable to lookup in current state: Shutdown" >&2; exit 149; }
    # Like the real one, it writes the file it is told, even one called "-", and prints no picture.
    cat "$here/screen.png" > "$6"
    echo "Wrote screenshot to: $6" >&2 ;;
  "simctl ui") exit 0 ;;
  "open -a") exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#;

const ADB: &str = r#"here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/adb.log"
serial=""
if [ "$1" = "-s" ]; then serial="$2"; shift 2; fi
case "$1 $2" in
  "devices -l") echo "List of devices attached"; cat "$here/emulators.txt"; echo ;;
  "emu avd") cat "$here/$serial.name" 2>/dev/null || exit 1; echo OK ;;
  "shell getprop")
    if [ "$3" != sys.boot_completed ]; then echo 15; exit 0; fi
    # An emulator that is "still starting" says so a few times, then is ready.
    left=$(cat "$here/$serial.booting" 2>/dev/null || echo 0)
    if [ "$left" -gt 0 ]; then echo $((left - 1)) > "$here/$serial.booting"; echo; else echo 1; fi ;;
  "shell input")
    if [ -f "$here/will-not-touch" ]; then echo "Error: Injecting input events requires the caller to be allowed" >&2; exit 255; fi ;;
  "shell cmd") exit 0 ;;
  "emu kill")
    grep -v "^$serial " "$here/emulators.txt" > "$here/emulators.tmp"
    mv "$here/emulators.tmp" "$here/emulators.txt"
    echo "OK: killing emulator, bye bye" ;;
  "reverse --list") cat "$here/$serial.reverse" 2>/dev/null; exit 0 ;;
  "reverse --remove")
    grep -v " $3 " "$here/$serial.reverse" > "$here/$serial.reverse.tmp"
    mv "$here/$serial.reverse.tmp" "$here/$serial.reverse" ;;
  "reverse tcp:"*) echo "UsbFfs $2 $3" >> "$here/$serial.reverse" ;;
  "shell am")
    if [ -f "$here/will-not-open" ]; then echo "Error: Activity not started, unable to resolve Intent"; exit 0; fi
    for word in "$@"; do last="$word"; done
    echo "$last" > "$here/$serial.url"
    echo "Starting: Intent { act=android.intent.action.VIEW }" ;;
  "exec-out screencap") cat "$here/screen.png" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#;

/// Android's `emulator`: it lists the names in `avds.txt`, with a line of its own news before them as
/// the real one prints, and "starts" one by adding a line to what `adb` lists.
const EMULATOR_TOOL: &str = r#"here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/emulator.log"
case "$1" in
  -list-avds)
    echo "INFO    | Storing crashdata in: /tmp/android/emu-crash.db, detection is enabled for process: 4242"
    cat "$here/avds.txt" 2>/dev/null; exit 0 ;;
  -avd)
    if [ -f "$here/will-not-boot" ]; then exit 1; fi
    echo "emulator-5556          device product:sdk_gphone64 model:sdk_gphone64 transport_id:7" >> "$here/emulators.txt"
    echo "$2" > "$here/emulator-5556.name" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#;

/// `idb`: it says how many points the screen of a simulator has, and takes every touch.
const IDB: &str = r#"here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/idb.log"
case "$1" in
  describe)
    if [ -f "$here/will-not-describe" ]; then echo "no companion" >&2; exit 1; fi
    echo '{"udid":"x","screen_dimensions":{"width":1206,"height":2622,"density":3.0,"width_points":402,"height_points":874}}' ;;
  ui) exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#;

const OFF: &str = "AAAA-1111";
const RUNNING: &str = "BBBB-2222";
const EMULATOR: &str = "emulator-5554";
/// What `adb` calls the emulator the stand-in "starts".
const STARTED: &str = "emulator-5556";
const NEEDS_IDB: &str = "Tapping a simulator from here needs idb on this Mac (brew install facebook/fb/idb-companion, then pipx install fb-idb). Until then, tap and type in the Simulator's own window.";

/// The head of a PNG of that size: all the core reads of a screen before it passes the picture on.
fn screen_of(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    png.extend(width.to_be_bytes());
    png.extend(height.to_be_bytes());
    png.extend([8, 2, 0, 0, 0]);
    png
}

/// A core whose `xcrun`, `adb`, `emulator` and `idb` are the stand-ins, with two simulators (one off,
/// one running), one emulator that runs, none that is off, and a phone on a cable. Returns the folder
/// the stand-ins live in.
fn stand_ins(sandbox: &Sandbox) -> (Arc<Previews>, PathBuf) {
    let tools = sandbox.0.join("tools");
    let xcrun = fake_tool(&tools, "xcrun", XCRUN);
    let adb = fake_tool(&tools, "adb", ADB);
    let emulator = Some(fake_tool(&tools, "emulator", EMULATOR_TOOL));
    let idb = Some(fake_tool(&tools, "idb", IDB));
    std::fs::write(tools.join("screen.png"), base64::engine::general_purpose::STANDARD.decode(PIXEL).unwrap()).unwrap();
    std::fs::write(tools.join("simulators.txt"), format!("{OFF}|iPhone 17 Pro\n{RUNNING}|iPad Air\n")).unwrap();
    std::fs::write(tools.join(format!("{RUNNING}.state")), "Booted\n").unwrap();
    let emulators = format!(
        "{EMULATOR}          device product:sdk_gphone64 model:sdk_gphone64 transport_id:1\n\
         1eaed69e               device usb:1-1 product:phone model:A_phone transport_id:2\n"
    );
    std::fs::write(tools.join("emulators.txt"), emulators).unwrap();
    std::fs::write(tools.join(format!("{EMULATOR}.name")), "Pixel_9_API_35\n").unwrap();
    let browser = Some(PathBuf::from("/nonexistent/ae-test/browser"));
    let previews =
        Previews::with_tools(Tools { browser, xcrun: Some(xcrun), adb: Some(adb), emulator, idb, patience: None });
    (Arc::new(previews), tools)
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

/// A core's state with the stand-ins and one idle task.
async fn core_with_devices(sandbox: &Sandbox) -> (AppState, String, PathBuf) {
    let (previews, tools) = stand_ins(sandbox);
    let mut state = AppState::new(sandbox.manager());
    state.previews = previews;
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &project.id).await;
    (state, task_id, tools)
}

/// What a stand-in was asked so far, one call per entry.
fn asked(tools: &Path, tool: &str) -> Vec<String> {
    let log = std::fs::read_to_string(tools.join(format!("{tool}.log"))).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

fn note(tools: &Path, file: &str) -> String {
    std::fs::read_to_string(tools.join(file)).unwrap_or_default().trim().to_string()
}

async fn open(state: &AppState, task_id: &str, kind: &str, id: &str, address: &str) -> anyhow::Result<Value> {
    dispatch(state, "devices.open", &json!({ "taskId": task_id, "kind": kind, "id": id, "address": address })).await
}

async fn close(state: &AppState, task_id: &str) {
    dispatch(state, "devices.close", &json!({ "taskId": task_id })).await.unwrap();
}

#[tokio::test]
async fn a_simulator_is_listed_started_shown_and_shut_down_again_when_its_tab_is_left() {
    let sandbox = Sandbox::new("devices-ios");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;

    // What runs already comes first. Looking starts nothing.
    let listed = dispatch(&state, "devices.list", &json!({ "taskId": task_id, "kind": "ios" })).await.unwrap();
    assert_eq!(
        listed,
        json!({ "kind": "ios", "needs": null, "devices": [
            { "id": RUNNING, "name": "iPad Air", "system": "iOS 27.0", "state": "running" },
            { "id": OFF, "name": "iPhone 17 Pro", "system": "iOS 27.0", "state": "off" },
        ] })
    );
    assert_eq!(asked(&tools, "xcrun"), vec!["simctl list devices available -j"]);

    // Picking the one that is off starts it and opens the app's own address in it, page and all.
    let opened = open(&state, &task_id, "ios", OFF, "localhost:3000/login?next=/cart&tab=2").await.unwrap();
    assert_eq!(
        opened,
        json!({ "kind": "ios", "id": OFF, "name": "iPhone 17 Pro", "address": "localhost:3000/login?next=/cart&tab=2", "startedHere": true, "touch": true, "touchNeeds": null })
    );
    assert_eq!(note(&tools, &format!("{OFF}.state")), "Booted");
    assert_eq!(note(&tools, &format!("{OFF}.url")), "http://localhost:3000/login?next=/cart&tab=2");
    let calls = asked(&tools, "xcrun");
    assert!(calls.contains(&format!("simctl bootstatus {OFF} -b")), "{calls:?}");

    // Its screen, as a picture. Asked again with the mark of the picture one has, only "the same" comes back.
    let screen = dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(screen["png"], PIXEL);
    assert!(screen["at"].as_u64().is_some_and(|at| at > 0));
    let mark = screen["mark"].as_str().unwrap();
    let again = dispatch(&state, "devices.screen", &json!({ "taskId": task_id, "have": mark })).await.unwrap();
    assert_eq!((again["same"].as_bool(), again["mark"].as_str(), again.get("png")), (Some(true), Some(mark), None));
    // The picture came through a file of the core's own, which is gone again.
    let took: Vec<String> = asked(&tools, "xcrun").into_iter().filter(|call| call.starts_with("simctl io ")).collect();
    let files = sandbox.data().join("tmp").join("screens");
    assert_eq!(took.len(), 2, "{took:?}");
    assert!(took[0].starts_with(&format!("simctl io {OFF} screenshot --type=png {}/", files.display())), "{took:?}");
    assert_eq!(std::fs::read_dir(&files).unwrap().count(), 0, "no picture is kept");

    // The Simulator window is brought forward through the system's own `open`.
    dispatch(&state, "devices.front", &json!({ "taskId": task_id })).await.unwrap();
    assert!(asked(&tools, "xcrun").contains(&format!("open -a Simulator --args -CurrentDeviceUDID {OFF}")));

    // Opened again, as Reload does: the address goes to the simulator once more, and it is not started twice.
    open(&state, &task_id, "ios", OFF, "localhost:3000/cart").await.unwrap();
    assert_eq!(note(&tools, &format!("{OFF}.url")), "http://localhost:3000/cart");
    let calls = asked(&tools, "xcrun");
    assert_eq!(calls.iter().filter(|call| call.starts_with("simctl bootstatus")).count(), 1, "{calls:?}");
    assert_eq!(calls.iter().filter(|call| call.starts_with("simctl shutdown")).count(), 0, "{calls:?}");

    // Leaving the tab shuts down what the core started, and there is no screen to ask for any more.
    close(&state, &task_id).await;
    assert_eq!(note(&tools, &format!("{OFF}.state")), "Shutdown");
    assert!(asked(&tools, "xcrun").contains(&format!("simctl shutdown {OFF}")));
    let error = dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert_eq!(error, "No device is on show for this task. Pick one again.");
    // Leaving twice is the same as leaving once.
    close(&state, &task_id).await;
    assert_eq!(asked(&tools, "xcrun").iter().filter(|call| call.starts_with("simctl shutdown")).count(), 1);
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_simulator_that_was_running_already_is_never_shut_down() {
    let sandbox = Sandbox::new("devices-theirs");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;
    let project = state.manager.projects().unwrap()[0].id.clone();
    let shut_down = || asked(&tools, "xcrun").iter().filter(|call| call.starts_with("simctl shutdown")).count();

    // Left by its tab.
    let opened = open(&state, &task_id, "ios", RUNNING, "127.0.0.1:5173").await.unwrap();
    assert_eq!(opened["startedHere"], false);
    assert_eq!(note(&tools, &format!("{RUNNING}.url")), "http://127.0.0.1:5173/");
    assert!(!asked(&tools, "xcrun").iter().any(|call| call.starts_with("simctl bootstatus")), "nothing was started");
    close(&state, &task_id).await;
    // Left because its task was archived.
    open(&state, &task_id, "ios", RUNNING, "127.0.0.1:5173").await.unwrap();
    dispatch(&state, "tasks.archive", &json!({ "taskId": task_id })).await.unwrap();
    // Left because its window went away.
    let other = idle_task(&state, &project).await;
    let window = state.previews.devices.window();
    let params =
        json!({ "taskId": other, "kind": "ios", "id": RUNNING, "address": "127.0.0.1:5173", "window": window.id() });
    dispatch(&state, "devices.open", &params).await.unwrap();
    drop(window);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Left because the core stops.
    open(&state, &other, "ios", RUNNING, "127.0.0.1:5173").await.unwrap();
    state.previews.close_all().await;

    assert_eq!(shut_down(), 0, "{:?}", asked(&tools, "xcrun"));
    assert_eq!(note(&tools, &format!("{RUNNING}.state")), "Booted");
    state.manager.stop_all().await;
}

#[tokio::test]
async fn archiving_a_task_and_stopping_everything_shut_down_what_the_core_started() {
    let sandbox = Sandbox::new("devices-stop");
    let (state, archived, tools) = core_with_devices(&sandbox).await;
    let project = state.manager.projects().unwrap()[0].id.clone();
    let runs = |udid: &str| note(&tools, &format!("{udid}.state")) == "Booted";

    // Two tasks show the simulator the core started for the first of them.
    let other = idle_task(&state, &project).await;
    assert_eq!(open(&state, &archived, "ios", OFF, "localhost:3000").await.unwrap()["startedHere"], true);
    assert_eq!(open(&state, &other, "ios", OFF, "localhost:3000").await.unwrap()["startedHere"], true);
    // Archiving one of them leaves it running for the other.
    dispatch(&state, "tasks.archive", &json!({ "taskId": archived })).await.unwrap();
    assert!(runs(OFF), "another task still shows it");
    // When the core stops, it shuts down what it started.
    state.previews.close_all().await;
    assert!(!runs(OFF));
    assert!(runs(RUNNING), "what it did not start stays as it was");
    assert_eq!(asked(&tools, "xcrun").iter().filter(|call| call.starts_with("simctl shutdown")).count(), 1);

    // A task that is archived while it alone shows a simulator the core started takes it down with it.
    let alone = idle_task(&state, &project).await;
    open(&state, &alone, "ios", OFF, "localhost:3000").await.unwrap();
    assert!(runs(OFF));
    dispatch(&state, "tasks.archive", &json!({ "taskId": alone })).await.unwrap();
    assert!(!runs(OFF));
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_window_that_goes_away_lets_go_of_the_devices_it_showed() {
    let sandbox = Sandbox::new("devices-window");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;
    let runs = || note(&tools, &format!("{OFF}.state")) == "Booted";

    // The door tells the core which window asks. That window is closed, or loaded anew.
    let window = state.previews.devices.window();
    let params =
        json!({ "taskId": task_id, "kind": "ios", "id": OFF, "address": "localhost:3000", "window": window.id() });
    dispatch(&state, "devices.open", &params).await.unwrap();
    assert!(runs());
    drop(window);
    let mut down = false;
    for _ in 0..200 {
        down = !runs();
        if down {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(down, "the simulator the core started for that window is shut down");

    // A window that is gone before the simulator has started gets nothing left running in its name.
    let gone = state.previews.devices.window();
    let id = gone.id();
    drop(gone);
    let params = json!({ "taskId": task_id, "kind": "ios", "id": OFF, "address": "localhost:3000", "window": id });
    let error = dispatch(&state, "devices.open", &params).await.unwrap_err().to_string();
    assert!(error.contains("is gone"), "{error}");
    assert!(!runs());
    state.manager.stop_all().await;
}

#[tokio::test]
async fn an_emulator_is_listed_given_a_way_to_the_app_shown_and_left_as_it_was() {
    let sandbox = Sandbox::new("devices-android");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;

    // The emulator under the name it was given; the phone on a cable is not one.
    let listed = dispatch(&state, "devices.list", &json!({ "taskId": task_id, "kind": "android" })).await.unwrap();
    assert_eq!(
        listed,
        json!({ "kind": "android", "needs": null, "devices": [
            { "id": EMULATOR, "name": "Pixel 9 API 35", "system": "Android 15", "state": "running" },
        ] })
    );

    // The port of the app is passed on from the emulator to this computer, and the address is opened
    // there as one word, whatever it holds.
    let opened = open(&state, &task_id, "android", EMULATOR, "http://localhost:3000/login?a=1&b=2;c").await.unwrap();
    assert_eq!(
        opened,
        json!({ "kind": "android", "id": EMULATOR, "name": "Pixel 9 API 35", "address": "localhost:3000/login?a=1&b=2;c", "startedHere": false, "touch": true, "touchNeeds": null })
    );
    let calls = asked(&tools, "adb");
    assert!(calls.contains(&format!("-s {EMULATOR} reverse tcp:3000 tcp:3000")), "{calls:?}");
    let view = format!(
        "-s {EMULATOR} shell am start -a android.intent.action.VIEW -d 'http://localhost:3000/login?a=1&b=2;c'"
    );
    assert!(calls.contains(&view), "{calls:?}");
    assert_eq!(note(&tools, &format!("{EMULATOR}.reverse")), "UsbFfs tcp:3000 tcp:3000");

    let screen = dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(screen["png"], PIXEL);
    assert!(asked(&tools, "adb").contains(&format!("-s {EMULATOR} exec-out screencap -p")));
    let error = dispatch(&state, "devices.front", &json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert!(error.contains("cannot be brought forward from here"), "{error}");

    // Opened again: the way to the port is there already and is not made twice.
    open(&state, &task_id, "android", EMULATOR, "localhost:3000").await.unwrap();
    assert_eq!(asked(&tools, "adb").iter().filter(|call| call.ends_with("reverse tcp:3000 tcp:3000")).count(), 1);
    // Another app, on another port: the old way is closed and a new one made.
    open(&state, &task_id, "android", EMULATOR, "localhost:4000").await.unwrap();
    assert_eq!(note(&tools, &format!("{EMULATOR}.reverse")), "UsbFfs tcp:4000 tcp:4000");

    // Leaving the tab takes back what the core made.
    close(&state, &task_id).await;
    assert_eq!(note(&tools, &format!("{EMULATOR}.reverse")), "");
    assert!(asked(&tools, "adb").contains(&format!("-s {EMULATOR} reverse --remove tcp:4000")));

    // A way to the port that was there before the tab is somebody else's: it is used, and it stays.
    std::fs::write(tools.join(format!("{EMULATOR}.reverse")), "UsbFfs tcp:8081 tcp:8081\n").unwrap();
    open(&state, &task_id, "android", EMULATOR, "localhost:8081").await.unwrap();
    close(&state, &task_id).await;
    assert_eq!(note(&tools, &format!("{EMULATOR}.reverse")), "UsbFfs tcp:8081 tcp:8081");
    assert!(!asked(&tools, "adb").iter().any(|call| call.contains("reverse --remove tcp:8081")));
    state.manager.stop_all().await;
}

#[tokio::test]
async fn what_is_missing_is_said_in_one_sentence_and_no_screen_is_made_up() {
    let sandbox = Sandbox::new("devices-missing");
    let (mut state, task_id, tools) = core_with_devices(&sandbox).await;
    let needs = |state: AppState, kind: &'static str| {
        let params = json!({ "taskId": task_id, "kind": kind });
        async move {
            let listed = dispatch(&state, "devices.list", &params).await.unwrap();
            assert_eq!(listed["devices"], json!([]));
            listed["needs"].as_str().unwrap().to_string()
        }
    };

    // Xcode is there and has no simulator; adb is there, no emulator runs (the phone on a cable does
    // not count) and the Device Manager has none to start.
    std::fs::write(tools.join("simulators.txt"), "").unwrap();
    std::fs::write(tools.join("emulators.txt"), "1eaed69e    device usb:1-1 model:A_phone transport_id:2\n").unwrap();
    assert_eq!(
        needs(state.clone(), "ios").await,
        "Xcode has no iOS simulator yet: add one in Xcode under Window, Devices and Simulators."
    );
    assert_eq!(
        needs(state.clone(), "android").await,
        "Android has no emulator yet: make one in the Device Manager of Android Studio."
    );
    // Only Xcode's command line tools: `xcrun` is there, the simulators are not.
    std::fs::write(tools.join("no-simctl"), "").unwrap();
    let said = needs(state.clone(), "ios").await;
    assert!(said.starts_with("The iOS simulator needs Xcode on this Mac"), "{said}");
    assert!(said.contains("unable to find utility \"simctl\""), "{said}");

    // Nothing is shown for a device that is not there, and none is opened.
    let error = open(&state, &task_id, "android", EMULATOR, "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(error, "Android has no emulator yet: make one in the Device Manager of Android Studio.");
    let error = dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert_eq!(error, "No device is on show for this task. Pick one again.");
    // Nothing is touched either, and nothing made light or dark.
    let tap = json!({ "taskId": task_id, "what": "tap", "x": 0.5, "y": 0.5 });
    let error = dispatch(&state, "devices.input", &tap).await.unwrap_err().to_string();
    assert_eq!(error, "No device is on show for this task. Pick one again.");
    let dark = json!({ "taskId": task_id, "scheme": "dark" });
    let error = dispatch(&state, "devices.scheme", &dark).await.unwrap_err().to_string();
    assert_eq!(error, "No device is on show for this task. Pick one again.");

    // adb alone, without the program that starts emulators: only the ones that run could be shown.
    let adb_only = Tools {
        xcrun: Some(tools.join("xcrun")),
        adb: Some(tools.join("adb")),
        emulator: Some(PathBuf::from("/nonexistent/ae-test/emulator")),
        ..Tools::default()
    };
    state.previews = Arc::new(Previews::with_tools(adb_only));
    assert_eq!(
        needs(state.clone(), "android").await,
        "No Android emulator is running: start one from the Device Manager of Android Studio."
    );

    // The tools themselves are not there.
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    state.previews =
        Arc::new(Previews::with_tools(Tools { xcrun: Some(missing.clone()), adb: Some(missing), ..Tools::default() }));
    assert_eq!(
        needs(state.clone(), "ios").await,
        "The iOS simulator needs Xcode on this Mac: install it from the App Store and open it once."
    );
    assert_eq!(
        needs(state.clone(), "android").await,
        "The Android emulator needs adb, which comes with Android Studio (SDK Platform-Tools in its SDK Manager)."
    );
    let other = json!({ "taskId": task_id, "kind": "watch" });
    assert!(dispatch(&state, "devices.list", &other).await.is_err());
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_device_that_will_not_open_the_address_is_told_and_nothing_stays_running_for_it() {
    let sandbox = Sandbox::new("devices-refused");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;
    let runs = || note(&tools, &format!("{OFF}.state")) == "Booted";

    // An address that is not on this computer is refused before any tool is asked.
    let error = open(&state, &task_id, "ios", OFF, "example.com/login").await.unwrap_err().to_string();
    assert!(error.contains("Only an app on this computer can be shown"), "{error}");
    assert_eq!(asked(&tools, "xcrun"), Vec::<String>::new());
    // A simulator that is not in the list.
    let error = open(&state, &task_id, "ios", "CCCC-3333", "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(error, "That device is not there any more. Look again for the ones that are.");

    // A simulator that does not start.
    std::fs::write(tools.join("will-not-start"), "").unwrap();
    let error = open(&state, &task_id, "ios", OFF, "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(error, "iPhone 17 Pro did not start. xcrun said: Unable to boot device in current state: Creating");
    std::fs::remove_file(tools.join("will-not-start")).unwrap();

    // One that starts and then will not open the address is shut down again: the tab shows nothing of it.
    std::fs::write(tools.join("will-not-open"), "").unwrap();
    let error = open(&state, &task_id, "ios", OFF, "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(
        error,
        "iPhone 17 Pro did not open localhost:3000. xcrun said: The request to open the address was denied"
    );
    assert!(!runs(), "what was started for a tab that shows nothing is shut down");
    // The emulator says its refusal in words only; the way to the port is taken back.
    let error = open(&state, &task_id, "android", EMULATOR, "localhost:3000").await.unwrap_err().to_string();
    assert!(error.contains("did not open localhost:3000. adb said: Error: Activity not started"), "{error}");
    assert_eq!(note(&tools, &format!("{EMULATOR}.reverse")), "");
    std::fs::remove_file(tools.join("will-not-open")).unwrap();

    // A simulator that is shut down by somebody else while it is on show: its picture is not kept up.
    open(&state, &task_id, "ios", OFF, "localhost:3000").await.unwrap();
    std::fs::write(tools.join(format!("{OFF}.state")), "Shutdown\n").unwrap();
    let error = dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert_eq!(
        error,
        "iPhone 17 Pro gave no picture of its screen: Unable to lookup in current state: Shutdown. Is it still running?"
    );
    state.manager.stop_all().await;
}

#[tokio::test]
async fn an_emulator_that_is_off_is_started_from_here_and_shut_down_again_when_its_tab_is_left() {
    let sandbox = Sandbox::new("devices-avd");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;
    std::fs::write(tools.join("avds.txt"), "Pixel_9_API_35\nTablet_API_34\n").unwrap();
    let off = "avd:Tablet_API_34";

    // The one that runs, then the one the Device Manager knows and nobody started. The line of the
    // program's own news is not a device.
    let listed = dispatch(&state, "devices.list", &json!({ "taskId": task_id, "kind": "android" })).await.unwrap();
    assert_eq!(
        listed["devices"],
        json!([
            { "id": EMULATOR, "name": "Pixel 9 API 35", "system": "Android 15", "state": "running" },
            { "id": off, "name": "Tablet API 34", "system": "Android", "state": "off" },
        ])
    );
    assert_eq!(asked(&tools, "emulator"), vec!["-list-avds"], "looking starts nothing");

    // Picking it starts it, waits for Android in it, and goes on as with one that ran: from then on
    // it is the serial adb gave it.
    let opened = open(&state, &task_id, "android", off, "localhost:3000").await.unwrap();
    assert_eq!(
        opened,
        json!({ "kind": "android", "id": STARTED, "name": "Tablet API 34", "address": "localhost:3000", "startedHere": true, "touch": true, "touchNeeds": null })
    );
    assert!(asked(&tools, "emulator").contains(&"-avd Tablet_API_34".to_string()));
    let calls = asked(&tools, "adb");
    assert!(calls.contains(&format!("-s {STARTED} shell getprop sys.boot_completed")), "{calls:?}");
    assert!(calls.contains(&format!("-s {STARTED} reverse tcp:3000 tcp:3000")), "{calls:?}");

    // Asked for by its name again, as Reload does: it runs by now and is not started twice.
    open(&state, &task_id, "android", off, "localhost:3000/cart").await.unwrap();
    assert_eq!(asked(&tools, "emulator").iter().filter(|call| call.starts_with("-avd")).count(), 1);
    assert_eq!(note(&tools, &format!("{STARTED}.url")), "'http://localhost:3000/cart'");

    // Another task shows the same emulator: it stays on until both have let go.
    let project = state.manager.projects().unwrap().remove(0);
    let other = idle_task(&state, &project.id).await;
    assert_eq!(open(&state, &other, "android", STARTED, "localhost:3000").await.unwrap()["startedHere"], true);
    close(&state, &task_id).await;
    assert!(!asked(&tools, "adb").iter().any(|call| call.ends_with("emu kill")));
    close(&state, &other).await;
    assert!(asked(&tools, "adb").contains(&format!("-s {STARTED} emu kill")));
    assert!(!note(&tools, "emulators.txt").contains(STARTED));
    // The one that ran before the tab was never the core's to shut down.
    open(&state, &task_id, "android", EMULATOR, "localhost:3000").await.unwrap();
    close(&state, &task_id).await;
    assert!(!asked(&tools, "adb").contains(&format!("-s {EMULATOR} emu kill")));

    // One that somebody else started a moment ago and that is not ready yet: it is waited for, not
    // started a second time, and it is theirs, so it stays on.
    std::fs::write(tools.join("emulators.txt"), format!("{STARTED}   device product:sdk model:sdk transport_id:7\n"))
        .unwrap();
    std::fs::write(tools.join(format!("{STARTED}.booting")), "1\n").unwrap();
    let before = asked(&tools, "emulator").iter().filter(|call| call.starts_with("-avd")).count();
    let opened = open(&state, &task_id, "android", off, "localhost:3000").await.unwrap();
    assert_eq!((opened["id"].as_str(), opened["startedHere"].as_bool()), (Some(STARTED), Some(false)));
    assert_eq!(asked(&tools, "emulator").iter().filter(|call| call.starts_with("-avd")).count(), before);
    let kills = || asked(&tools, "adb").iter().filter(|call| call.ends_with("emu kill")).count();
    let killed = kills();
    close(&state, &task_id).await;
    assert_eq!(kills(), killed);
    std::fs::write(tools.join("emulators.txt"), "").unwrap();

    // One whose program ends at once is not waited for, and a name nobody could have listed is refused.
    std::fs::write(tools.join("will-not-boot"), "").unwrap();
    let error = open(&state, &task_id, "android", off, "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(
        error,
        "Tablet API 34 did not start: the emulator's program ended with code 1. Start it from the Device Manager of Android Studio to see why."
    );
    let error = open(&state, &task_id, "android", "avd:x; reboot", "localhost:3000").await.unwrap_err().to_string();
    assert_eq!(error, "That device is not there any more. Look again for the ones that are.");
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_touch_on_the_picture_lands_on_the_emulator_where_it_was_made() {
    let sandbox = Sandbox::new("devices-touch-android");
    let (state, task_id, tools) = core_with_devices(&sandbox).await;
    std::fs::write(tools.join("screen.png"), screen_of(1080, 2400)).unwrap();
    open(&state, &task_id, "android", EMULATOR, "localhost:3000").await.unwrap();
    let input = |what: Value| {
        let mut params = what;
        params["taskId"] = json!(task_id);
        let state = state.clone();
        async move { dispatch(&state, "devices.input", &params).await.map_err(|error| error.to_string()) }
    };
    let did = |call: &str| asked(&tools, "adb").contains(&format!("-s {EMULATOR} shell {call}"));

    // Where a touch lands is counted from the picture that was seen: before the first one, nothing is touched.
    let early = input(json!({ "what": "tap", "x": 0.5, "y": 0.25 })).await.unwrap_err();
    assert_eq!(early, "Wait for the first picture of its screen.");
    dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();

    input(json!({ "what": "tap", "x": 0.5, "y": 0.25 })).await.unwrap();
    assert!(did("input tap 540 600"), "{:?}", asked(&tools, "adb"));
    // The very edge is still on the screen, and a place beyond it is the edge.
    input(json!({ "what": "tap", "x": 1.0, "y": 7.0 })).await.unwrap();
    assert!(did("input tap 1079 2399"));
    input(json!({ "what": "swipe", "x": 0.5, "y": 0.75, "x2": 0.5, "y2": 0.25 })).await.unwrap();
    assert!(did("input swipe 540 1800 540 600 250"));

    // Typed text is one word for the shell of the device, with its spaces written the way `input` reads them.
    input(json!({ "what": "text", "text": "it's 5 o'clock; ok" })).await.unwrap();
    assert!(did(r"input text 'it'\''s%s5%so'\''clock;%sok'"), "{:?}", asked(&tools, "adb"));
    let cyrillic = input(json!({ "what": "text", "text": "привет" })).await.unwrap_err();
    assert!(cyrillic.starts_with("An emulator takes only plain Latin letters"), "{cyrillic}");
    assert!(input(json!({ "what": "text", "text": "" })).await.is_err());
    assert!(input(json!({ "what": "text", "text": "a".repeat(501) })).await.unwrap_err().contains("in parts"));

    for (key, code) in [
        ("home", "KEYCODE_HOME"),
        ("back", "KEYCODE_BACK"),
        ("recent", "KEYCODE_APP_SWITCH"),
        ("enter", "KEYCODE_ENTER"),
        ("delete", "KEYCODE_DEL"),
    ] {
        input(json!({ "what": "key", "key": key })).await.unwrap();
        assert!(did(&format!("input keyevent {code}")), "{key}");
    }
    assert!(input(json!({ "what": "key", "key": "power" })).await.is_err());
    assert!(input(json!({ "what": "shake" })).await.is_err());
    assert!(input(json!({ "what": "tap", "x": 0.5 })).await.is_err());

    // Light and dark are the emulator's own setting.
    dispatch(&state, "devices.scheme", &json!({ "taskId": task_id, "scheme": "dark" })).await.unwrap();
    assert!(did("cmd uimode night yes"));
    dispatch(&state, "devices.scheme", &json!({ "taskId": task_id, "scheme": "light" })).await.unwrap();
    assert!(did("cmd uimode night no"));
    assert!(dispatch(&state, "devices.scheme", &json!({ "taskId": task_id, "scheme": "sepia" })).await.is_err());

    // What the device refuses is said in its own words.
    std::fs::write(tools.join("will-not-touch"), "").unwrap();
    let refused = input(json!({ "what": "key", "key": "home" })).await.unwrap_err();
    assert_eq!(
        refused,
        "Pixel 9 API 35 did not take that. It said: Error: Injecting input events requires the caller to be allowed"
    );
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_simulator_is_touched_through_idb_in_points_and_says_what_it_needs_without_it() {
    let sandbox = Sandbox::new("devices-touch-ios");
    let (mut state, task_id, tools) = core_with_devices(&sandbox).await;
    std::fs::write(tools.join("screen.png"), screen_of(1206, 2622)).unwrap();
    open(&state, &task_id, "ios", RUNNING, "localhost:3000").await.unwrap();
    dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();
    let input = |state: AppState, what: Value| {
        let mut params = what;
        params["taskId"] = json!(task_id);
        async move { dispatch(&state, "devices.input", &params).await.map_err(|error| error.to_string()) }
    };
    let did = |call: &str| asked(&tools, "idb").contains(&call.to_string());

    // The picture has three pixels to a point, and idb counts in points. It is asked for that once.
    input(state.clone(), json!({ "what": "tap", "x": 0.5, "y": 0.5 })).await.unwrap();
    assert!(did(&format!("ui tap --udid {RUNNING} 201 437")), "{:?}", asked(&tools, "idb"));
    input(state.clone(), json!({ "what": "swipe", "x": 0.5, "y": 0.75, "x2": 0.5, "y2": 0.25 })).await.unwrap();
    assert!(did(&format!("ui swipe --udid {RUNNING} 201 655 201 218 --duration 0.25")), "{:?}", asked(&tools, "idb"));
    assert_eq!(asked(&tools, "idb").iter().filter(|call| call.starts_with("describe")).count(), 1);
    // Text goes as it is, in any letters; the keys go by their codes; an iPhone has no Back.
    input(state.clone(), json!({ "what": "text", "text": "привет, it's me" })).await.unwrap();
    assert!(did(&format!("ui text --udid {RUNNING} привет, it's me")));
    input(state.clone(), json!({ "what": "key", "key": "home" })).await.unwrap();
    input(state.clone(), json!({ "what": "key", "key": "enter" })).await.unwrap();
    input(state.clone(), json!({ "what": "key", "key": "delete" })).await.unwrap();
    assert!(did(&format!("ui button --udid {RUNNING} HOME")));
    assert!(did(&format!("ui key --udid {RUNNING} 40")) && did(&format!("ui key --udid {RUNNING} 42")));
    let back = input(state.clone(), json!({ "what": "key", "key": "back" })).await.unwrap_err();
    assert_eq!(back, "An iPhone has no such button. Swipe on its picture instead.");

    dispatch(&state, "devices.scheme", &json!({ "taskId": task_id, "scheme": "dark" })).await.unwrap();
    assert!(asked(&tools, "xcrun").contains(&format!("simctl ui {RUNNING} appearance dark")));
    close(&state, &task_id).await;

    // An iPad that will not say how many points it has draws two pixels to a point.
    std::fs::write(tools.join("will-not-describe"), "").unwrap();
    std::fs::write(tools.join("screen.png"), screen_of(1640, 2360)).unwrap();
    open(&state, &task_id, "ios", RUNNING, "localhost:3000").await.unwrap();
    dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();
    input(state.clone(), json!({ "what": "tap", "x": 0.5, "y": 0.5 })).await.unwrap();
    assert!(did(&format!("ui tap --udid {RUNNING} 410 590")), "{:?}", asked(&tools, "idb"));
    close(&state, &task_id).await;

    // A Mac without idb: the tab is told so when the simulator is opened, and a touch says the same.
    let before = asked(&tools, "idb").len();
    let without = Tools { xcrun: Some(tools.join("xcrun")), adb: Some(tools.join("adb")), ..Tools::default() };
    state.previews = Arc::new(Previews::with_tools(without));
    let opened = open(&state, &task_id, "ios", RUNNING, "localhost:3000").await.unwrap();
    assert_eq!((opened["touch"].as_bool(), opened["touchNeeds"].as_str()), (Some(false), Some(NEEDS_IDB)));
    dispatch(&state, "devices.screen", &json!({ "taskId": task_id })).await.unwrap();
    let error = input(state.clone(), json!({ "what": "tap", "x": 0.5, "y": 0.5 })).await.unwrap_err();
    assert_eq!(error, NEEDS_IDB);
    assert_eq!(asked(&tools, "idb").len(), before, "the real idb is never looked for next to a stand-in");
    state.manager.stop_all().await;
}
