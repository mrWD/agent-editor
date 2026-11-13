//! End-to-end tests of the relay the Preview view shows a running app through.
//!
//! The "app" is a small server started here, on this machine. The relay is
//! talked to over plain sockets, byte by byte, the way a browser does, so
//! what it passes on and what it changes can be compared exactly.

mod common;

use ae_core::preview::SCRIPT_PATH;
use ae_core::server::{AppState, dispatch};
use ae_core::{Options, Port};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use common::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<title>Shop</title>\n</head>\n<body><h1>Sign in</h1></body>\n</html>\n";

/// Bytes that are not text and do not repeat, so a missing or doubled piece shows.
fn noise(length: usize) -> Vec<u8> {
    let mut state: u32 = 0x9E37_79B9;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect()
}

const BIG: usize = 8 * 1024 * 1024;

/// What the app saw of a request: the headers that tell where it came from, and its body.
async fn seen(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string);
    axum::Json(json!({
        "host": text("host"),
        "origin": text("origin"),
        "referer": text("referer"),
        "encoding": text("accept-encoding"),
        "cookie": text("cookie"),
        "body": String::from_utf8_lossy(&body),
    }))
}

/// A small app on a free port of this machine. Returns its port.
async fn start_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let html = || [(header::CONTENT_TYPE, "text/html; charset=utf-8")];
    let app = Router::new()
        .route("/", get(move || async move { (html(), PAGE) }))
        // A page sent in pieces, without saying its length first.
        .route(
            "/stream",
            get(move || async move {
                let pieces =
                    PAGE.as_bytes().chunks(17).map(|piece| Ok::<_, std::io::Error>(Bytes::copy_from_slice(piece)));
                (html(), Body::from_stream(futures_util::stream::iter(pieces.collect::<Vec<_>>())))
            }),
        )
        // A script that talks about a head must not be taken for a page.
        .route(
            "/app.js",
            get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], "document.write('<head></head>');\n") }),
        )
        .route("/big.bin", get(|| async { ([(header::CONTENT_TYPE, "application/octet-stream")], noise(BIG)) }))
        .route(
            "/stream.bin",
            get(|| async {
                let pieces: Vec<_> = noise(BIG / 8)
                    .chunks(64 * 1024)
                    .map(|piece| Ok::<_, std::io::Error>(Bytes::copy_from_slice(piece)))
                    .collect();
                (
                    [(header::CONTENT_TYPE, "application/octet-stream")],
                    Body::from_stream(futures_util::stream::iter(pieces)),
                )
            }),
        )
        .route("/seen", post(seen).get(seen))
        .route(
            "/go",
            get(move || async move {
                (StatusCode::FOUND, [(header::LOCATION, format!("http://localhost:{port}/login?next=1"))])
            }),
        )
        .route(
            "/strict",
            get(move || async move {
                (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (header::X_FRAME_OPTIONS, "DENY"),
                        (
                            header::CONTENT_SECURITY_POLICY,
                            "default-src 'self'; frame-ancestors 'none'; script-src 'nonce-r4nd0m'",
                        ),
                        (header::SET_COOKIE, "sid=7; Path=/; Domain=localhost"),
                    ],
                    PAGE,
                )
            }),
        )
        .route(
            "/echo",
            get(|upgrade: WebSocketUpgrade| async {
                upgrade.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        // Only what was said comes back. The answer to a close is sent by the
                        // next read, which then ends the loop.
                        let said = matches!(message, Message::Text(_) | Message::Binary(_));
                        if said && socket.send(message).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// One answer as it came over the wire.
struct Answer {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (known, value) = line.split_once(':')?;
            known.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends one request to the relay over a socket of its own and reads the whole answer.
/// `more` is added to the request's headers; `body` is sent with its length.
async fn ask(port: u16, method: &str, path: &str, more: &str, body: &[u8]) -> Answer {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.expect("the relay listens");
    let host =
        if more.to_lowercase().contains("host:") { String::new() } else { format!("Host: 127.0.0.1:{port}\r\n") };
    let length = if body.is_empty() { String::new() } else { format!("Content-Length: {}\r\n", body.len()) };
    socket.write_all(format!("{method} {path} HTTP/1.1\r\n{host}{more}{length}\r\n").as_bytes()).await.unwrap();
    socket.write_all(body).await.unwrap();
    let mut all = Vec::new();
    // The relay closes the connection when the answer is complete.
    tokio::time::timeout(Duration::from_secs(30), socket.read_to_end(&mut all))
        .await
        .expect("the relay ends its answer")
        .unwrap();
    let split = all.windows(4).position(|window| window == b"\r\n\r\n").expect("the answer has a head");
    let head = String::from_utf8_lossy(&all[..split]).into_owned();
    let status = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
    Answer { status, head, body: all[split + 4..].to_vec() }
}

async fn get_from(port: u16, path: &str) -> Answer {
    ask(port, "GET", path, "", b"").await
}

/// Undoes the chunks of a body that was passed on in chunks.
fn unchunk(mut wire: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = wire.windows(2).position(|window| window == b"\r\n").expect("a chunk starts with its length");
        let length = usize::from_str_radix(std::str::from_utf8(&wire[..line_end]).unwrap().trim(), 16).unwrap();
        if length == 0 {
            return body;
        }
        body.extend_from_slice(&wire[line_end + 2..line_end + 2 + length]);
        wire = &wire[line_end + 2 + length + 2..];
    }
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

/// A core's state with one idle task in a project of the sandbox.
async fn state_with_task(sandbox: &Sandbox) -> (AppState, String) {
    let state = AppState::new(sandbox.manager());
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &project.id).await;
    (state, task_id)
}

/// Opens the relay of a task for an app and returns the relay's port.
async fn open(state: &AppState, task_id: &str, app_port: u16) -> u16 {
    let params =
        json!({ "taskId": task_id, "target": format!("localhost:{app_port}"), "origin": "http://127.0.0.1:4999" });
    let opened = dispatch(state, "preview.open", &params).await.expect("the relay opens");
    assert_eq!(opened["target"], format!("localhost:{app_port}"));
    opened["port"].as_u64().unwrap() as u16
}

/// Whether the relay that listened on `port` is gone and the port is free for others.
async fn nothing_listens_on(port: u16) -> bool {
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
        return true;
    }
    // Taken again already. The system hands a freed port to the next program that asks for any port,
    // so this is fine as long as whoever has it now is not the relay.
    let Ok(mut socket) = TcpStream::connect(("127.0.0.1", port)).await else { return true };
    let request = format!("GET {SCRIPT_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let mut answer = Vec::new();
    let _ = socket.write_all(request.as_bytes()).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut answer)).await;
    !String::from_utf8_lossy(&answer).contains("AE_PREVIEW_CONFIG")
}

/// The tests of this file run one at a time: each asks the system for free ports and then
/// checks that a port is free again, which another test asking at that moment would spoil.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn script_tag() -> String {
    format!("<script src=\"{SCRIPT_PATH}\"></script>")
}

