//! Discord 通知后处理模块 / Discord Notification Post-processing Module
//!
//! 将录制信息（主播名、时间戳、时长、文件名、文件大小）和封面图
//! 通过 Discord Webhook 发送到指定频道。
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - 环境变量 `PP_INPUT`: 输入视频文件路径 / Input video file path via env var
//! - 标准输出 `OUTPUT:{path}`: 成功后输出视频路径 / Output video path on success
//! - 标准输出 `PROGRESS:{done}/{total}`: 进度上报 / Progress reporting
//! - 标准输出 `STATUS:{speed}`: 上传速度上报 / Upload speed reporting

use pp_utils::{
    emit_progress_step, find_cover, format_bytes, format_duration, format_speed, param, parse_stem,
    video_duration, PROGRESS_SCALE,
};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use socket2::{Socket, Domain, Type};

const DESCRIBE: &str = r#"{
  "id": "notify_discord",
  "name": "Discord 通知 0.1.2",
  "description": "将录制信息和封面图发送到 Discord Webhook",
  "params": [
    {
      "key": "webhook_url",
      "label": "Webhook URL",
      "type": "string",
      "default": ""
    },
    {
      "key": "proxy",
      "label": "代理地址（支持 http://、socks5://）",
      "type": "string",
      "default": ""
    },
    {
      "key": "username",
      "label": "Bot 显示名称",
      "type": "string",
      "default": "Recorder Bot"
    }
  ]
}"#;

/// 解析 URL，返回 (host, port, path)
fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let url = url.trim();
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else {
        return Err(format!("Unsupported URL scheme: {}", url));
    };
    let default_port: u16 = if scheme == "https" { 443 } else { 80 };
    let (authority, path) = if let Some(idx) = rest.find('/') {
        (&rest[..idx], rest[idx..].to_string())
    } else {
        (rest, "/".to_string())
    };
    let (host, port) = if let Some(idx) = authority.rfind(':') {
        let p: u16 = authority[idx + 1..].parse().map_err(|_| "invalid port".to_string())?;
        (authority[..idx].to_string(), p)
    } else {
        (authority.to_string(), default_port)
    };
    Ok((host, port, path))
}

/// 通过 CONNECT 隧道建立到目标的 TcpStream（HTTP 代理）
fn connect_via_http_proxy_stream(mut stream: TcpStream, target_host: &str, target_port: u16)
    -> Result<TcpStream, String>
{
    let req = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\nProxy-Connection: keep-alive\r\n\r\n",
        target_host, target_port, target_host, target_port
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("proxy CONNECT write: {}", e))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1];
    loop {
        stream.read_exact(&mut tmp).map_err(|e| format!("proxy CONNECT read: {}", e))?;
        buf.push(tmp[0]);
        if buf.ends_with(b"\r\n\r\n") { break; }
        if buf.len() > 4096 { return Err("proxy CONNECT response too large".to_string()); }
    }
    let resp = String::from_utf8_lossy(&buf);
    if !resp.starts_with("HTTP/1.1 200") && !resp.starts_with("HTTP/1.0 200") {
        return Err(format!("proxy CONNECT rejected: {}", resp.lines().next().unwrap_or("")));
    }
    Ok(stream)
}

/// 通过 SOCKS5 代理建立 TcpStream
fn connect_via_socks5_stream(mut stream: TcpStream, target_host: &str, target_port: u16)
    -> Result<TcpStream, String>
{
    stream.write_all(&[0x05, 0x01, 0x00]).map_err(|e| format!("socks5 write: {}", e))?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).map_err(|e| format!("socks5 read: {}", e))?;
    if resp[1] != 0x00 { return Err("socks5 auth not accepted".to_string()); }
    let host_bytes = target_host.as_bytes();
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    req.extend_from_slice(host_bytes);
    req.push((target_port >> 8) as u8);
    req.push((target_port & 0xff) as u8);
    stream.write_all(&req).map_err(|e| format!("socks5 write: {}", e))?;
    let mut resp2 = [0u8; 10];
    stream.read_exact(&mut resp2).map_err(|e| format!("socks5 read: {}", e))?;
    if resp2[1] != 0x00 { return Err(format!("socks5 connect rejected: {}", resp2[1])); }
    Ok(stream)
}

