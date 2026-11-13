//! Screenshots of the previewed app.
//!
//! A page cannot take a picture of a frame that shows another page, in any
//! browser engine. So the core takes it: it finds a browser of the Chromium
//! family on this computer and runs it without a window, for one picture.
//!
//! The picture is a fresh load of the page, in a browser that starts empty:
//! not signed in, and without anything the user typed into the frames of the
//! Preview. The interface says so where it offers the screenshot.
//!
//! The browser is not sent to the app's address itself. Its window is never
//! narrower than 500 pixels (measured with Chrome 157: asked for 320, the page
//! was laid out 500 wide and the picture was that page cut off at 320), so a
//! phone's width cannot be had from the window. The task's relay serves a page
//! that holds the app's page in a frame of the asked size, as the Preview
//! itself does, and the browser takes the picture of that.
//!
//! The window is also asked higher than the frame, and the picture is cut to
//! the frame afterwards. Part of a window's height goes to the browser's own
//! bars even when nothing is shown, and what a frame holds below the edge of
//! its window is not reliably drawn (measured: a bar fixed to the bottom of the
//! page was missing from the picture until the whole frame was in view).

use crate::preview::Previews;
use crate::tasks::Manager;
use crate::tools::{self, Tools};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A page that is still loading after this long is photographed as it is.
const LOAD_LIMIT: Duration = Duration::from_secs(15);
/// The browser itself is stopped when it has not ended by then.
const RUN_LIMIT: Duration = Duration::from_secs(40);
/// How much higher than the frame the window is asked: room for the browser's own bars, with a
/// wide margin. Chrome 157 on macOS takes 87 pixels.
const ROOM_FOR_BARS: u32 = 240;
/// The widths and heights a picture can be asked in, as the Preview has them.
const WIDTHS: std::ops::RangeInclusive<u64> = 200..=3840;
const HEIGHTS: std::ops::RangeInclusive<u64> = 200..=4320;
/// What the interface says when no browser is found. Kept here so both say the same.
pub const NEEDS_BROWSER: &str = "A screenshot needs Chrome, Chromium or Edge on this computer.";

/// A browser that can take the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    pub program: PathBuf,
    /// What people call it: "Chrome", "Edge".
    pub name: String,
}

/// What a browser is called, read from the name of its program.
fn name_of(program: &Path) -> String {
    let file = program.file_stem().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let known = [("edge", "Edge"), ("brave", "Brave"), ("chromium", "Chromium"), ("chrome", "Chrome")];
    known.iter().find(|(part, _)| file.contains(part)).map_or("the browser", |(_, name)| name).to_string()
}

/// The names a Chromium-family browser goes by on the `PATH`.
const ON_PATH: [&str; 10] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "brave-browser",
    "brave",
    "chrome",
    "msedge",
];

/// The usual places of Chrome, Chromium, Edge and Brave on this system, the most common first.
fn usual_places() -> Vec<PathBuf> {
    let mut places = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let apps = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
        ];
        let folders = [Some(PathBuf::from("/Applications")), dirs::home_dir().map(|home| home.join("Applications"))];
        for folder in folders.into_iter().flatten() {
            places.extend(apps.iter().map(|app| folder.join(app)));
        }
    }
    #[cfg(windows)]
    {
        let programs = [
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
        ];
        for folder in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Some(folder) = std::env::var_os(folder).filter(|value| !value.is_empty()) {
                places.extend(programs.iter().map(|program| Path::new(&folder).join(program)));
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let files = [
            "/opt/google/chrome/chrome",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/opt/microsoft/msedge/msedge",
            "/usr/bin/microsoft-edge",
            "/opt/brave.com/brave/brave",
            "/usr/bin/brave-browser",
        ];
        places.extend(files.iter().map(PathBuf::from));
    }
    places
}

/// The browser screenshots are taken with, or `None` when this computer has none.
/// Looked for anew every time: it is a few looks at the disk, and a browser installed
/// a minute ago is found without a restart.
pub fn find_browser(tools: &Tools) -> Option<Browser> {
    let program = match &tools.browser {
        // A browser that was named is the only one used, there or not.
        Some(named) => named.is_file().then(|| named.clone()),
        None => tools::first_file(usual_places()).or_else(|| ON_PATH.iter().find_map(|name| which::which(name).ok())),
    }?;
    Some(Browser { name: name_of(&program), program })
}