#[tokio::test]
async fn a_page_gets_our_script_and_everything_else_passes_unchanged() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-pages");
    let (state, task_id) = state_with_task(&sandbox).await;
    let app = start_app().await;
    let relay = open(&state, &task_id, app).await;
    assert_ne!(relay, app);

    // The page: the same page, with one tag of ours right after <head>, and a length that is true again.
    let page = get_from(relay, "/").await;
    assert_eq!(page.status, 200);
    let expected = PAGE.replace("<head>", &format!("<head>{}", script_tag()));
    assert_eq!(page.text(), expected);
    assert_eq!(page.header("content-length"), Some(expected.len().to_string()));
    assert_eq!(page.header("content-type").as_deref(), Some("text/html; charset=utf-8"));

    // The same page sent in pieces arrives whole, with the tag, and says its length.
    let streamed = get_from(relay, "/stream").await;
    assert_eq!(streamed.text(), expected);
    assert_eq!(streamed.header("content-length"), Some(expected.len().to_string()));
    assert_eq!(streamed.header("transfer-encoding"), None);

    // A script file is not a page, whatever it says.
    let script = get_from(relay, "/app.js").await;
    assert_eq!(script.text(), "document.write('<head></head>');\n");

    // A large file that is not text: every byte, in order.
    let big = get_from(relay, "/big.bin").await;
    assert_eq!(big.status, 200);
    assert_eq!(big.body.len(), BIG);
    assert!(big.body == noise(BIG), "the large file arrives byte for byte");
    // And one sent in pieces, which the relay passes on in the same pieces.
    let pieces = get_from(relay, "/stream.bin").await;
    assert_eq!(pieces.header("transfer-encoding").as_deref(), Some("chunked"));
    assert!(unchunk(&pieces.body) == noise(BIG / 8), "the file sent in pieces arrives byte for byte");

    // A path the app does not have is the app's own "not found", not ours.
    assert_eq!(get_from(relay, "/nowhere").await.status, 404);
    // HEAD has no body to wait for.
    let head_only = ask(relay, "HEAD", "/", "", b"").await;
    assert_eq!(head_only.status, 200);
    assert!(head_only.body.is_empty());

    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
}

