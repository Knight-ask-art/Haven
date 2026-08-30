//! 发现：DLNA(SSDP M-SEARCH) + Chromecast(mDNS _googlecast._tcp) 合并。
//! 超时内并发探测，超时后合并去重返回。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use haven_application::services::CastDiscoveryPort;
use haven_application::wire::{CastDeviceDto, CastProtocolDto};
use haven_common::{AppError, ErrorKind};
use tokio::net::UdpSocket;

use super::device_registry;

const SSDP_ADDR: &str = "239.255.255.250:1900";
const SSDP_MX_SECS: u32 = 2;
const SSDP_ST_DLNA: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
const SSDP_ST_ROOT: &str = "upnp:rootdevice";

pub struct SsdpMdnsDiscovery {
    client: reqwest::Client,
}

impl Default for SsdpMdnsDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl SsdpMdnsDiscovery {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }

    async fn ssdp_discover(&self, timeout_ms: u32) -> Vec<CastDeviceDto> {
        let timeout = Duration::from_millis(timeout_ms as u64);
        let socket = match self.bind_ssdp_socket().await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let msearch_root = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: {SSDP_MX_SECS}\r\nST: {SSDP_ST_ROOT}\r\n\r\n"
        );
        let msearch_renderer = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: {SSDP_MX_SECS}\r\nST: {SSDP_ST_DLNA}\r\n\r\n"
        );
        let addr: SocketAddr = SSDP_ADDR.parse().unwrap();
        let _ = socket.send_to(msearch_root.as_bytes(), addr).await;
        let _ = socket.send_to(msearch_renderer.as_bytes(), addr).await;
        // 再发一次提升发现率
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = socket.send_to(msearch_root.as_bytes(), addr).await;

        let mut seen_location: HashSet<String> = HashSet::new();
        let mut devices = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut buf = vec![0u8; 8192];
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let recv = tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await;
            let (len, _peer) = match recv {
                Ok(Ok(v)) => v,
                _ => break,
            };
            let text = String::from_utf8_lossy(&buf[..len]);
            if let Some(loc) = parse_location(&text) {
                let loc = loc.trim().to_owned();
                if seen_location.contains(&loc) {
                    continue;
                }
                seen_location.insert(loc.clone());
                // 异步拉取设备描述（不阻塞主循环， spawn 并收集）
                if let Some(dev) = self.fetch_ssdp_device(&loc).await {
                    devices.push(dev);
                }
                if devices.len() >= 32 {
                    break;
                }
            }
        }
        devices
    }

    async fn bind_ssdp_socket(&self) -> std::io::Result<UdpSocket> {
        // 按 spec 优先 3339-3438，失败则 ephemeral
        for port in 3339..=3438 {
            if let Ok(s) = UdpSocket::bind(format!("0.0.0.0:{port}")).await {
                let _ = s.join_multicast_v4(
                    "239.255.255.250".parse().unwrap(),
                    "0.0.0.0".parse().unwrap(),
                );
                return Ok(s);
            }
        }
        let s = UdpSocket::bind("0.0.0.0:0").await?;
        let _ = s.join_multicast_v4(
            "239.255.255.250".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        );
        Ok(s)
    }

    async fn fetch_ssdp_device(&self, location: &str) -> Option<CastDeviceDto> {
        let resp = self.client.get(location).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let xml = resp.text().await.ok()?;
        let (friendly, model, av_url, rc_url) = parse_device_xml(&xml)?;
        // 仅保留提供 AVTransport 的设备（可投）
        if av_url.is_none() && rc_url.is_none() {
            return None;
        }
        let ip = extract_ip_from_url(location).unwrap_or_else(|| "0.0.0.0".into());
        // 解析 controlURL 的绝对地址（相对路径需以 LOCATION 为基）
        let control_url = av_url
            .as_deref()
            .and_then(|u| resolve_control_url(location, u))
            .or_else(|| {
                rc_url
                    .as_deref()
                    .and_then(|u| resolve_control_url(location, u))
            });
        let device_id = format!("dlna-{:x}", fx_hash(location));
        device_registry::insert(
            device_id.clone(),
            device_registry::DeviceInfo {
                ip: ip.clone(),
                protocol: CastProtocolDto::Dlna,
                control_url: control_url.clone(),
                location: Some(location.to_owned()),
            },
        );
        Some(CastDeviceDto {
            device_id,
            friendly_name: friendly.unwrap_or_else(|| "DLNA 设备".into()),
            ip,
            protocol: CastProtocolDto::Dlna,
            model_name: model,
        })
    }

    async fn mdns_discover(&self, timeout_ms: u32) -> Vec<CastDeviceDto> {
        let timeout = Duration::from_millis(timeout_ms as u64);
        let mdns = match mdns_sd::ServiceDaemon::new() {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        let receiver = match mdns.browse("_googlecast._tcp.local.") {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut devices: Vec<CastDeviceDto> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let recv = tokio::time::timeout(remaining, receiver.recv_async()).await;
            let event = match recv {
                Ok(Ok(e)) => e,
                _ => break,
            };
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(info) => {
                    let fullname = info.get_fullname().to_owned();
                    if seen.contains(&fullname) {
                        continue;
                    }
                    seen.insert(fullname.clone());
                    let ip = info
                        .get_addresses()
                        .iter()
                        .next()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "0.0.0.0".into());
                    let friendly = info
                        .get_property_val_str("fn")
                        .or_else(|| info.get_property_val_str("md"))
                        .unwrap_or_else(|| info.get_hostname().trim_end_matches('.'))
                        .to_owned();
                    let model = info.get_property_val_str("md").map(|s| s.to_owned());
                    let device_id = format!("cast-{:x}", fx_hash(&fullname));
                    device_registry::insert(
                        device_id.clone(),
                        device_registry::DeviceInfo {
                            ip: ip.clone(),
                            protocol: CastProtocolDto::Chromecast,
                            control_url: None,
                            location: None,
                        },
                    );
                    devices.push(CastDeviceDto {
                        device_id,
                        friendly_name: friendly,
                        ip,
                        protocol: CastProtocolDto::Chromecast,
                        model_name: model,
                    });
                    if devices.len() >= 16 {
                        break;
                    }
                }
                mdns_sd::ServiceEvent::SearchStopped(_) => break,
                _ => {}
            }
        }
        let _ = mdns.shutdown();
        devices
    }
}

