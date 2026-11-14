use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::hub::{Conn, Hub, Out, SseLogin};
use crate::proto;
use crate::{http, util, ws};

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/style.css");
const NO_STORE: &[(&str, &str)] = &[
    ("Cache-Control", "no-store, no-cache, must-revalidate"),
    ("Pragma", "no-cache"),
    ("Expires", "0"),
];

static INDEX_PAGE: LazyLock<String> = LazyLock::new(|| {
    let mut input = String::with_capacity(APP_JS.len() + STYLE.len());
    input.push_str(APP_JS);
    input.push_str(STYLE);
    let version = util::hex(&util::sha1(input.as_bytes()));
    INDEX.replace("{{ASSET_VERSION}}", &version[..12])
});

fn index_html() -> &'static str {
    &INDEX_PAGE
}

pub fn handle(mut stream: TcpStream, hub: Arc<Hub>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let req = match http::read_request(&mut stream) {
        Ok(Some(req)) => req,
        _ => return,
    };
    let method = req.method.clone();
    let path = req.path.clone();

    match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => html(&mut stream, index_html()),
        ("GET", p) if p.starts_with("/g/") => html(&mut stream, index_html()),
        ("GET", "/app.js") => asset(&mut stream, "application/javascript; charset=utf-8", APP_JS),
        ("GET", "/style.css") => asset(&mut stream, "text/css; charset=utf-8", STYLE),
        ("GET", "/favicon.ico") => {
            let _ = http::respond(&mut stream, 204, "No Content", "", b"");
        }
        ("GET", "/healthz") => {
            let _ = http::text(&mut stream, 200, "OK", "ok");
        }
        ("GET", "/api/rooms") => {
            let body = hub.rooms_json();
            let _ = http::json(&mut stream, 200, "OK", &body);
        }
        ("GET", "/api/ws") => ws_handler(stream, req, hub),
        ("GET", "/api/sse") => sse_handler(stream, req, hub),
        ("POST", "/api/input") => post_input(stream, req, hub),
        _ => {
            let _ = http::not_found(&mut stream);
        }
    }
}

fn html(stream: &mut TcpStream, body: &str) {
    let _ = http::respond_with(
        stream,
        200,
        "OK",
        "text/html; charset=utf-8",
        body.as_bytes(),
        NO_STORE,
    );
}

fn asset(stream: &mut TcpStream, ctype: &str, body: &str) {
    let _ = http::respond_with(stream, 200, "OK", ctype, body.as_bytes(), NO_STORE);
}

fn ws_handler(mut stream: TcpStream, req: http::Request, hub: Arc<Hub>) {
    let Some(game) = req.q("g").and_then(util::sanitize_game_id) else {
        let _ = http::text(&mut stream, 400, "Bad Request", "missing or invalid game id");
        return;
    };
    let nick = util::sanitize_nick(req.q("nick").unwrap_or(""));
    let token = req.q("token").and_then(util::sanitize_token);
    let key = req.header("sec-websocket-key").unwrap_or("").to_string();
    let upgrade = req
        .header("upgrade")
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if key.is_empty() || !upgrade {
        let _ = http::text(&mut stream, 400, "Bad Request", "websocket upgrade required");
        return;
    }

    let accept = ws::accept_key(&key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept
    );
    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);

    let conn_id = hub.next_conn_id();
    let (tx, rx) = hub.channel();
    let conn = Conn {
        id: conn_id,
        tx: tx.clone(),
    };
    let (room, player, _token) = match hub.join(&game, &nick, token, conn) {
        Ok(value) => value,
        Err(err) => {
            let json = format!("{{\"t\":\"error\",\"msg\":\"{}\"}}", util::json_escape(&err));
            let _ = stream.write_all(&ws::encode(0x2, &proto::event_frame(&json)));
            let _ = stream.write_all(&ws::encode(0x8, &[]));
            return;
        }
    };

    let mut write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let writer = thread::spawn(move || {
        while let Ok(out) = rx.recv() {
            let bytes = match out {
                Out::App(payload) => ws::encode(0x2, &payload),
                Out::Raw(raw) => raw,
            };
            if write_stream.write_all(&bytes).is_err() {
                break;
            }
        }
        let _ = write_stream.write_all(&ws::encode(0x8, &[]));
    });

    let mut reader = ws::Reader::new(stream, req.rest);
    loop {
        match reader.next() {
            Ok(Some(ws::Incoming::Binary(buf))) | Ok(Some(ws::Incoming::Text(buf))) => {
                if let Some(msg) = proto::parse_client(&buf) {
                    if let Ok(mut guard) = room.lock() {
                        guard.on_input(player, conn_id, msg);
                    }
                }
            }
            Ok(Some(ws::Incoming::Ping(payload))) => {
                let _ = tx.try_send(Out::Raw(ws::encode(0xA, &payload)));
            }
            Ok(Some(ws::Incoming::Close)) => {
                let _ = tx.try_send(Out::Raw(ws::encode(0x8, &[])));
                break;
            }
            Ok(Some(ws::Incoming::Pong)) => {}
            Ok(None) | Err(_) => break,
        }
    }

    if let Ok(mut guard) = room.lock() {
        guard.detach(player, conn_id);
    }
    drop(tx);
    let _ = writer.join();
}