#[tokio::test]
async fn the_relay_serves_the_script_itself_and_tells_it_whom_to_talk_to() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-script");
    let (state, task_id) = state_with_task(&sandbox).await;
    let app = start_app().await;
    let relay = open(&state, &task_id, app).await;

    let script = get_from(relay, SCRIPT_PATH).await;
    assert_eq!(script.status, 200);
    assert_eq!(script.header("content-type").as_deref(), Some("text/javascript; charset=utf-8"));
    assert_eq!(script.header("cache-control").as_deref(), Some("no-store"));
    // The window that asked for the relay is the only one the page's script answers.
    assert!(
        script.text().contains(r#"var AE_PREVIEW_CONFIG = {"parents":["http://127.0.0.1:4999"]};"#),
        "{}",
        script.text()
    );
    // A second window, under another name of this computer, is added; a made-up one is not.
    let again = json!({ "taskId": task_id, "target": format!("localhost:{app}"), "origin": "http://localhost:4999" });
    dispatch(&state, "preview.open", &again).await.unwrap();
    let bad = json!({ "taskId": task_id, "target": format!("localhost:{app}"), "origin": "\"];alert(1);[\"" });
    dispatch(&state, "preview.open", &bad).await.unwrap();
    let script = get_from(relay, SCRIPT_PATH).await;
    assert!(
        script.text().contains(r#"{"parents":["http://127.0.0.1:4999","http://localhost:4999"]}"#),
        "{}",
        script.text()
    );
    assert!(!script.text().contains("alert(1)"));

    // Nothing else lives under our own path, and none of it reaches the app.
    assert_eq!(get_from(relay, "/__ae_preview/other").await.status, 404);
    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
}

#[tokio::test]
async fn the_app_sees_requests_as_its_own_and_its_answers_work_in_a_frame() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-headers");
    let (state, task_id) = state_with_task(&sandbox).await;
    let app = start_app().await;
    let relay = open(&state, &task_id, app).await;

    // A form posted from the page: the body arrives, and the app is told its own address, not the relay's.
    let more = format!(
        "Origin: http://127.0.0.1:{relay}\r\nReferer: http://127.0.0.1:{relay}/login\r\nAccept-Encoding: gzip, br\r\nCookie: sid=7\r\nContent-Type: text/plain\r\n"
    );
    let posted = ask(relay, "POST", "/seen", &more, b"email=a%40b.c").await;
    assert_eq!(posted.status, 200);
    let saw: Value = serde_json::from_slice(&posted.body).unwrap();
    assert_eq!(saw["host"], format!("localhost:{app}"));
    assert_eq!(saw["origin"], format!("http://localhost:{app}"));
    assert_eq!(saw["referer"], format!("http://localhost:{app}/login"));
    assert_eq!(saw["encoding"], "identity", "a page must arrive as text");
    assert_eq!(saw["cookie"], "sid=7");
    assert_eq!(saw["body"], "email=a%40b.c");

    // A redirect to the app's own address stays inside the relay.
    let moved = get_from(relay, "/go").await;
    assert_eq!(moved.status, 302);
    assert_eq!(moved.header("location").as_deref(), Some("/login?next=1"));

    // A page that forbids frames is shown all the same; the rest of its policy stays, and our tag carries its word.
    let strict = get_from(relay, "/strict").await;
    assert_eq!(strict.header("x-frame-options"), None);
    assert_eq!(
        strict.header("content-security-policy").as_deref(),
        Some("default-src 'self'; script-src 'nonce-r4nd0m'")
    );
    assert_eq!(strict.header("set-cookie").as_deref(), Some("sid=7; Path=/"));
    assert!(
        strict.text().contains(&format!("<head><script nonce=\"r4nd0m\" src=\"{SCRIPT_PATH}\"></script>")),
        "{}",
        strict.text()
    );

    // Another site's page cannot use the relay under a name of its own.
    let foreign = ask(relay, "GET", "/seen", "Host: evil.example\r\n", b"").await;
    assert_eq!(foreign.status, 403);
    // A request that already went through a relay is going in circles.
    assert_eq!(ask(relay, "GET", "/", "X-Ae-Preview: 1\r\n", b"").await.status, 508);

    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
}

/// Reads from a socket until the empty line that ends a head.
async fn read_head(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(socket.read_u8().await.expect("the head arrives"));
    }
    String::from_utf8(head).unwrap()
}

