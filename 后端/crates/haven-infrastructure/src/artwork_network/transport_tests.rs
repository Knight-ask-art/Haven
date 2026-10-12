//! `ArtworkTarget` 的物理传输回归：证明“已检查的 socket 集合”真的决定连接目的地。
//!
//! 这个测试完全离线：起一个本地 TCP 监听，用私有构造入口 `compose_target` 把固定
//! 地址换成该监听地址，再断言 `ArtworkTarget::build_request` 发出的请求确实落在同
//! 一个 socket 上，并拿回 HTTP 200 与正文。主机名刻意使用 `.invalid`（RFC 2606
//! 保留、不会解析）：如果 reqwest 的 DNS 覆盖表的键与它实际查询的名字不一致，覆盖
//! 就会落空并静默回退到系统解析，连接必然失败——这正是本测试要钉住的回归。尾点
//! 形式（`artwork-transport.invalid.`）单独覆盖，因为传输层用的是 URL authority 里
//! 的主机原文，任何归一化都会让覆盖表查不到键。
//!
//! 覆盖范围仅限 `ArtworkTarget -> reqwest -> 本地 socket` 这一跳：test-only 的环回
//! 地址只经由私有 `compose_target` 注入，生产入口 `resolve_remote_target` 仍然拒绝
//! 字面环回地址（见同模块的 literal loopback 拒绝测试）。公共 DNS、DoH 公网证明与
//! 系统 TUN/路由映射都不在本测试的证明范围内，这里也没有任何网络出口。

use std::net::SocketAddr;
use std::time::Duration;

use reqwest::{StatusCode, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::compose_target;

/// 每一步等待的硬上限：任何一步挂住都让测试失败，而不是永久阻塞测试线程。
const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// `validate_host` 归一化后的主机（小写、无尾点），与生产路径传给 `compose_target`
/// 的值一致。
const NORMALIZED_HOST: &str = "artwork-transport.invalid";
/// 裸主机与尾点主机：DNS 覆盖必须对两者都逐字命中。
const URL_HOSTS: [&str; 2] = ["artwork-transport.invalid", "artwork-transport.invalid."];
const RESPONSE_BODY: &str = "artwork-bytes";
const REQUEST_PATH: &str = "/poster.jpg";

#[tokio::test]
async fn pinned_target_connects_to_the_exact_checked_socket_for_bare_and_terminal_dot_hosts() {
    for raw_host in URL_HOSTS {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket: SocketAddr = listener.local_addr().unwrap();
        let raw_url = format!("http://{}:{}{}", raw_host, socket.port(), REQUEST_PATH);
        let url: Url = raw_url.parse().unwrap();
        assert_eq!(url.host_str(), Some(raw_host), "URL 必须逐字保留主机原文");

        // 唯一允许 test-only 环回地址出现的入口：私有 compose_target。
        let target = compose_target(
            &url,
            Some("cms10"),
            NORMALIZED_HOST.to_owned(),
            vec![socket],
        );
        assert_eq!(target.pinned_host.as_deref(), Some(raw_host), "{raw_host}");
        assert_eq!(target.addresses, vec![socket], "{raw_host}");

        let server = tokio::spawn(serve_one_connection(listener));
        // 域名没有系统 DNS 答案：能连上就说明请求被覆盖表送到了 socket 上。
        let response = step(target.build_request().unwrap().send())
            .await
            .expect("请求必须在超时内完成")
            .expect("固定到已检查 socket 的连接必须成功");
        assert_eq!(response.status(), StatusCode::OK, "{raw_host}");
        assert_eq!(response.text().await.unwrap(), RESPONSE_BODY, "{raw_host}");

        let request_head = step(server).await.expect("本地服务不得挂住").unwrap();
        assert_request_head(&request_head, raw_host, socket.port());
    }
}

/// 只服务一个连接：读完请求头、回 200 与固定正文，再把收到的请求头交给断言。
async fn serve_one_connection(listener: TcpListener) -> Vec<u8> {
    let (mut stream, _) = step(listener.accept())
        .await
        .expect("本地监听不得无限等待")
        .expect("连接必须建立");
    let received = step(read_request_head(&mut stream))
        .await
        .expect("请求头必须在超时内到齐");
    stream
        .write_all(&ok_response())
        .await
        .expect("响应必须写出");
    stream.flush().await.expect("响应必须刷出");
    received
}

async fn read_request_head(stream: &mut TcpStream) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let read = stream.read(&mut buffer).await.expect("读取请求失败");
        if read == 0 {
            break;
        }
        received.extend_from_slice(&buffer[..read]);
        if received.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    received
}

fn ok_response() -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        RESPONSE_BODY.len(),
        RESPONSE_BODY
    )
    .into_bytes()
}

/// 请求必须是 origin-form，`Host` 必须逐字等于 URL 原文里的主机（含可能的尾点）。
/// 主机原文一旦被归一化，DNS 覆盖表就会查不到键并静默落回系统解析。
fn assert_request_head(head: &[u8], raw_host: &str, port: u16) {
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    assert!(
        head.starts_with(&format!("get {REQUEST_PATH} http/1.1")),
        "请求行异常: {head}"
    );
    let host = head
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap_or_else(|| panic!("请求缺少 Host 头: {head}"));
    assert_eq!(
        host.trim(),
        format!("{raw_host}:{port}"),
        "Host 必须逐字使用 URL 原文主机: {head}"
    );
}

/// 给任意一步等待加上硬上限，避免监听或读取把测试线程挂住。
async fn step<F: std::future::Future>(future: F) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(STEP_TIMEOUT, future).await
}
