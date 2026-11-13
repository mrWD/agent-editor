//! Screenshots of the previewed app, taken by a browser the core runs.
//!
//! The browser is a stand-in: a shell script that does what the real one does
//! as far as the core can see. It asks the relay for the page it was sent to
//! (and keeps it, so the test can read it), asks for the wait that page asks
//! for once the app's page has loaded, and writes a picture where it was told
//! to. No real browser is started.
#![cfg(unix)]

mod common;

use ae_core::preview::Previews;
use ae_core::server::{AppState, dispatch};
use ae_core::tools::Tools;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the stand-in does, in the order the real browser does it.
const BROWSER: &str = r#"here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/browser.log"
for word in "$@"; do
  case "$word" in
    --screenshot=*) file="${word#--screenshot=}" ;;
    --window-size=*) size="${word#--window-size=}" ;;
    http://*) url="$word" ;;
  esac
done
width="${size%%,*}"
# The page it was sent to, kept for the test to read.
curl -s "$url" > "$here/page-$width.html"
# Once the app's page has loaded, the page asks for a wait and says how high the window is inside:
# the window less the browser's own bars, or whatever the test says.
inside=$(( ${size##*,} - 87 ))
[ -f "$here/low" ] && inside=$(cat "$here/low")
[ -f "$here/never-loads" ] || curl -s -o /dev/null "$url/settle?h=$inside"
if [ -f "$here/hangs" ]; then sleep 600 & echo $! > "$here/helper.pid"; wait; fi
if [ -f "$here/fails" ]; then echo "The stand-in was told to fail" >&2; exit 1; fi
cp "$here/picture.png" "$file"
"#;

/// A PNG of one colour, `width` by `height`.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut rows = Vec::new();
    for _ in 0..height {
        rows.push(0);
        rows.extend(std::iter::repeat_n([245u8, 181, 68], width as usize).flatten());
    }
    let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    packer.write_all(&rows).unwrap();
    let mut head = Vec::new();
    head.extend_from_slice(&width.to_be_bytes());
    head.extend_from_slice(&height.to_be_bytes());
    head.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, body) in [(b"IHDR", head), (b"IDAT", packer.finish().unwrap()), (b"IEND", vec![])] {
        let mut check = flate2::Crc::new();
        check.update(kind);
        check.update(&body);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(&body);
        out.extend_from_slice(&check.sum().to_be_bytes());
    }
    out
}

/// The width and height a PNG says it has.
fn size_of(png: &[u8]) -> (u32, u32) {
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "a PNG");
    let number = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (number(16), number(20))
}

/// An "app" that answers every request with one small page. Returns its port.
async fn start_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let _ = socket.read(&mut request).await;
                let page = "<!doctype html><html><head><title>Shop</title></head><body>Sign in</body></html>";
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            });
        }
    });
    port
}