/// 建立到目标主机的 TCP 连接（支持 HTTP/SOCKS5 代理）
/// 将 SO_SNDBUF 设为 32KB，使 write_all 在缓冲区满时阻塞，从而获得真实进度。
fn tcp_connect(host: &str, port: u16, proxy: &str) -> Result<TcpStream, String> {
    // 用 socket2 建立连接，以便设置 SO_SNDBUF
    let make_stream = |addr: std::net::SocketAddr| -> Result<TcpStream, String> {
        let domain = if addr.is_ipv6() { Domain::IPV6 } else { Domain::IPV4 };
        let sock = Socket::new(domain, Type::STREAM, None)
            .map_err(|e| format!("socket create failed: {}", e))?;
        // 32 KB 发送缓冲区：让 write_all 在数据真正发出前阻塞，进度才是真实的
        sock.set_send_buffer_size(32 * 1024)
            .map_err(|e| format!("set SO_SNDBUF failed: {}", e))?;
        sock.connect(&addr.into())
            .map_err(|e| format!("connect failed: {}", e))?;
        Ok(sock.into())
    };

    if proxy.is_empty() {
        // 解析主机名
        let addrs: Vec<std::net::SocketAddr> = format!("{}:{}", host, port)
            .parse::<std::net::SocketAddr>()
            .map(|a| vec![a])
            .unwrap_or_else(|_| {
                std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
                    .map(|i| i.collect())
                    .unwrap_or_default()
            });
        let addr = addrs.into_iter().next()
            .ok_or_else(|| format!("could not resolve host: {}", host))?;
        let stream = make_stream(addr)?;
        stream.set_write_timeout(Some(Duration::from_secs(600))).ok();
        stream.set_read_timeout(Some(Duration::from_secs(600))).ok();
        return Ok(stream);
    }

    let (proxy_scheme, proxy_rest) = if let Some(r) = proxy.strip_prefix("socks5://") {
        ("socks5", r)
    } else if let Some(r) = proxy.strip_prefix("http://") {
        ("http", r)
    } else if let Some(r) = proxy.strip_prefix("https://") {
        ("http", r)
    } else {
        return Err(format!("Unsupported proxy scheme: {}", proxy));
    };
    let proxy_authority = proxy_rest.split('/').next().unwrap_or(proxy_rest);
    let (proxy_host, proxy_port) = if let Some(idx) = proxy_authority.rfind(':') {
        let p: u16 = proxy_authority[idx + 1..].parse().unwrap_or(1080);
        (&proxy_authority[..idx], p)
    } else {
        (proxy_authority, if proxy_scheme == "socks5" { 1080u16 } else { 8080u16 })
    };

    // 代理连接：先用 socket2 连到代理，设置小缓冲区，再做隧道握手
    let proxy_addrs: Vec<std::net::SocketAddr> = format!("{}:{}", proxy_host, proxy_port)
        .parse::<std::net::SocketAddr>()
        .map(|a| vec![a])
        .unwrap_or_else(|_| {
            std::net::ToSocketAddrs::to_socket_addrs(&(proxy_host, proxy_port))
                .map(|i| i.collect())
                .unwrap_or_default()
        });
    let proxy_addr = proxy_addrs.into_iter().next()
        .ok_or_else(|| format!("could not resolve proxy: {}", proxy_host))?;
    let domain = if proxy_addr.is_ipv6() { Domain::IPV6 } else { Domain::IPV4 };
    let sock = Socket::new(domain, Type::STREAM, None)
        .map_err(|e| format!("socket create failed: {}", e))?;
    sock.set_send_buffer_size(32 * 1024)
        .map_err(|e| format!("set SO_SNDBUF failed: {}", e))?;
    sock.connect(&proxy_addr.into())
        .map_err(|e| format!("proxy connect failed: {}", e))?;
    let stream: TcpStream = sock.into();
    stream.set_write_timeout(Some(Duration::from_secs(60))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();

    let stream = if proxy_scheme == "socks5" {
        connect_via_socks5_stream(stream, host, port)?
    } else {
        connect_via_http_proxy_stream(stream, host, port)?
    };
    stream.set_write_timeout(Some(Duration::from_secs(600))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(600))).ok();
    Ok(stream)
}