/// One WebSocket frame as a browser sends it: masked, text or close.
fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut bytes = vec![0x80 | opcode];
    if payload.len() < 126 {
        bytes.push(0x80 | payload.len() as u8);
    } else {
        bytes.push(0x80 | 126);
        bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    bytes.extend_from_slice(&mask);
    bytes.extend(payload.iter().enumerate().map(|(at, byte)| byte ^ mask[at % 4]));
    bytes
}

/// Reads one frame a server sent: its kind and what it carries.
async fn read_frame(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let first = socket.read_u8().await.expect("a frame arrives");
    let short = socket.read_u8().await.unwrap();
    assert_eq!(short & 0x80, 0, "a server does not mask");
    let length = if short == 126 { socket.read_u16().await.unwrap() as usize } else { short as usize };
    let mut payload = vec![0; length];
    socket.read_exact(&mut payload).await.unwrap();
    (first & 0x0F, payload)
}

#[tokio::test]
async fn a_websocket_echoes_through_the_relay() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-socket");
    let (state, task_id) = state_with_task(&sandbox).await;
    let app = start_app().await;
    let relay = open(&state, &task_id, app).await;

    let mut socket = TcpStream::connect(("127.0.0.1", relay)).await.unwrap();
    let hello = format!(
        "GET /echo HTTP/1.1\r\nHost: 127.0.0.1:{relay}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: http://127.0.0.1:{relay}\r\n\r\n"
    );
    socket.write_all(hello.as_bytes()).await.unwrap();
    let talk = async {
        let head = read_head(&mut socket).await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        // The answer to the key is the app's own: the relay passed the handshake on untouched.
        assert!(
            head.to_lowercase().contains("sec-websocket-accept: s3ppLMBiTxaQ9kYGzzhZRbK+xOo=".to_lowercase().as_str()),
            "{head}"
        );

        socket.write_all(&frame(1, b"hot reload?")).await.unwrap();
        assert_eq!(read_frame(&mut socket).await, (1, b"hot reload?".to_vec()));
        // A longer message, and several in a row: nothing is held back or reordered.
        let long = "x".repeat(3000).into_bytes();
        socket.write_all(&frame(1, &long)).await.unwrap();
        socket.write_all(&frame(1, b"second")).await.unwrap();
        assert_eq!(read_frame(&mut socket).await, (1, long));
        assert_eq!(read_frame(&mut socket).await, (1, b"second".to_vec()));

        // Closing is passed on too, and the relay lets go of the connection.
        socket.write_all(&frame(8, &[0x03, 0xE8])).await.unwrap();
        assert_eq!(read_frame(&mut socket).await.0, 8);
        let mut rest = Vec::new();
        socket.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    };
    tokio::time::timeout(Duration::from_secs(20), talk).await.expect("the WebSocket answers in time");

    // A socket that is open when the relay stops is dropped with it.
    let mut open_socket = TcpStream::connect(("127.0.0.1", relay)).await.unwrap();
    open_socket.write_all(hello.as_bytes()).await.unwrap();
    assert!(read_head(&mut open_socket).await.starts_with("HTTP/1.1 101"));
    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), open_socket.read_to_end(&mut rest)).await;
    assert!(ended.is_ok(), "the open WebSocket ends when the relay stops");
    assert!(nothing_listens_on(relay).await);
}

#[tokio::test]
async fn only_an_app_on_this_computer_can_be_shown() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-local");
    let (state, task_id) = state_with_task(&sandbox).await;

    for elsewhere in ["example.com", "http://example.com:3000/login", "192.168.1.20:5173", "https://localhost:3000"] {
        let refusal = dispatch(&state, "preview.open", &json!({ "taskId": task_id, "target": elsewhere }))
            .await
            .expect_err("an address that is not a local http address is refused");
        let text = refusal.to_string();
        assert!(text.contains("on this computer") || text.contains("https is not supported"), "{elsewhere}: {text}");
    }
    // A refusal starts nothing.
    let shown = dispatch(&state, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown["relay"], Value::Null);
    // A task that is not there has no preview.
    assert!(
        dispatch(&state, "preview.open", &json!({ "taskId": "nobody", "target": "localhost:3000" })).await.is_err()
    );
    assert!(dispatch(&state, "preview.open", &json!({ "taskId": task_id })).await.is_err(), "no address, no relay");
}