/// One picture to take.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ask {
    width: u32,
    height: u32,
    dark: bool,
    path: String,
}

impl Ask {
    fn read(value: &Value) -> Result<Ask> {
        let side = |name: &str, allowed: &std::ops::RangeInclusive<u64>| {
            value
                .get(name)
                .and_then(Value::as_u64)
                .filter(|pixels| allowed.contains(pixels))
                .map(|pixels| pixels as u32)
                .ok_or_else(|| {
                    anyhow!("A screenshot needs a {name} from {} to {} pixels", allowed.start(), allowed.end())
                })
        };
        let theme = value.get("theme").and_then(Value::as_str).unwrap_or("light");
        if theme != "light" && theme != "dark" {
            bail!("A screenshot is taken in light or in dark, not in \"{theme}\"");
        }
        let path = value.get("path").and_then(Value::as_str).unwrap_or("/");
        Ok(Ask {
            width: side("width", &WIDTHS)?,
            height: side("height", &HEIGHTS)?,
            dark: theme == "dark",
            path: crate::preview::page_path(path)?,
        })
    }
}

/// The words the browser is started with for one picture.
///
/// `--blink-settings=preferredColorScheme` is what a page's `prefers-color-scheme` answers to:
/// 0 is dark, 1 is light. Checked with Chrome 157 on macOS, both ways: 0 turned a page dark on
/// a light system, and 1 kept it light in a browser told that the system is dark.
fn browser_args(ask: &Ask, url: &str, file: &Path, profile: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--headless=new".into()];
    let mut joined = |name: &str, value: &std::ffi::OsStr| {
        let mut word = OsString::from(name);
        word.push(value);
        args.push(word);
    };
    joined("--screenshot=", file.as_os_str());
    joined("--window-size=", format!("{},{}", ask.width, ask.height + ROOM_FOR_BARS).as_ref());
    // A profile of its own, thrown away afterwards: with the user's own, a Chrome that is
    // already open would be handed the address and no picture would be taken.
    joined("--user-data-dir=", profile.as_os_str());
    let scheme = if ask.dark { "0" } else { "1" };
    joined("--blink-settings=preferredColorScheme=", scheme.as_ref());
    joined("--timeout=", LOAD_LIMIT.as_millis().to_string().as_ref());
    for plain in [
        "--hide-scrollbars",
        // One pixel of the picture for one pixel of the page, on every screen.
        "--force-device-scale-factor=1",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--mute-audio",
    ] {
        args.push(plain.into());
    }
    args.push(url.into());
    args
}

/// The width and height a PNG says it has, or `None` when the bytes are not a PNG.
pub(crate) fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let whole = bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR";
    let number = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    whole.then(|| (number(16), number(20)))
}

/// One part of a PNG as it stands in the file: its length, its name, what it holds, and a check over the last two.
fn png_part(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut check = flate2::Crc::new();
    check.update(kind);
    check.update(body);
    let mut part = (body.len() as u32).to_be_bytes().to_vec();
    part.extend_from_slice(kind);
    part.extend_from_slice(body);
    part.extend_from_slice(&check.sum().to_be_bytes());
    part
}

/// Cuts a PNG down to its top `height` rows.
///
/// A PNG keeps its rows one after another, each written in terms of the row above it at most,
/// so the first rows of a picture are a picture by themselves: they are unpacked, the rest is
/// left where it is, and they are packed again under a head that says the new height.
pub(crate) fn cut_to_height(png: &[u8], height: u32) -> Result<Vec<u8>> {
    let odd = || anyhow!("the picture is not a PNG that can be cut to size");
    let (width, full) = png_size(png).ok_or_else(odd)?;
    if height >= full {
        return Ok(png.to_vec());
    }
    let (mut head, mut packed, mut before, mut after) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut at = 8;
    while at + 12 <= png.len() {
        let length = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        let end = at.checked_add(12 + length).filter(|end| *end <= png.len()).ok_or_else(odd)?;
        let (kind, body) = (&png[at + 4..at + 8], &png[at + 8..at + 8 + length]);
        match kind {
            b"IHDR" => head = body.to_vec(),
            b"IDAT" => packed.extend_from_slice(body),
            b"IEND" => break,
            // What else a picture carries (its colours' meaning, mostly) goes along as it is.
            _ if packed.is_empty() => before.extend_from_slice(&png[at..end]),
            _ => after.extend_from_slice(&png[at..end]),
        }
        at = end;
    }
    // Depth, kind of colour, and whether the rows are woven into each other (then the first rows are not the top).
    let (depth, colour, woven) = match head.as_slice() {
        [_, _, _, _, _, _, _, _, depth, colour, 0, 0, woven] => (*depth as usize, *colour, *woven),
        _ => return Err(odd()),
    };
    let channels = match colour {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        6 => 4,
        _ => return Err(odd()),
    };
    if woven != 0 {
        return Err(odd());
    }
    // Each row: one byte that says how it is written, then its pixels.
    let row = 1 + (width as usize * channels * depth).div_ceil(8);
    let mut rows = vec![0u8; row * height as usize];
    flate2::read::ZlibDecoder::new(packed.as_slice()).read_exact(&mut rows).map_err(|_| odd())?;
    let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    packer.write_all(&rows)?;
    let repacked = packer.finish()?;
    head[4..8].copy_from_slice(&height.to_be_bytes());

    let mut out = png[..8].to_vec();
    out.extend(png_part(b"IHDR", &head));
    out.extend_from_slice(&before);
    out.extend(png_part(b"IDAT", &repacked));
    out.extend_from_slice(&after);
    out.extend(png_part(b"IEND", &[]));
    Ok(out)
}

