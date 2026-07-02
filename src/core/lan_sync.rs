use crate::{problem::Problem, store::AppStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DISCOVERY_PORT: u16 = 53492;
const HTTP_PREFERRED_PORT: u16 = 53493;
const DEVICE_ID_KEY: &str = "lan_sync_device_id";
const EXPORT_PATH: &str = "/sync/export";

static SYNC_SERVER: OnceLock<SyncServerState> = OnceLock::new();
static DISCOVERY: OnceLock<DiscoveryState> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDeckPackage {
    pub name: String,
    pub source_path: String,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPackage {
    pub app: String,
    pub version: u32,
    pub exported_at: u64,
    pub device_id: String,
    pub device_name: String,
    pub decks: Vec<SyncDeckPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAnnouncement {
    pub app: String,
    pub device_id: String,
    pub device_name: String,
    pub addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSyncPeer {
    pub device_id: String,
    pub device_name: String,
    pub addr: String,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SyncImportSummary {
    pub decks: usize,
    pub imported: usize,
    pub inserted: usize,
    pub updated: usize,
}

struct SyncServerState {
    addr: String,
}

struct DiscoveryState {
    peers: Mutex<HashMap<String, DiscoveredSyncPeer>>,
}

pub fn ensure_discovery_listener() {
    let _ = DISCOVERY.get_or_init(|| {
        let state = DiscoveryState {
            peers: Mutex::new(HashMap::new()),
        };
        thread::Builder::new()
            .name("shuaforge-lan-sync-discovery".into())
            .spawn(listen_for_announcements)
            .ok();
        state
    });
}

pub fn discovered_peers() -> Vec<DiscoveredSyncPeer> {
    ensure_discovery_listener();
    let now = now_secs();
    DISCOVERY
        .get()
        .and_then(|state| state.peers.lock().ok())
        .map(|mut peers| {
            peers.retain(|_, peer| now.saturating_sub(peer.last_seen) < 12);
            let mut list = peers.values().cloned().collect::<Vec<_>>();
            list.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
            list
        })
        .unwrap_or_default()
}

pub fn ensure_sync_server_running() -> Result<String, String> {
    ensure_discovery_listener();
    if let Some(state) = SYNC_SERVER.get() {
        return Ok(state.addr.clone());
    }

    let store = AppStore::open_default().map_err(|err| err.to_string())?;
    let device_id = ensure_device_id(&store)?;
    let device_name = default_device_name();
    let listener = TcpListener::bind(("0.0.0.0", HTTP_PREFERRED_PORT))
        .or_else(|_| TcpListener::bind("0.0.0.0:0"))
        .map_err(|err| format!("启动局域网同步服务失败：{err}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|err| format!("读取局域网同步服务端口失败：{err}"))?;
    let port = local_addr.port();
    let addr = resolve_lan_addr(port).unwrap_or_else(|| format!("127.0.0.1:{port}"));

    thread::Builder::new()
        .name("shuaforge-lan-sync-server".into())
        .spawn(move || serve_sync(listener))
        .map_err(|err| format!("启动局域网同步服务线程失败：{err}"))?;

    start_broadcasting(SyncAnnouncement {
        app: "ShuaForge".into(),
        device_id,
        device_name,
        addr: addr.clone(),
    });

    let _ = SYNC_SERVER.set(SyncServerState { addr: addr.clone() });
    Ok(addr)
}

pub fn fetch_and_import_from_peer(addr: &str) -> Result<SyncImportSummary, String> {
    let package = fetch_package(addr)?;
    import_package(&package)
}

fn fetch_package(addr: &str) -> Result<SyncPackage, String> {
    let mut stream = TcpStream::connect(addr).map_err(|err| format!("连接同步设备失败：{err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|err| err.to_string())?;
    let request =
        format!("GET {EXPORT_PATH} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("发送同步请求失败：{err}"))?;

    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|err| format!("读取同步响应失败：{err}"))?;
    if !status.contains("200") {
        return Err(format!("同步设备返回异常状态：{}", status.trim()));
    }
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|err| format!("读取同步响应头失败：{err}"))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim())
        {
            content_length = value.parse().unwrap_or(0);
        }
    }
    let mut body = Vec::new();
    if content_length > 0 {
        body.resize(content_length, 0);
        reader
            .read_exact(&mut body)
            .map_err(|err| format!("读取同步包失败：{err}"))?;
    } else {
        reader
            .read_to_end(&mut body)
            .map_err(|err| format!("读取同步包失败：{err}"))?;
    }
    serde_json::from_slice(&body).map_err(|err| format!("解析同步包失败：{err}"))
}

fn import_package(package: &SyncPackage) -> Result<SyncImportSummary, String> {
    if package.app != "ShuaForge" || package.version != 1 {
        return Err("同步包格式不兼容".into());
    }
    let mut store = AppStore::open_default().map_err(|err| err.to_string())?;
    let mut summary = SyncImportSummary::default();
    for deck in &package.decks {
        let source = format!(
            "lan-sync://{}/{}",
            package.device_id,
            deck.source_path.trim().trim_start_matches('/')
        );
        let mut problems = deck.problems.clone();
        for problem in &mut problems {
            if problem
                .deck_name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                problem.deck_name = Some(deck.name.clone());
            }
        }
        let import = store
            .import_problems(&problems, &source)
            .map_err(|err| err.to_string())?;
        summary.decks += 1;
        summary.imported += import.imported;
        summary.inserted += import.inserted;
        summary.updated += import.updated;
    }
    Ok(summary)
}

fn serve_sync(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = handle_sync_stream(stream);
    }
}

fn handle_sync_stream(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    loop {
        let mut header_line = String::new();
        let bytes = reader.read_line(&mut header_line)?;
        if bytes == 0 || header_line == "\r\n" || header_line == "\n" {
            break;
        }
    }

    let method = request_line.split_whitespace().next().unwrap_or("GET");
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    if method == "GET" && path == EXPORT_PATH {
        match build_package() {
            Ok(package) => {
                let body = serde_json::to_vec(&package).unwrap_or_default();
                write_response(
                    &mut stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    &body,
                )
            }
            Err(err) => write_response(
                &mut stream,
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                err.as_bytes(),
            ),
        }
    } else {
        write_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not Found",
        )
    }
}

fn build_package() -> Result<SyncPackage, String> {
    let store = AppStore::open_default().map_err(|err| err.to_string())?;
    let device_id = ensure_device_id(&store)?;
    let device_name = default_device_name();
    let mut decks = Vec::new();
    for deck in store.deck_cards().map_err(|err| err.to_string())? {
        let problems = store
            .load_deck_problems(deck.id)
            .map_err(|err| err.to_string())?;
        decks.push(SyncDeckPackage {
            name: deck.name,
            source_path: deck.source_path,
            problems,
        });
    }
    Ok(SyncPackage {
        app: "ShuaForge".into(),
        version: 1,
        exported_at: now_secs(),
        device_id,
        device_name,
        decks,
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn listen_for_announcements() {
    let Ok(socket) = UdpSocket::bind(format!("0.0.0.0:{DISCOVERY_PORT}")) else {
        return;
    };
    socket.set_broadcast(true).ok();
    socket.set_read_timeout(Some(Duration::from_secs(1))).ok();
    let mut buf = [0u8; 4096];
    loop {
        if let Ok((len, _src)) = socket.recv_from(&mut buf)
            && let Ok(ann) = serde_json::from_slice::<SyncAnnouncement>(&buf[..len])
            && ann.app == "ShuaForge"
        {
            let own_id = AppStore::open_default()
                .ok()
                .and_then(|store| ensure_device_id(&store).ok());
            if own_id.as_deref() == Some(&ann.device_id) {
                continue;
            }
            if let Some(state) = DISCOVERY.get()
                && let Ok(mut peers) = state.peers.lock()
            {
                peers.insert(
                    ann.device_id.clone(),
                    DiscoveredSyncPeer {
                        device_id: ann.device_id,
                        device_name: ann.device_name,
                        addr: ann.addr,
                        last_seen: now_secs(),
                    },
                );
            }
        }
    }
}

fn start_broadcasting(ann: SyncAnnouncement) {
    thread::Builder::new()
        .name("shuaforge-lan-sync-broadcast".into())
        .spawn(move || {
            let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
                return;
            };
            socket.set_broadcast(true).ok();
            let targets = broadcast_targets();
            loop {
                if let Ok(data) = serde_json::to_vec(&ann) {
                    for target in &targets {
                        let _ = socket.send_to(&data, target);
                    }
                }
                thread::sleep(Duration::from_secs(2));
            }
        })
        .ok();
}

fn broadcast_targets() -> Vec<String> {
    let mut targets = vec![format!("255.255.255.255:{DISCOVERY_PORT}")];
    for ip in local_ips() {
        let mut parts = ip.split('.').collect::<Vec<_>>();
        if parts.len() == 4 {
            parts[3] = "255";
            let target = format!("{}:{DISCOVERY_PORT}", parts.join("."));
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets
}

fn resolve_lan_addr(port: u16) -> Option<String> {
    local_ips()
        .into_iter()
        .next()
        .map(|ip| format!("{ip}:{port}"))
}

fn local_ips() -> Vec<String> {
    let mut ips = Vec::new();
    for target in [
        "8.8.8.8:80",
        "10.0.0.1:80",
        "172.16.0.1:80",
        "192.168.0.1:80",
        "192.168.1.1:80",
        "192.168.31.1:80",
    ] {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0")
            && socket.connect(target).is_ok()
            && let Ok(local) = socket.local_addr()
        {
            let ip = local.ip().to_string();
            if !ip.starts_with("127.") && !ip.starts_with("0.") && !ips.contains(&ip) {
                ips.push(ip);
            }
        }
    }
    ips
}

fn ensure_device_id(store: &AppStore) -> Result<String, String> {
    if let Some(id) = store
        .get_setting(DEVICE_ID_KEY)
        .map_err(|err| err.to_string())?
        .filter(|id| !id.trim().is_empty())
    {
        return Ok(id);
    }
    let seed = format!("{}:{}", default_device_name(), now_secs());
    let id = hex::encode(&Sha256::digest(seed.as_bytes())[..16]);
    store
        .set_setting(DEVICE_ID_KEY, &id)
        .map_err(|err| err.to_string())?;
    Ok(id)
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .map(|name| format!("ShuaForge · {name}"))
        .unwrap_or_else(|_| "ShuaForge 设备".into())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