/// 通过 TLS 包装 TcpStream（用于 HTTPS）
fn tls_wrap(stream: TcpStream, host: &str) -> Result<rustls_wrapper::TlsStream, String> {
    rustls_wrapper::wrap(stream, host)
}

mod rustls_wrapper {
    use std::io::{self, Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, StreamOwned};

    pub struct TlsStream(StreamOwned<ClientConnection, TcpStream>);

    impl Read for TlsStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> { self.0.read(buf) }
    }
    impl Write for TlsStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> { self.0.write(buf) }
        fn flush(&mut self) -> io::Result<()> { self.0.flush() }
    }

    pub fn wrap(stream: TcpStream, host: &str) -> Result<TlsStream, String> {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|e| format!("invalid server name '{}': {}", host, e))?;
        let conn = ClientConnection::new(Arc::new(config), server_name)
            .map_err(|e| format!("TLS init failed: {}", e))?;
        Ok(TlsStream(StreamOwned::new(conn, stream)))
    }
}

/// 将 multipart body 直接写入 stream，同时上报真实进度和速度。
/// 这是获得真实上传进度的关键：直接写入 TCP/TLS stream = 直接发送到网络。
fn write_multipart_with_progress(
    stream: &mut dyn Write,
    header_bytes: &[u8],   // HTTP 请求头
    pre_file: &[u8],       // multipart 文件字段之前的部分（payload_json + 文件头）
    img_bytes: &[u8],      // 图片数据
    post_file: &[u8],      // multipart 结束边界
) -> io::Result<()> {
    let total = (pre_file.len() + img_bytes.len() + post_file.len()) as u64;
    let mut done: u64 = 0;
    let mut last_reported: u64 = u64::MAX;
    let mut speed_bytes: u64 = 0;
    let mut speed_last = Instant::now();

    // 写 HTTP 头
    stream.write_all(header_bytes)?;

    // 写 multipart 前缀（payload_json + 文件头），不计入进度（很小）
    stream.write_all(pre_file)?;

    // 分块写入图片数据，实时上报进度
    const CHUNK: usize = 32 * 1024; // 32 KB per chunk
    let mut offset = 0usize;
    while offset < img_bytes.len() {
        let end = (offset + CHUNK).min(img_bytes.len());
        stream.write_all(&img_bytes[offset..end])?;
        let n = (end - offset) as u64;
        done += n;
        speed_bytes += n;
        offset = end;

        let scaled = ((done as u128 * PROGRESS_SCALE as u128) / total as u128)
            .min(PROGRESS_SCALE as u128) as u64;
        if scaled != last_reported {
            last_reported = scaled;
            println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
        }
        let elapsed = speed_last.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let bps = speed_bytes as f64 / elapsed.as_secs_f64();
            println!("STATUS:{}", format_speed(bps));
            speed_bytes = 0;
            speed_last = Instant::now();
        }
    }

    // 写 multipart 结束边界
    stream.write_all(post_file)?;
    stream.flush()?;
    Ok(())
}