#[tokio::test]
async fn the_relay_keeps_its_port_when_the_address_changes_and_says_when_nothing_answers() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-retarget");
    let (state, task_id) = state_with_task(&sandbox).await;
    let app = start_app().await;
    let relay = open(&state, &task_id, app).await;
    let shown = dispatch(&state, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown["relay"], json!({ "port": relay, "target": format!("localhost:{app}") }));

    // The same task, another app: the frames keep their address, the relay now passes on to the new app.
    let other = start_app().await;
    assert_eq!(open(&state, &task_id, other).await, relay);
    let saw: Value = serde_json::from_slice(&get_from(relay, "/seen").await.body).unwrap();
    assert_eq!(saw["host"], format!("localhost:{other}"));

    // An address nothing listens on: a page of ours says so, and it still carries our script.
    let free = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    assert_eq!(open(&state, &task_id, free).await, relay);
    let nobody = get_from(relay, "/").await;
    assert_eq!(nobody.status, 502);
    assert!(nobody.text().contains(&format!("Nothing answers at localhost:{free}. Start the app, then reload.")));
    assert!(nobody.text().contains(&script_tag()));
    assert!(nobody.text().contains("<meta name=\"ae-preview-problem\" content=\"unreachable\">"));

    // The relay refuses to be pointed at itself.
    let itself =
        dispatch(&state, "preview.open", &json!({ "taskId": task_id, "target": format!("127.0.0.1:{relay}") })).await;
    assert!(itself.unwrap_err().to_string().contains("the address of the preview itself"));

    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
    assert!(nothing_listens_on(relay).await, "closing frees the port");
    let shown = dispatch(&state, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown["relay"], Value::Null);
    // Closing what is closed is not an error.
    dispatch(&state, "preview.close", &json!({ "taskId": task_id })).await.unwrap();

    // Opened again, even by a core started anew, the relay takes the port it had: what the app
    // keeps in the browser is kept under its address, port and all.
    let later = AppState::new(sandbox.manager());
    let again = open(&later, &task_id, app).await;
    if again != relay {
        // Another program on this machine may take a freed port at any moment; only then is another port right.
        let taken = std::net::TcpListener::bind(("127.0.0.1", relay)).is_err();
        assert!(taken, "port {relay} was free, so the relay should have taken it again, not {again}");
    }
    assert_eq!(get_from(again, "/app.js").await.status, 200);
    dispatch(&later, "preview.close", &json!({ "taskId": task_id })).await.unwrap();
}

#[tokio::test]
async fn what_the_view_was_set_to_is_remembered_per_task() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-settings");
    let (state, task_id) = state_with_task(&sandbox).await;
    let shown = dispatch(&state, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown, json!({ "relay": null, "settings": null }));

    let settings = json!({ "address": "localhost:5173/login", "widths": [320, 768], "custom": [414], "theme": "dark" });
    dispatch(&state, "preview.remember", &json!({ "taskId": task_id, "settings": settings })).await.unwrap();
    let shown = dispatch(&state, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown["settings"], settings);

    // It is kept in the core's store, so it is there after a restart: a second state on the same data sees it.
    let later = AppState::new(sandbox.manager());
    let shown = dispatch(&later, "preview.state", &json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(shown["settings"], settings);
    assert_eq!(shown["relay"], Value::Null, "a relay does not outlive its core");

    // Only a set of named values is taken, and not an endless one.
    assert!(dispatch(&state, "preview.remember", &json!({ "taskId": task_id, "settings": "dark" })).await.is_err());
    let endless = json!({ "taskId": task_id, "settings": { "address": "x".repeat(20_000) } });
    assert!(dispatch(&state, "preview.remember", &endless).await.is_err());
}

#[tokio::test]
async fn archiving_a_task_and_stopping_the_core_leave_nothing_listening() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let sandbox = Sandbox::new("preview-stop");
    let options = Options { port: Port::Free { prefer: None }, ..Options::default() };
    let core = ae_core::startup::serve(sandbox.manager(), &options).await.expect("the core starts");
    let state = core.state().clone();
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let app = start_app().await;

    // Archiving a task stops its relay with everything else it ran.
    let archived = idle_task(&state, &project.id).await;
    let first = open(&state, &archived, app).await;
    assert_eq!(get_from(first, "/app.js").await.status, 200);
    dispatch(&state, "tasks.archive", &json!({ "taskId": archived })).await.unwrap();
    assert!(nothing_listens_on(first).await, "archiving frees the port");

    // Two more tasks, each with a relay of its own; stopping the core stops both.
    let one = idle_task(&state, &project.id).await;
    let two = idle_task(&state, &project.id).await;
    let (port_one, port_two) = (open(&state, &one, app).await, open(&state, &two, app).await);
    assert_ne!(port_one, port_two, "each task has a relay of its own");
    assert_eq!(get_from(port_two, "/app.js").await.status, 200);
    drop(state);
    core.stop().await.expect("the core stops");
    assert!(nothing_listens_on(port_one).await);
    assert!(nothing_listens_on(port_two).await);
}
