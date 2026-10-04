//! 有界 loopback HTTP 回调协议；只解析形状并返回固定响应。
use super::{MAX_HEADER_COUNT, MAX_REQUEST_BYTES, REQUEST_READ_TIMEOUT};
use crate::cloud_drive::handshake::LOOPBACK_CALLBACK_PATH;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) enum HeadDecision {
    /// 目标正好是 `/oauth2/callback`，携带原始 query。
    Callback(String),
    /// 与本次尝试无关但格式合法：回固定安全响应后继续监听。
    Ignore(u16),
    /// 违反 HTTP 形状：静默关闭，不消费尝试。
    Drop,
}

/// 整段请求头必须在 [`REQUEST_READ_TIMEOUT`] 内读完，且不超过 [`MAX_REQUEST_BYTES`]。
pub(super) async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    tokio::time::timeout(REQUEST_READ_TIMEOUT, read_head_inner(stream))
        .await
        .ok()
        .flatten()
}

async fn read_head_inner(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = zeroize::Zeroizing::new(Vec::with_capacity(512));
    let mut chunk = [0u8; 512];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return None;
        }
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            buffer.truncate(end);
            return Some(std::mem::take(&mut *buffer));
        }
    }
}

/// GET-only；Host 必须恰好出现一次并精确等于本次 loopback 监听地址；
/// Content-Length / Transfer-Encoding / 重复 Host / obs-fold 一律拒绝。
pub(super) fn evaluate_head(head: &[u8], port: u16) -> HeadDecision {
    let Ok(text) = std::str::from_utf8(head) else {
        return HeadDecision::Drop;
    };
    let mut lines = text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return HeadDecision::Drop;
    };
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return HeadDecision::Drop;
    };
    if parts.next().is_some() || version != "HTTP/1.1" {
        return HeadDecision::Drop;
    }
    let expected_host = format!("127.0.0.1:{port}");
    let mut host_seen = false;
    let mut header_count = 0usize;
    for line in lines {
        header_count += 1;
        if header_count > MAX_HEADER_COUNT || line.starts_with(' ') || line.starts_with('\t') {
            return HeadDecision::Drop;
        }
        let Some((name, value)) = line.split_once(':') else {
            return HeadDecision::Drop;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return HeadDecision::Drop;
        }
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" => {
                if host_seen || value != expected_host {
                    return HeadDecision::Drop;
                }
                host_seen = true;
            }
            // GET 没有正文：任何 framing 头（存在即拒绝）都不接受。
            "content-length" | "transfer-encoding" => return HeadDecision::Drop,
            _ => {}
        }
    }
    if !host_seen {
        return HeadDecision::Drop;
    }
    if method != "GET" {
        return HeadDecision::Ignore(405);
    }
    match callback_query(target) {
        Some(query) => HeadDecision::Callback(query),
        None => HeadDecision::Ignore(404),
    }
}

/// 目标必须正好是 `/oauth2/callback`（可带 query）；其它路径、fragment 与绝对地址都拒绝。
fn callback_query(target: &str) -> Option<String> {
    let rest = target.strip_prefix(LOOPBACK_CALLBACK_PATH)?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let query = rest.strip_prefix('?')?;
    if query.contains('#') {
        return None;
    }
    Some(query.to_owned())
}

/// 固定文案的安全响应；不回显请求里的任何内容。
pub(super) async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Bad Request",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::time::timeout(REQUEST_READ_TIMEOUT, async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "callback response timeout"))?
}