fn write_chunk(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    stream.write_all(format!("{:x}\r\n", data.len()).as_bytes())?;
    stream.write_all(data)?;
    stream.write_all(b"\r\n")
}

fn write_sse_event(stream: &mut TcpStream, event: &str, data: &str) -> io::Result<()> {
    let payload = format!("event: {}\ndata: {}\n\n", event, data);
    write_chunk(stream, payload.as_bytes())
}

fn sse_handler(mut stream: TcpStream, req: http::Request, hub: Arc<Hub>) {
    let Some(game) = req.q("g").and_then(util::sanitize_game_id) else {
        let _ = http::text(&mut stream, 400, "Bad Request", "missing or invalid game id");
        return;
    };
    let nick = util::sanitize_nick(req.q("nick").unwrap_or(""));
    let token = req.q("token").and_then(util::sanitize_token);

    // No Connection and no Content-Length: explicit connection headers make
    // Cloudflare Tunnel and some reverse proxies buffer event streams.
    // Chunked framing is what keeps the stream flowing.
    let headers = "HTTP/1.1 200 OK\r\n\
        Content-Type: text/event-stream; charset=utf-8\r\n\
        Cache-Control: no-cache, no-transform\r\n\
        X-Accel-Buffering: no\r\n\
        Transfer-Encoding: chunked\r\n\r\n";
    if stream.write_all(headers.as_bytes()).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_nodelay(true);

    let conn_id = hub.next_conn_id();
    let (tx, rx) = hub.channel();
    let sid = util::random_token();
    let conn = Conn {
        id: conn_id,
        tx: tx.clone(),
    };
    let (room, player, _token) = match hub.join(&game, &nick, token, conn) {
        Ok(value) => value,
        Err(err) => {
            let json = format!("{{\"t\":\"error\",\"msg\":\"{}\"}}", util::json_escape(&err));
            let _ = write_sse_event(&mut stream, "fatal", &json);
            let _ = stream.write_all(b"0\r\n\r\n");
            return;
        }
    };
    drop(tx);

    hub.sse.lock().unwrap().insert(
        sid.clone(),
        SseLogin {
            room: room.clone(),
            player,
            conn: conn_id,
        },
    );

    let hello = format!("{{\"sid\":\"{}\",\"you\":{}}}", sid, player);
    if write_sse_event(&mut stream, "hello", &hello).is_err() {
        hub.sse.lock().unwrap().remove(&sid);
        if let Ok(mut guard) = room.lock() {
            guard.detach(player, conn_id);
        }
        return;
    }

    let mut last_heartbeat = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Out::App(frame)) => {
                let data = util::b64_encode(&frame);
                if write_sse_event(&mut stream, "bin", &data).is_err() {
                    break;
                }
            }
            Ok(Out::Raw(_)) => {}
            Err(RecvTimeoutError::Timeout) => {
                if last_heartbeat.elapsed() >= Duration::from_secs(10) {
                    if write_chunk(&mut stream, b": hb\n\n").is_err() {
                        break;
                    }
                    last_heartbeat = Instant::now();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    hub.sse.lock().unwrap().remove(&sid);
    if let Ok(mut guard) = room.lock() {
        guard.detach(player, conn_id);
    }
    let _ = stream.write_all(b"0\r\n\r\n");
    let _ = stream.flush();
}

fn post_input(mut stream: TcpStream, req: http::Request, hub: Arc<Hub>) {
    let sid = req.q("sid").unwrap_or("").to_string();
    let login = {
        let sse = hub.sse.lock().unwrap();
        sse.get(&sid)
            .map(|login| (login.room.clone(), login.player, login.conn))
    };
    let Some((room, player, conn)) = login else {
        let _ = http::text(&mut stream, 404, "Not Found", "unknown session");
        return;
    };
    if let Some(msg) = proto::parse_client(&req.body) {
        if let Ok(mut guard) = room.lock() {
            guard.on_input(player, conn, msg);
        }
    }
    let _ = http::respond(&mut stream, 204, "No Content", "", b"");
}