/// 读取 HTTP 响应，返回状态码和 body
fn read_http_response(stream: &mut dyn Read) -> Result<(u16, String), String> {
    // 读取响应头直到 \r\n\r\n
    let mut header_buf = Vec::new();
    let mut tmp = [0u8; 1];
    loop {
        stream.read_exact(&mut tmp).map_err(|e| format!("read response: {}", e))?;
        header_buf.push(tmp[0]);
        if header_buf.ends_with(b"\r\n\r\n") { break; }
        if header_buf.len() > 65536 { return Err("response header too large".to_string()); }
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let status: u16 = header_str
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // 解析 Content-Length 或 Transfer-Encoding: chunked
    let content_length: Option<usize> = header_str.lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok());
    let is_chunked = header_str.lines()
        .any(|l| l.to_lowercase().contains("transfer-encoding") && l.to_lowercase().contains("chunked"));

    let body = if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).unwrap_or(());
        String::from_utf8_lossy(&buf).to_string()
    } else if is_chunked {
        let mut body = Vec::new();
        loop {
            // 读取 chunk size 行
            let mut size_line = Vec::new();
            loop {
                stream.read_exact(&mut tmp).unwrap_or(());
                size_line.push(tmp[0]);
                if size_line.ends_with(b"\r\n") { break; }
            }
            let size_str = String::from_utf8_lossy(&size_line).trim().to_string();
            let chunk_size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
            if chunk_size == 0 { break; }
            let mut chunk = vec![0u8; chunk_size];
            stream.read_exact(&mut chunk).unwrap_or(());
            body.extend_from_slice(&chunk);
            // 读取 \r\n
            stream.read_exact(&mut [0u8; 2]).unwrap_or(());
        }
        String::from_utf8_lossy(&body).to_string()
    } else {
        String::new()
    };

    Ok((status, body))
}

