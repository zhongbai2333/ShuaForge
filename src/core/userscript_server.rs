use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Mutex, OnceLock, mpsc},
    thread,
};

const USERSCRIPT: &str = include_str!("../../userscripts/shuaforge-question-exporter.user.js");
const SCRIPT_PATH: &str = "/shuaforge-question-exporter.user.js";
const BRIDGE_PING_PATH: &str = "/bridge/ping";
const BRIDGE_POLL_PATH: &str = "/bridge/poll";
const BRIDGE_SNAPSHOT_PATH: &str = "/bridge/snapshot";
const PREFERRED_PORT: u16 = 53391;
static INSTALL_URL: OnceLock<String> = OnceLock::new();
static BRIDGE_BASE_URL: OnceLock<String> = OnceLock::new();
static BRIDGE: OnceLock<BridgeState> = OnceLock::new();

struct BridgeState {
    commands: Mutex<VecDeque<BridgeCommand>>,
    snapshot_sender: Mutex<Option<SnapshotCollector>>,
    clients: Mutex<HashMap<String, std::time::Instant>>,
}

struct SnapshotCollector {
    sender: mpsc::Sender<String>,
    remaining: usize,
}

#[derive(Debug, Clone)]
struct BridgeCommand {
    id: String,
    command: String,
    selector: Option<String>,
    target_client_id: Option<String>,
}

pub fn open_userscript_install_page() -> Result<String, String> {
    let script_url = ensure_server_url()?;
    let install_url = tampermonkey_install_url(&script_url);
    webbrowser::open(&install_url).map_err(|err| format!("打开浏览器失败：{err}"))?;
    Ok(install_url)
}

pub fn ensure_bridge_running() -> Result<String, String> {
    ensure_server_url().map(|script_url| {
        script_url
            .strip_suffix(SCRIPT_PATH)
            .unwrap_or(&script_url)
            .to_owned()
    })
}

pub fn request_page_snapshot() -> Result<mpsc::Receiver<String>, String> {
    ensure_server_url()?;
    let bridge = bridge_state();
    let client_ids = registered_client_ids()?;
    if client_ids.is_empty() {
        return Err("未发现已打开的题库页面。请打开装有 ShuaForge 油猴脚本的题库/解析页面，脚本会自动连接。".to_owned());
    }
    let (sender, receiver) = mpsc::channel();
    {
        let mut snapshot_sender = bridge
            .snapshot_sender
            .lock()
            .map_err(|_| "页面快照通道已损坏".to_owned())?;
        *snapshot_sender = Some(SnapshotCollector {
            sender,
            remaining: client_ids.len(),
        });
    }
    let mut commands = bridge
        .commands
        .lock()
        .map_err(|_| "页面快照命令队列已损坏".to_owned())?;
    let batch_id = timestamp_millis();
    for (index, client_id) in client_ids.into_iter().enumerate() {
        commands.push_back(BridgeCommand {
            id: format!("{batch_id}-{index}"),
            command: "snapshot".into(),
            selector: None,
            target_client_id: Some(client_id),
        });
    }
    Ok(receiver)
}

fn bridge_state() -> &'static BridgeState {
    BRIDGE.get_or_init(|| BridgeState {
        commands: Mutex::new(VecDeque::new()),
        snapshot_sender: Mutex::new(None),
        clients: Mutex::new(HashMap::new()),
    })
}

fn registered_client_ids() -> Result<Vec<String>, String> {
    let now = std::time::Instant::now();
    let mut clients = bridge_state()
        .clients
        .lock()
        .map_err(|_| "浏览器页面列表已损坏".to_owned())?;
    clients.retain(|_, seen_at| now.duration_since(*seen_at) < std::time::Duration::from_secs(20));
    Ok(clients.keys().cloned().collect())
}

fn register_client(client_id: String) {
    if let Ok(mut clients) = bridge_state().clients.lock() {
        clients.insert(client_id, std::time::Instant::now());
    }
}