#[async_trait]
impl CastDiscoveryPort for SsdpMdnsDiscovery {
    async fn discover(&self, timeout_ms: u32) -> Result<Vec<CastDeviceDto>, AppError> {
        if timeout_ms == 0 || timeout_ms > 10_000 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "timeout_ms 需在 1..10000",
                false,
            ));
        }
        // 并发 SSDP + mDNS，超时取两者合并
        let ssdp_fut = self.ssdp_discover(timeout_ms);
        let mdns_fut = self.mdns_discover(timeout_ms);
        let (ssdp_devices, mdns_devices) = tokio::join!(ssdp_fut, mdns_fut);
        let mut merged: Vec<CastDeviceDto> = Vec::new();
        let mut seen_id: HashSet<String> = HashSet::new();
        for d in ssdp_devices.into_iter().chain(mdns_devices) {
            if seen_id.contains(&d.device_id) {
                continue;
            }
            seen_id.insert(d.device_id.clone());
            merged.push(d);
        }
        Ok(merged)
    }
}

fn parse_location(text: &str) -> Option<String> {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("location:") {
            let v = line["location:".len()..].trim();
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

#[allow(clippy::type_complexity)]
fn parse_device_xml(
    xml: &str,
) -> Option<(
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    // 极简 XML 抽取（quick-xml 无需完整 DOM，字符串搜索足够且不引入复杂生命周期）
    // friendlyName
    let friendly = extract_tag(xml, "friendlyName");
    let model = extract_tag(xml, "modelName").or_else(|| extract_tag(xml, "modelDescription"));
    // 找所有 <service> 块，匹配 AVTransport / RenderingControl
    let mut av_url: Option<String> = None;
    let mut rc_url: Option<String> = None;
    let mut search_start = 0;
    while let Some(s) = xml[search_start..].find("<service>") {
        let abs_s = search_start + s;
        let Some(e) = xml[abs_s..].find("</service>") else {
            break;
        };
        let abs_e = abs_s + e + "</service>".len();
        let block = &xml[abs_s..abs_e];
        let ty = extract_tag(block, "serviceType").unwrap_or_default();
        let ctrl = extract_tag(block, "controlURL");
        if ty.contains("AVTransport") {
            av_url = ctrl.clone();
        } else if ty.contains("RenderingControl") {
            rc_url = ctrl.clone();
        }
        search_start = abs_e;
    }
    Some((friendly, model, av_url, rc_url))
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    let v = xml[s..e].trim();
    if v.is_empty() {
        None
    } else {
        Some(html_unescape(v))
    }
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn extract_ip_from_url(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let host = after.split('/').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_owned())
    }
}

fn resolve_control_url(location: &str, control: &str) -> Option<String> {
    if control.starts_with("http://") || control.starts_with("https://") {
        return Some(control.to_owned());
    }
    // 以 LOCATION 为基解析相对 URL
    let base = location
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or(location);
    if control.starts_with('/') {
        // 取 scheme+host
        if let Some((scheme, rest)) = location.split_once("://") {
            if let Some(host_end) = rest.find('/') {
                let host = &rest[..host_end];
                return Some(format!("{scheme}://{host}{control}"));
            }
            return Some(format!("{scheme}://{rest}{control}"));
        }
        Some(format!("{base}{control}"))
    } else {
        Some(format!("{base}/{control}"))
    }
}

fn fx_hash(s: &str) -> u64 {
    // 简易 FNV-1a 64
    let mut h: u64 = 14695981039346656037;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}
