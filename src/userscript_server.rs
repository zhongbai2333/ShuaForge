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
    let url = ensure_server_url()?;
    webbrowser::open(&url).map_err(|err| format!("打开浏览器失败：{err}"))?;
    Ok(url)
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

fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = handle_stream(stream);
    }
}

fn handle_stream(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");

    match path {
        SCRIPT_PATH => write_response(
            &mut stream,
            "200 OK",
            "application/javascript; charset=utf-8",
            USERSCRIPT,
        ),
        "/" => write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            install_html(),
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
    let extra_headers = if status == "302 Found" {
        format!("Location: {SCRIPT_PATH}\r\n")
    } else {
        String::new()
    };
    let headers = format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn install_html() -> &'static str {
    r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <title>安装 ShuaForge 题库导出器</title>
</head>
<body>
  <h1>安装 ShuaForge 题库导出器</h1>
  <p>如果浏览器没有自动打开脚本管理器安装页，请确认已安装 Tampermonkey、Violentmonkey 或脚本猫。</p>
  <p><a href="/shuaforge-question-exporter.user.js">点击打开 userscript 安装页</a></p>
</body>
</html>
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_userscript_has_metadata_header() {
        assert!(USERSCRIPT.starts_with("// ==UserScript=="));
        assert!(USERSCRIPT.contains("// @name         ShuaForge 题库导出器"));
        assert!(USERSCRIPT.contains("// ==/UserScript=="));
    }
}