/// A folder for the files of one run, removed with everything in it when the run is over.
pub(crate) struct Scratch(pub PathBuf);

impl Scratch {
    /// `what` names the kind of run: "shots" for a browser's, "screens" for a device's.
    pub(crate) fn new(data_dir: &Path, what: &str) -> Result<Scratch> {
        // Under the core's own data, not the system's temp folder: a browser installed
        // as a snap may write to the first and not to the second.
        let dir = data_dir.join("tmp").join(what).join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).with_context(|| format!("Cannot create {}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// "phone 320" has no meaning here: a picture is named by its width alone.
fn of(ask: &Ask) -> String {
    format!("{} pixels wide", ask.width)
}

/// Takes one picture and gives it as the interface gets it.
async fn take(previews: &Previews, browser: &Browser, data_dir: &Path, task_id: &str, ask: &Ask) -> Result<Value> {
    let ticket = previews.shot_page(task_id, ask.width, ask.height, &ask.path)?;
    let scratch = Scratch::new(data_dir, "shots")?;
    let file = scratch.0.join("shot.png");
    let args = browser_args(ask, &ticket.url, &file, &scratch.0.join("profile"));
    let ran = tools::run(&browser.program, args, previews.tools().limit(RUN_LIMIT)).await?;
    if ran.timed_out {
        bail!("{} took too long over the page {} and was stopped. No screenshot was taken.", browser.name, of(ask));
    }
    // The browser's exit code says little; whether the picture is there says everything.
    let whole = tokio::fs::read(&file).await.map_err(|_| {
        anyhow!("{} made no picture of the page {}. It said: {}", browser.name, of(ask), ran.last_words())
    })?;
    // The frame has to have been in view whole, or its lower part may not have been drawn.
    let window_height = ticket.window_height();
    if window_height.is_some_and(|inside| inside < ask.height) {
        bail!(
            "The window of {} was too low for a picture {} pixels high, so none was taken. A lower Preview makes a lower picture.",
            browser.name,
            ask.height
        );
    }
    let bytes = cut_to_height(&whole, ask.height)
        .map_err(|error| anyhow!("{} made a file for the page {}, but {error}.", browser.name, of(ask)))?;
    let (width, height) = png_size(&bytes).unwrap_or_default();
    if bytes.len() > crate::pictures::MAX_BYTES {
        bail!(
            "The screenshot {} is larger than {} MB, which is more than a message takes.",
            of(ask),
            crate::pictures::MAX_BYTES / 1024 / 1024
        );
    }
    Ok(json!({
        "width": ask.width,
        "pixels": { "width": width, "height": height },
        "theme": if ask.dark { "dark" } else { "light" },
        "path": ask.path,
        "mimeType": "image/png",
        "size": bytes.len(),
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        // The app's page never said it had loaded: the browser gave up waiting and the picture shows it as it was then.
        "stillLoading": window_height.is_none(),
    }))
}

/// The commands of the Screenshot tool.
///
/// - `shots.state { taskId }` gives `{ browser: "Chrome" | null, needs: "A screenshot needs …" | null }`.
/// - `shots.take { taskId, shots: [{ width, height, theme, path }] }` gives
///   `{ browser, shots: [{ width, pixels: { width, height }, theme, path, mimeType, size, data, stillLoading }] }`,
///   one PNG in base64 for each, in the order asked. One failure fails the whole command.
pub async fn command(previews: &Previews, manager: &Manager, method: &str, params: &Value) -> Result<Value> {
    let task_id = params.get("taskId").and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: taskId"))?;
    manager.task(task_id)?;
    let browser = find_browser(previews.tools());
    match method {
        "shots.state" => Ok(json!({
            "browser": browser.as_ref().map(|browser| browser.name.clone()),
            "needs": browser.is_none().then_some(NEEDS_BROWSER),
        })),
        "shots.take" => {
            let browser = browser.ok_or_else(|| anyhow!(NEEDS_BROWSER))?;
            let asked = params.get("shots").and_then(Value::as_array).filter(|list| !list.is_empty());
            let asks: Vec<Ask> = asked
                .ok_or_else(|| anyhow!("Missing parameter: shots"))?
                .iter()
                .map(Ask::read)
                .collect::<Result<_>>()?;
            if asks.len() > crate::pictures::MAX_PICTURES {
                bail!("A message takes {} pictures at most.", crate::pictures::MAX_PICTURES);
            }
            // All at once: each is a browser of its own, and waiting for them one by one would add up.
            let data_dir = manager.data_dir();
            let taking = asks.iter().map(|ask| take(previews, &browser, data_dir, task_id, ask));
            let shots: Vec<Value> = futures_util::future::join_all(taking).await.into_iter().collect::<Result<_>>()?;
            Ok(json!({ "browser": browser.name, "shots": shots }))
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_is_called_what_people_call_it() {
        let name = |program: &str| name_of(Path::new(program));
        assert_eq!(name("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"), "Chrome");
        assert_eq!(name("/usr/bin/chromium-browser"), "Chromium");
        assert_eq!(name("/opt/microsoft/msedge/msedge"), "Edge");
        assert_eq!(name("/opt/brave.com/brave/brave"), "Brave");
        assert_eq!(name("/tmp/tools/browser"), "the browser");
    }

    #[test]
    fn a_browser_that_was_named_is_the_only_one_used() {
        let missing = Tools { browser: Some(PathBuf::from("/nonexistent/ae-test/browser")), ..Tools::default() };
        assert_eq!(find_browser(&missing), None, "never the real Chrome of the computer the tests run on");
        let here = Tools { browser: std::env::current_exe().ok(), ..Tools::default() };
        assert_eq!(find_browser(&here).map(|browser| browser.program), std::env::current_exe().ok());
    }

    #[test]
    fn what_to_take_a_picture_of_is_read_with_care() {
        let ask = Ask::read(&json!({ "width": 320, "height": 700, "theme": "dark", "path": "/login?a=1" })).unwrap();
        assert_eq!(ask, Ask { width: 320, height: 700, dark: true, path: "/login?a=1".into() });
        let plain = Ask::read(&json!({ "width": 1600, "height": 900 })).unwrap();
        assert_eq!((plain.dark, plain.path.as_str()), (false, "/"));
        let refused = |value: Value| Ask::read(&value).unwrap_err().to_string();
        assert!(refused(json!({ "width": 100, "height": 700 })).contains("width from 200 to 3840"));
        assert!(refused(json!({ "width": 320 })).contains("height from 200 to 4320"));
        assert!(refused(json!({ "width": 320, "height": 700, "theme": "sepia" })).contains("light or in dark"));
        assert!(
            refused(json!({ "width": 320, "height": 700, "path": "//example.com" })).contains("not a page of the app")
        );
    }

    #[test]
    fn the_browser_is_told_the_size_the_theme_and_where_to_put_the_picture() {
        let ask = Ask { width: 320, height: 700, dark: true, path: "/".into() };
        let args =
            browser_args(&ask, "http://127.0.0.1:5999/x", Path::new("/data/shot.png"), Path::new("/data/profile"));
        let words: Vec<String> = args.iter().map(|word| word.to_string_lossy().into_owned()).collect();
        assert_eq!(words[0], "--headless=new");
        for expected in [
            "--screenshot=/data/shot.png",
            "--window-size=320,940",
            "--hide-scrollbars",
            "--blink-settings=preferredColorScheme=0",
            "--user-data-dir=/data/profile",
            "--timeout=15000",
        ] {
            assert!(words.iter().any(|word| word == expected), "{expected} is missing from {words:?}");
        }
        assert_eq!(words.last().unwrap(), "http://127.0.0.1:5999/x", "the address comes last");
        let light = browser_args(&Ask { dark: false, ..ask }, "http://x", Path::new("a"), Path::new("b"));
        assert!(light.iter().any(|word| word == "--blink-settings=preferredColorScheme=1"));
    }

    /// A PNG made here, row by row: `colour(y)` is the colour of every pixel of row `y`.
    fn striped(width: u32, height: u32, colour: impl Fn(u32) -> [u8; 3]) -> Vec<u8> {
        let mut rows = Vec::new();
        for y in 0..height {
            // Rows written as they are, and rows written as "the same as the row above" where they are.
            let same = y > 0 && colour(y) == colour(y - 1);
            rows.push(if same { 2 } else { 0 });
            for _ in 0..width {
                rows.extend_from_slice(&if same { [0, 0, 0] } else { colour(y) });
            }
        }
        let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        packer.write_all(&rows).unwrap();
        let mut head = Vec::new();
        head.extend_from_slice(&width.to_be_bytes());
        head.extend_from_slice(&height.to_be_bytes());
        head.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, body) in
            [(b"IHDR", head), (b"sRGB", vec![0]), (b"IDAT", packer.finish().unwrap()), (b"IEND", vec![])]
        {
            out.extend(png_part(kind, &body));
        }
        out
    }

    /// The rows of a PNG made by `striped` or cut from one, unpacked: the colour of the first pixel of each.
    fn first_pixels(png: &[u8]) -> Vec<[u8; 3]> {
        let (width, height) = png_size(png).unwrap();
        let start = png.windows(4).position(|kind| kind == b"IDAT").unwrap();
        let length = u32::from_be_bytes(png[start - 4..start].try_into().unwrap()) as usize;
        let mut rows = Vec::new();
        flate2::read::ZlibDecoder::new(&png[start + 4..start + 4 + length]).read_to_end(&mut rows).unwrap();
        let row = 1 + width as usize * 3;
        assert_eq!(rows.len(), row * height as usize, "the picture holds exactly its rows");
        let mut pixels: Vec<[u8; 3]> = Vec::new();
        for y in 0..height as usize {
            let line = &rows[y * row..(y + 1) * row];
            let own = [line[1], line[2], line[3]];
            pixels.push(if line[0] == 2 { pixels[y - 1] } else { own });
        }
        pixels
    }

    #[test]
    fn a_picture_is_cut_to_its_top_rows_and_stays_a_picture() {
        let colour = |y: u32| {
            if y < 3 {
                [255, 0, 0]
            } else if y < 6 {
                [0, 200, 0]
            } else {
                [0, 0, 255]
            }
        };
        let whole = striped(4, 10, colour);
        let cut = cut_to_height(&whole, 6).unwrap();
        assert_eq!(png_size(&cut), Some((4, 6)));
        assert_eq!(first_pixels(&cut), (0..6).map(colour).collect::<Vec<_>>());
        // What else the picture carried is still there, in its place before the pixels.
        let place = |kind: &[u8]| cut.windows(4).position(|window| window == kind).unwrap();
        assert!(place(b"IHDR") < place(b"sRGB") && place(b"sRGB") < place(b"IDAT") && place(b"IDAT") < place(b"IEND"));
        // Every part is signed again, so a reader that checks finds nothing wrong: the head's sign is that of the new head.
        let mut check = flate2::Crc::new();
        check.update(&cut[12..29]);
        assert_eq!(&cut[29..33], &check.sum().to_be_bytes());

        assert_eq!(cut_to_height(&whole, 10).unwrap(), whole, "a picture that is not higher is left as it is");
        assert_eq!(cut_to_height(&whole, 99).unwrap(), whole);
        assert!(cut_to_height(b"not a picture", 6).is_err());
        assert!(cut_to_height(&whole[..whole.len() - 30], 9).is_err(), "a picture that stops half way is refused");
    }

    #[test]
    fn the_size_of_a_picture_is_read_from_its_first_bytes() {
        let pixel = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")
            .unwrap();
        assert_eq!(png_size(&pixel), Some((1, 1)));
        assert_eq!(png_size(b"GIF89a, which is a picture but not this kind"), None);
        assert_eq!(png_size(&pixel[..20]), None);
        assert_eq!(png_size(b""), None);
    }
}