/// A core whose only browser is the stand-in, one idle task, and the folder the stand-in lives in.
async fn core_with_browser(sandbox: &Sandbox, patience: Option<Duration>) -> (AppState, String, PathBuf) {
    let tools = sandbox.0.join("tools");
    let browser = fake_tool(&tools, "browser", BROWSER);
    // Higher than any picture asked for here, as a real window is: the core cuts it to the frame.
    std::fs::write(tools.join("picture.png"), png(8, 1200)).unwrap();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let mut state = AppState::new(sandbox.manager());
    state.previews = Arc::new(Previews::with_tools(Tools {
        browser: Some(browser),
        xcrun: Some(missing.clone()),
        adb: Some(missing),
        patience,
        ..Tools::default()
    }));
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = state.manager.subscribe();
    let task = state.manager.create(&project.id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (state, task.row.id, tools)
}

/// Shows an app in the task's Preview, so there is a relay to take pictures through. Returns the relay's port.
async fn show_app(state: &AppState, task_id: &str) -> u16 {
    let app = start_app().await;
    let params = json!({ "taskId": task_id, "target": format!("localhost:{app}"), "origin": "http://127.0.0.1:4999" });
    dispatch(state, "preview.open", &params).await.unwrap()["port"].as_u64().unwrap() as u16
}

fn calls(tools: &Path) -> Vec<String> {
    std::fs::read_to_string(tools.join("browser.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

/// Whatever is still in the folder the core keeps the files of a run in.
fn left_behind(sandbox: &Sandbox) -> Vec<PathBuf> {
    let dir = sandbox.data().join("tmp").join("shots");
    std::fs::read_dir(dir).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default()
}

async fn status_of(port: u16, path: &str) -> u16 {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut answer)).await;
    String::from_utf8_lossy(&answer).split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0)
}

#[tokio::test]
async fn a_picture_is_taken_at_each_width_through_the_tasks_relay_and_cut_to_the_frame() {
    let sandbox = Sandbox::new("shots");
    let (state, task_id, tools) = core_with_browser(&sandbox, None).await;
    let said = dispatch(&state, "shots.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(said, json!({ "browser": "the browser", "needs": null }));
    let relay = show_app(&state, &task_id).await;

    let asked = json!({ "taskId": task_id, "shots": [
        { "width": 320, "height": 700, "theme": "dark", "path": "/login?next=/cart&tab=2" },
        { "width": 1024, "height": 600, "theme": "light", "path": "/" },
    ] });
    let taken = dispatch(&state, "shots.take", &asked).await.unwrap();
    assert_eq!(taken["browser"], "the browser");
    let shots = taken["shots"].as_array().unwrap();
    assert_eq!(shots.len(), 2);
    // In the order asked, each cut from the window's height down to the frame's.
    for (shot, (width, height, theme)) in shots.iter().zip([(320, 700, "dark"), (1024, 600, "light")]) {
        assert_eq!((shot["width"].as_u64(), shot["theme"].as_str()), (Some(width), Some(theme)));
        assert_eq!(shot["mimeType"], "image/png");
        assert_eq!(shot["stillLoading"], false);
        let bytes = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap()).unwrap();
        assert_eq!(shot["size"].as_u64(), Some(bytes.len() as u64));
        assert_eq!(size_of(&bytes), (8, height), "the picture is as high as the frame, not as the window");
        assert_eq!(shot["pixels"], json!({ "width": 8, "height": height }));
    }

    // The browser was run once for each: without a window, at the frame's width and a window higher than
    // the frame, in the theme asked, with a profile of its own, and sent to a page of the task's relay.
    let log = calls(&tools);
    assert_eq!(log.len(), 2, "{log:?}");
    let phone = log.iter().find(|call| call.contains("--window-size=320,940")).expect("the phone's run");
    for word in ["--headless=new", "--hide-scrollbars", "--blink-settings=preferredColorScheme=0", "--timeout=15000"] {
        assert!(phone.split(' ').any(|said| said == word), "{word} is missing from: {phone}");
    }
    let data = sandbox.data().to_string_lossy().into_owned();
    assert!(phone.contains(&format!("--screenshot={data}/tmp/shots/")), "{phone}");
    assert!(phone.contains(&format!("--user-data-dir={data}/tmp/shots/")), "{phone}");
    let address = phone.split(' ').next_back().unwrap();
    assert!(address.starts_with(&format!("http://127.0.0.1:{relay}/__ae_preview/shot/")), "{address}");
    let laptop = log.iter().find(|call| call.contains("--window-size=1024,840")).expect("the laptop's run");
    assert!(laptop.contains("--blink-settings=preferredColorScheme=1"), "{laptop}");

    // The page it was sent to holds the app's page, the one asked for, in a frame of exactly the size asked.
    let page = std::fs::read_to_string(tools.join("page-320.html")).unwrap();
    assert!(page.contains("width:320px;height:700px"), "{page}");
    assert!(page.contains("<iframe id=\"page\" src=\"/login?next=/cart&amp;tab=2\""), "{page}");
    assert!(std::fs::read_to_string(tools.join("page-1024.html")).unwrap().contains("width:1024px;height:600px"));

    // Afterwards nothing is left: no file of the run, and the relay no longer serves the page.
    assert_eq!(left_behind(&sandbox), Vec::<PathBuf>::new());
    let path = address.strip_prefix(&format!("http://127.0.0.1:{relay}")).unwrap();
    assert_eq!(status_of(relay, path).await, 404, "the page for a screenshot is gone with the screenshot");
    assert_eq!(status_of(relay, "/__ae_preview/shot/0123456789abcdef0123456789abcdef").await, 404);
    // The app itself is still shown through the relay as before.
    assert_eq!(status_of(relay, "/").await, 200);
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_page_that_never_finishes_loading_is_told_as_such_and_a_frame_out_of_view_gives_no_picture() {
    let sandbox = Sandbox::new("shots-loading");
    let (state, task_id, tools) = core_with_browser(&sandbox, None).await;
    show_app(&state, &task_id).await;
    let one = json!({ "taskId": task_id, "shots": [{ "width": 768, "height": 500, "theme": "light", "path": "/" }] });

    // The app's page never said it had loaded: the browser gave up waiting and took what there was.
    std::fs::write(tools.join("never-loads"), "").unwrap();
    let taken = dispatch(&state, "shots.take", &one).await.unwrap();
    assert_eq!(taken["shots"][0]["stillLoading"], true);
    std::fs::remove_file(tools.join("never-loads")).unwrap();

    // A window too low inside to hold the frame: its lower part may not have been drawn, so no picture is given.
    std::fs::write(tools.join("low"), "420").unwrap();
    let error = dispatch(&state, "shots.take", &one).await.unwrap_err().to_string();
    assert!(error.contains("was too low for a picture 500 pixels high, so none was taken"), "{error}");
    assert_eq!(left_behind(&sandbox), Vec::<PathBuf>::new());
    state.manager.stop_all().await;
}

#[tokio::test]
async fn a_browser_that_fails_or_hangs_is_told_in_words_and_leaves_nothing_running() {
    let sandbox = Sandbox::new("shots-fail");
    let (state, task_id, tools) = core_with_browser(&sandbox, Some(Duration::from_secs(4))).await;
    show_app(&state, &task_id).await;
    let one = json!({ "taskId": task_id, "shots": [{ "width": 320, "height": 700, "theme": "light", "path": "/" }] });

    std::fs::write(tools.join("fails"), "").unwrap();
    let error = dispatch(&state, "shots.take", &one).await.unwrap_err().to_string();
    assert_eq!(
        error,
        "the browser made no picture of the page 320 pixels wide. It said: The stand-in was told to fail"
    );
    std::fs::remove_file(tools.join("fails")).unwrap();

    // A browser that never ends is stopped when its time is up, with the helper it started.
    std::fs::write(tools.join("hangs"), "").unwrap();
    let started = std::time::Instant::now();
    let error = dispatch(&state, "shots.take", &one).await.unwrap_err().to_string();
    assert_eq!(
        error,
        "the browser took too long over the page 320 pixels wide and was stopped. No screenshot was taken."
    );
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    let helper: i32 = std::fs::read_to_string(tools.join("helper.pid")).unwrap().trim().parse().unwrap();
    let mut gone = false;
    for _ in 0..100 {
        // SAFETY: signal 0 sends nothing; it only asks whether the process is there.
        gone = unsafe { libc::kill(helper, 0) } != 0;
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "what the browser started is stopped with it");
    assert_eq!(left_behind(&sandbox), Vec::<PathBuf>::new(), "a failed run leaves no files either");
    state.manager.stop_all().await;
}

#[tokio::test]
async fn what_cannot_be_photographed_is_refused_before_any_browser_is_started() {
    let sandbox = Sandbox::new("shots-refused");
    let (mut state, task_id, tools) = core_with_browser(&sandbox, None).await;
    let shot = |width: u64, path: &str| json!({ "width": width, "height": 700, "theme": "light", "path": path });
    let refused = |state: AppState, shots: Vec<Value>| {
        let params = json!({ "taskId": task_id, "shots": shots });
        async move { dispatch(&state, "shots.take", &params).await.unwrap_err().to_string() }
    };

    // No app is shown for the task yet: there is no page to take a picture of.
    let error = refused(state.clone(), vec![shot(320, "/")]).await;
    assert_eq!(error, "Show the app in the Preview first: there is no page to take a picture of.");
    show_app(&state, &task_id).await;

    // Only a page of the app, at a size the Preview has, and no more pictures than a message takes.
    assert!(refused(state.clone(), vec![shot(320, "//example.com/")]).await.contains("is not a page of the app"));
    assert!(refused(state.clone(), vec![shot(320, "javascript:alert(1)")]).await.contains("is not a page of the app"));
    assert!(refused(state.clone(), vec![shot(90, "/")]).await.contains("width from 200 to 3840"));
    let six = (0..6).map(|n| shot(320 + n, "/")).collect();
    assert_eq!(refused(state.clone(), six).await, "A message takes 5 pictures at most.");
    assert!(refused(state.clone(), vec![]).await.contains("Missing parameter: shots"));
    let other = json!({ "taskId": "no-such-task" });
    assert!(dispatch(&state, "shots.state", &other).await.is_err());
    assert_eq!(calls(&tools), Vec::<String>::new(), "no browser was started for any of it");

    // A computer without a browser of that family: the tool says what it needs, and takes nothing.
    let missing = PathBuf::from("/nonexistent/ae-test/browser");
    state.previews = Arc::new(Previews::with_tools(Tools { browser: Some(missing), ..Tools::default() }));
    let said = dispatch(&state, "shots.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        said,
        json!({ "browser": null, "needs": "A screenshot needs Chrome, Chromium or Edge on this computer." })
    );
    let error = refused(state.clone(), vec![shot(320, "/")]).await;
    assert_eq!(error, "A screenshot needs Chrome, Chromium or Edge on this computer.");
    state.manager.stop_all().await;
}