fn ensure_server_url() -> Result<String, String> {
    if let Some(url) = INSTALL_URL.get() {
        return Ok(url.clone());
    }

    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .map_err(|err| format!("启动本地脚本安装服务失败：{err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("读取本地服务端口失败：{err}"))?
        .port();
    let url = format!("http://127.0.0.1:{port}{SCRIPT_PATH}");
    let bridge_url = format!("http://127.0.0.1:{port}");

    thread::Builder::new()
        .name("shuaforge-userscript-server".into())
        .spawn(move || serve(listener))
        .map_err(|err| format!("启动本地脚本安装服务线程失败：{err}"))?;

    let _ = INSTALL_URL.set(url.clone());
    let _ = BRIDGE_BASE_URL.set(bridge_url);
    Ok(url)
}

fn tampermonkey_install_url(script_url: &str) -> String {
    format!("https://www.tampermonkey.net/script_installation.php#url={script_url}")
}

fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = handle_stream(stream);
    }
}

fn handle_stream(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut content_length = 0usize;

    loop {
        let mut header_line = String::new();
        let bytes = reader.read_line(&mut header_line)?;
        if bytes == 0 || header_line == "\r\n" || header_line == "\n" {
            break;
        }
        if let Some(value) = header_line
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim())
        {
            content_length = value.parse().unwrap_or(0);
        }
    }

    let method = request_line.split_whitespace().next().unwrap_or("GET");

    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = split_path_query(target);

    if method == "OPTIONS" {
        return write_empty_response(&mut stream, "204 No Content");
    }

    if method == "POST" && path == BRIDGE_SNAPSHOT_PATH {
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body)?;
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        if let Ok(mut collector) = bridge_state().snapshot_sender.lock()
            && let Some(active) = collector.as_mut()
        {
            let _ = active.sender.send(text);
            active.remaining = active.remaining.saturating_sub(1);
            if active.remaining == 0 {
                *collector = None;
            }
        }
        return write_response(
            &mut stream,
            "200 OK",
            "application/json; charset=utf-8",
            "{\"ok\":true}",
        );
    }

    match path {
        BRIDGE_PING_PATH => {
            if let Some(client_id) = query_param(query, "client_id") {
                register_client(client_id);
            }
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                "{\"ok\":true,\"service\":\"ShuaForge bridge\"}",
            )
        }
        BRIDGE_POLL_PATH => {
            write_response(&mut stream, "200 OK", "application/json; charset=utf-8", &{
                let client_id = query_param(query, "client_id");
                if let Some(client_id) = &client_id {
                    register_client(client_id.clone());
                }
                next_bridge_command_json(client_id.as_deref())
            })
        }
        SCRIPT_PATH => {
            let script = USERSCRIPT.replace(
                "__SHUAFORGE_BRIDGE_BASE_URL__",
                BRIDGE_BASE_URL
                    .get()
                    .map(String::as_str)
                    .unwrap_or("http://127.0.0.1:0"),
            );
            write_response_with_options(
                &mut stream,
                "200 OK",
                "text/javascript; charset=utf-8",
                &script,
                method == "HEAD",
            )
        }
        _ => write_response(
            &mut stream,
            "302 Found",
            "text/plain; charset=utf-8",
            "Redirecting to ShuaForge userscript installer...",
        ),
    }
}

fn next_bridge_command_json(client_id: Option<&str>) -> String {
    let command = bridge_state()
        .commands
        .lock()
        .ok()
        .and_then(|mut commands| {
            let index = commands.iter().position(|command| {
                command
                    .target_client_id
                    .as_deref()
                    .is_none_or(|target| Some(target) == client_id)
            })?;
            commands.remove(index)
        });
    match command {
        Some(command) => format!(
            "{{\"id\":\"{}\",\"command\":\"{}\",\"selector\":{}}}",
            json_escape(&command.id),
            json_escape(&command.command),
            command
                .selector
                .as_ref()
                .map(|selector| format!("\"{}\"", json_escape(selector)))
                .unwrap_or_else(|| "null".into())
        ),
        None => "{\"command\":\"none\"}".into(),
    }
}

