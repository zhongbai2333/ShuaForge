use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::OnceLock,
    thread,
};

const USERSCRIPT: &str = include_str!("../userscripts/shuaforge-question-exporter.user.js");
const SCRIPT_PATH: &str = "/shuaforge-question-exporter.user.js";
static INSTALL_URL: OnceLock<String> = OnceLock::new();

pub fn open_userscript_install_page() -> Result<String, String> {
    let script_url = ensure_server_url()?;
    let install_url = tampermonkey_install_url(&script_url);
    webbrowser::open(&install_url).map_err(|err| format!("打开浏览器失败：{err}"))?;
    Ok(install_url)
}

fn ensure_server_url() -> Result<String, String> {
    if let Some(url) = INSTALL_URL.get() {
        return Ok(url.clone());
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|err| format!("启动本地脚本安装服务失败：{err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("读取本地服务端口失败：{err}"))?
        .port();
    let url = format!("http://127.0.0.1:{port}{SCRIPT_PATH}");

    thread::Builder::new()
        .name("shuaforge-userscript-server".into())
        .spawn(move || serve(listener))
        .map_err(|err| format!("启动本地脚本安装服务线程失败：{err}"))?;

    let _ = INSTALL_URL.set(url.clone());
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

    loop {
        let mut header_line = String::new();
        let bytes = reader.read_line(&mut header_line)?;
        if bytes == 0 || header_line == "\r\n" || header_line == "\n" {
            break;
        }
    }

    let method = request_line.split_whitespace().next().unwrap_or("GET");

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");

    if method == "OPTIONS" {
        return write_empty_response(&mut stream, "204 No Content");
    }

    match path {
        SCRIPT_PATH => write_response_with_options(
            &mut stream,
            "200 OK",
            "text/javascript; charset=utf-8",
            USERSCRIPT,
            method == "HEAD",
        ),
        _ => write_response(
            &mut stream,
            "302 Found",
            "text/plain; charset=utf-8",
            "Redirecting to ShuaForge userscript installer...",
        ),
    }
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
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nExpires: 0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Private-Network: true\r\nCross-Origin-Resource-Policy: cross-origin\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
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
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Private-Network: true\r\nCross-Origin-Resource-Policy: cross-origin\r\nConnection: close\r\n\r\n"
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
        assert!(USERSCRIPT.contains("// @name         ShuaForge 题库导出器"));
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
        assert!(response.contains("Access-Control-Allow-Methods: GET, HEAD, OPTIONS"));
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