/// 执行一次 Discord Webhook 请求（含封面图或纯文字）。
fn send_once(
    webhook_url: &str,
    proxy: &str,
    bot_name: &str,
    content: &str,
    cover: Option<&PathBuf>,
) -> Result<(), String> {
    let (host, port, path) = parse_url(webhook_url)?;
    let is_https = port == 443;

    if let Some(img_path) = cover {
        let img_bytes =
            fs::read(img_path).map_err(|e| format!("Failed to read cover image: {}", e))?;
        let img_name = img_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cover.jpg")
            .to_string();
        let mime = if img_name.ends_with(".png") { "image/png" }
            else if img_name.ends_with(".webp") { "image/webp" }
            else { "image/jpeg" };

        let payload_json =
            serde_json::json!({ "username": bot_name, "content": content }).to_string();
        let boundary = "----RustBoundary7f3a9b2c";

        // 构建 multipart 前缀（payload_json 部分 + 文件头）
        let mut pre_file: Vec<u8> = Vec::new();
        let pj_header = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n",
            b = boundary
        );
        pre_file.extend_from_slice(pj_header.as_bytes());
        pre_file.extend_from_slice(payload_json.as_bytes());
        pre_file.extend_from_slice(b"\r\n");
        let file_header = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{n}\"\r\nContent-Type: {m}\r\n\r\n",
            b = boundary, n = img_name, m = mime
        );
        pre_file.extend_from_slice(file_header.as_bytes());

        // 构建 multipart 结束边界
        let mut post_file: Vec<u8> = Vec::new();
        post_file.extend_from_slice(b"\r\n");
        post_file.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let body_len = pre_file.len() + img_bytes.len() + post_file.len();
        let content_type = format!("multipart/form-data; boundary={}", boundary);

        // 构建 HTTP 请求头
        let http_header = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
            path = path, host = host, ct = content_type, len = body_len
        );

        println!("PROGRESS:0/{}", PROGRESS_SCALE);
        let upload_start = Instant::now();

        let tcp = tcp_connect(&host, port, proxy)?;

        if is_https {
            let mut tls = tls_wrap(tcp, &host)?;
            write_multipart_with_progress(
                &mut tls,
                http_header.as_bytes(),
                &pre_file,
                &img_bytes,
                &post_file,
            ).map_err(|e| format!("write failed: {}", e))?;
            let elapsed = upload_start.elapsed();
            if elapsed.as_secs_f64() > 0.0 {
                println!("STATUS:{}", format_speed(body_len as f64 / elapsed.as_secs_f64()));
            }
            let (status, body) = read_http_response(&mut tls)?;
            if status != 200 && status != 204 {
                return Err(format!("Discord returned {}: {}", status, body));
            }
        } else {
            let mut tcp = tcp;
            write_multipart_with_progress(
                &mut tcp,
                http_header.as_bytes(),
                &pre_file,
                &img_bytes,
                &post_file,
            ).map_err(|e| format!("write failed: {}", e))?;
            let elapsed = upload_start.elapsed();
            if elapsed.as_secs_f64() > 0.0 {
                println!("STATUS:{}", format_speed(body_len as f64 / elapsed.as_secs_f64()));
            }
            let (status, body) = read_http_response(&mut tcp)?;
            if status != 200 && status != 204 {
                return Err(format!("Discord returned {}: {}", status, body));
            }
        }
    } else {
        // 无封面图：纯文字消息，用 ureq 发送即可
        let payload = serde_json::json!({ "username": bot_name, "content": content }).to_string();
        let body_len = payload.len();
        let http_header = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
            path = path, host = host, len = body_len
        );
        let tcp = tcp_connect(&host, port, proxy)?;
        if is_https {
            let mut tls = tls_wrap(tcp, &host)?;
            tls.write_all(http_header.as_bytes()).map_err(|e| format!("write: {}", e))?;
            tls.write_all(payload.as_bytes()).map_err(|e| format!("write: {}", e))?;
            tls.flush().map_err(|e| format!("flush: {}", e))?;
            let (status, body) = read_http_response(&mut tls)?;
            if status != 200 && status != 204 {
                return Err(format!("Discord returned {}: {}", status, body));
            }
        } else {
            let mut tcp = tcp;
            tcp.write_all(http_header.as_bytes()).map_err(|e| format!("write: {}", e))?;
            tcp.write_all(payload.as_bytes()).map_err(|e| format!("write: {}", e))?;
            tcp.flush().map_err(|e| format!("flush: {}", e))?;
            let (status, body) = read_http_response(&mut tcp)?;
            if status != 200 && status != 204 {
                return Err(format!("Discord returned {}: {}", status, body));
            }
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);
    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    let webhook_url = param("webhook_url", "");
    if webhook_url.is_empty() {
        return Err("webhook_url is required".to_string());
    }
    let proxy = param("proxy", "");
    let bot_name = param("username", "Recorder Bot");

    emit_progress_step(0, 3);

    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
    let (model_name, timestamp) = parse_stem(stem);
    let file_size = fs::metadata(&input).map(|m| m.len()).unwrap_or(0);
    let duration = video_duration(&input).unwrap_or(0.0);

    let content = format!(
        "**ModelName:** `#{model}`\n\
         **Timestamp:** `{ts}`\n\
         **Duration:** `{dur}`\n\
         **FileName:** `{name}`\n\
         **FileSize:** `{size}`",
        model = model_name,
        ts = if timestamp.is_empty() { "—".to_string() } else { timestamp },
        dur = format_duration(duration),
        name = input.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        size = format_bytes(file_size),
    );

    emit_progress_step(1, 3);

    let cover = find_cover(&input);

    const MAX_ATTEMPTS: u32 = 5;
    const RETRY_DELAYS: [u64; 4] = [30, 60, 90, 120];

    let mut attempt = 0u32;
    loop {
        let result = send_once(&webhook_url, &proxy, &bot_name, &content, cover.as_ref());
        match result {
            Ok(()) => break,
            Err(e) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
                let delay = RETRY_DELAYS[(attempt as usize - 1).min(RETRY_DELAYS.len() - 1)];
                eprintln!(
                    "Discord request failed (attempt {}/{}): {}. retrying in {}s…",
                    attempt, MAX_ATTEMPTS, e, delay
                );
                std::thread::sleep(Duration::from_secs(delay));
            }
        }
    }

    emit_progress_step(3, 3);
    println!("OUTPUT:{}", input.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", DESCRIBE);
        return;
    }
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