fn split_path_query(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        if byte == b'%'
            && let (Some(high), Some(low)) = (iter.next(), iter.next())
            && let (Some(high), Some(low)) = (hex_value(high), hex_value(low))
        {
            bytes.push(high * 16 + low);
        } else if byte == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn timestamp_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write_response_with_options(stream, status, content_type, body, false)
}

fn write_response_with_options(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    head_only: bool,
) -> std::io::Result<()> {
    let extra_headers = if status == "302 Found" {
        format!("Location: {SCRIPT_PATH}\r\n")
    } else {
        String::new()
    };
    let headers = format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nExpires: 0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Private-Network: true\r\nCross-Origin-Resource-Policy: cross-origin\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    if !head_only {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

fn write_empty_response(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Private-Network: true\r\nCross-Origin-Resource-Policy: cross-origin\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        thread,
    };

    #[test]
    fn embedded_userscript_has_metadata_header() {
        assert!(USERSCRIPT.starts_with("// ==UserScript=="));
        assert!(USERSCRIPT.contains("// @name         ShuaForge 题库采集代理"));
        assert!(USERSCRIPT.contains("// ==/UserScript=="));
    }

    #[test]
    fn script_response_is_extension_install_friendly() {
        let response = request_once(
            "GET /shuaforge-question-exporter.user.js HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/javascript; charset=utf-8"));
        assert!(response.contains("Access-Control-Allow-Origin: *"));
        assert!(response.contains("Access-Control-Allow-Private-Network: true"));
        assert!(response.contains("Cross-Origin-Resource-Policy: cross-origin"));
        assert!(response.contains("// ==UserScript=="));
    }

    #[test]
    fn head_request_returns_headers_without_body() {
        let response = request_once(
            "HEAD /shuaforge-question-exporter.user.js HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Length: "));
        assert!(!response.contains("// ==UserScript=="));
    }

    #[test]
    fn options_request_supports_cors_preflight() {
        let response = request_once(
            "OPTIONS /shuaforge-question-exporter.user.js HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );

        assert!(response.starts_with("HTTP/1.1 204 No Content"));
        assert!(response.contains("Access-Control-Allow-Methods: GET, POST, HEAD, OPTIONS"));
        assert!(response.contains("Access-Control-Allow-Private-Network: true"));
    }

    #[test]
    fn official_install_url_wraps_local_userscript_url() {
        let url =
            tampermonkey_install_url("http://127.0.0.1:5339/shuaforge-question-exporter.user.js");

        assert_eq!(
            url,
            "https://www.tampermonkey.net/script_installation.php#url=http://127.0.0.1:5339/shuaforge-question-exporter.user.js"
        );
    }

    #[test]
    fn ping_registers_browser_clients_for_auto_connection() {
        register_client("auto-client-a".into());
        register_client("auto-client-b".into());

        let clients = registered_client_ids().expect("registered clients");

        assert!(clients.contains(&"auto-client-a".to_owned()));
        assert!(clients.contains(&"auto-client-b".to_owned()));
    }

    #[test]
    fn snapshot_requests_are_targeted_to_each_registered_client() {
        let bridge = bridge_state();
        bridge.commands.lock().expect("commands").clear();
        *bridge.snapshot_sender.lock().expect("snapshot sender") = None;
        bridge.clients.lock().expect("clients").clear();
        register_client("target-client-a".into());
        register_client("target-client-b".into());

        let _receiver = request_page_snapshot().expect("snapshot receiver");
        let first = next_bridge_command_json(Some("target-client-a"));
        let second = next_bridge_command_json(Some("target-client-b"));
        let none_left = next_bridge_command_json(Some("target-client-a"));

        assert!(first.contains("\"command\":\"snapshot\""));
        assert!(second.contains("\"command\":\"snapshot\""));
        assert_eq!(none_left, "{\"command\":\"none\"}");
    }

    fn request_once(request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept connection");
            handle_stream(stream).expect("handle stream");
        });

        let mut stream = TcpStream::connect(addr).expect("connect server");
        stream.write_all(request.as_bytes()).expect("write request");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown write");

        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        handle.join().expect("server thread");
        response
    }
}
