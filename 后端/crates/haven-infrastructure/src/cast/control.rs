//! 控制：DLNA SOAP AVTransport；Chromecast 播放/停止尚未实现，状态返回 Unknown。

use async_trait::async_trait;
use haven_application::services::CastControlPort;
use haven_application::wire::CastTransportStateDto;
use haven_common::{AppError, ErrorKind};

use super::device_registry;

pub struct SoapCastControl {
    client: reqwest::Client,
}

impl Default for SoapCastControl {
    fn default() -> Self {
        Self::new()
    }
}

impl SoapCastControl {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }

    fn is_chromecast(device_id: &str) -> bool {
        device_id.starts_with("cast-")
    }
}

#[async_trait]
impl CastControlPort for SoapCastControl {
    async fn play(&self, device_id: &str, lan_url: &str, title: &str) -> Result<(), AppError> {
        if device_id.trim().is_empty() || lan_url.trim().is_empty() {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "设备或地址缺失",
                false,
            ));
        }
        if Self::is_chromecast(device_id) {
            return self.play_chromecast(device_id, lan_url, title).await;
        }
        self.play_dlna(device_id, lan_url).await
    }

    async fn stop(&self, device_id: &str) -> Result<(), AppError> {
        if Self::is_chromecast(device_id) {
            return self.stop_chromecast(device_id).await;
        }
        self.stop_dlna(device_id).await
    }

    async fn status(&self, device_id: &str) -> Result<CastTransportStateDto, AppError> {
        if Self::is_chromecast(device_id) {
            return Ok(CastTransportStateDto::Unknown);
        }
        self.status_dlna(device_id).await
    }

    async fn position(&self, _device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError> {
        Ok((None, None))
    }
}

impl SoapCastControl {
    async fn play_dlna(&self, device_id: &str, lan_url: &str) -> Result<(), AppError> {
        let info = device_registry::get(device_id).ok_or_else(|| {
            AppError::new(
                "CAST_DEVICE_UNREACHABLE",
                ErrorKind::NotFound,
                "设备不可达或已离线",
                false,
            )
        })?;
        let control_url = info.control_url.ok_or_else(|| {
            AppError::new(
                "CAST_DEVICE_UNREACHABLE",
                ErrorKind::NotFound,
                "设备不支持投屏",
                false,
            )
        })?;
        // SetAVTransportURI
        let set_body = format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData></CurrentURIMetaData></u:SetAVTransportURI></s:Body></s:Envelope>"#,
            xml_escape(lan_url)
        );
        let resp = self
            .client
            .post(&control_url)
            .header("Content-Type", r#"text/xml; charset="utf-8""#)
            .header(
                "SOAPACTION",
                r#""urn:schemas-upnp-org:service:AVTransport:1#SetAVTransportURI""#,
            )
            .body(set_body)
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    "CAST_DEVICE_UNREACHABLE",
                    ErrorKind::Network,
                    format!("投屏连接失败: {e}"),
                    true,
                )
            })?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                "CAST_LOAD_FAILED",
                ErrorKind::Network,
                format!("设备拒绝播放: {}", truncate(&body, 200)),
                true,
            ));
        }
        // Play
        let play_body = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID><Speed>1</Speed></u:Play></s:Body></s:Envelope>"#;
        let resp = self
            .client
            .post(&control_url)
            .header("Content-Type", r#"text/xml; charset="utf-8""#)
            .header(
                "SOAPACTION",
                r#""urn:schemas-upnp-org:service:AVTransport:1#Play""#,
            )
            .body(play_body.to_owned())
            .send()
            .await
            .map_err(|e| {
                AppError::new(
                    "CAST_DEVICE_UNREACHABLE",
                    ErrorKind::Network,
                    format!("播放指令失败: {e}"),
                    true,
                )
            })?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::new(
                "CAST_LOAD_FAILED",
                ErrorKind::Network,
                format!("播放失败: {}", truncate(&body, 200)),
                true,
            ));
        }
        Ok(())
    }

    async fn stop_dlna(&self, device_id: &str) -> Result<(), AppError> {
        let info = device_registry::get(device_id).ok_or_else(|| {
            AppError::new(
                "CAST_DEVICE_UNREACHABLE",
                ErrorKind::NotFound,
                "设备不可达或已离线",
                true,
            )
        })?;
        let control_url = info.control_url.ok_or_else(|| {
            AppError::new(
                "CAST_DEVICE_UNREACHABLE",
                ErrorKind::NotFound,
                "设备不支持投屏控制",
                false,
            )
        })?;
        let body = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:Stop xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID></u:Stop></s:Body></s:Envelope>"#;
        let response = self
            .client
            .post(&control_url)
            .header("Content-Type", r#"text/xml; charset="utf-8""#)
            .header(
                "SOAPACTION",
                r#""urn:schemas-upnp-org:service:AVTransport:1#Stop""#,
            )
            .body(body.to_owned())
            .send()
            .await
            .map_err(|_| {
                AppError::new(
                    "CAST_STOP_FAILED",
                    ErrorKind::Network,
                    "停止投屏失败，请检查设备连接后重试",
                    true,
                )
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "CAST_STOP_FAILED",
                ErrorKind::Network,
                "设备未确认停止投屏，请重试",
                true,
            ));
        }
        Ok(())
    }

    async fn status_dlna(&self, device_id: &str) -> Result<CastTransportStateDto, AppError> {
        let Some(info) = device_registry::get(device_id) else {
            return Ok(CastTransportStateDto::Unknown);
        };
        let Some(control_url) = info.control_url else {
            return Ok(CastTransportStateDto::Unknown);
        };
        let body = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:GetTransportInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID></u:GetTransportInfo></s:Body></s:Envelope>"#;
        let resp = match self
            .client
            .post(&control_url)
            .header("Content-Type", r#"text/xml; charset="utf-8""#)
            .header(
                "SOAPACTION",
                r#""urn:schemas-upnp-org:service:AVTransport:1#GetTransportInfo""#,
            )
            .body(body.to_owned())
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(CastTransportStateDto::Unknown),
        };
        // 简易解析：找 <CurrentTransportState>PLAYING|PAUSED_PLAYBACK|STOPPED|TRANSITIONING
        let text = resp.text().await.unwrap_or_default();
        if text.contains("PLAYING") {
            Ok(CastTransportStateDto::Playing)
        } else if text.contains("PAUSED_PLAYBACK") {
            Ok(CastTransportStateDto::Paused)
        } else if text.contains("STOPPED") {
            Ok(CastTransportStateDto::Stopped)
        } else if text.contains("TRANSITIONING") {
            Ok(CastTransportStateDto::Transitioning)
        } else if text.contains("NO_MEDIA_PRESENT") {
            Ok(CastTransportStateDto::NoMedia)
        } else {
            Ok(CastTransportStateDto::Unknown)
        }
    }

    async fn play_chromecast(
        &self,
        _device_id: &str,
        _lan_url: &str,
        _title: &str,
    ) -> Result<(), AppError> {
        Err(chromecast_unsupported())
    }

    async fn stop_chromecast(&self, _device_id: &str) -> Result<(), AppError> {
        Err(chromecast_unsupported())
    }
}

fn chromecast_unsupported() -> AppError {
    AppError::new(
        "CAST_PROTOCOL_UNSUPPORTED",
        ErrorKind::Unsupported,
        "暂不支持 Chromecast 控制，请使用支持 DLNA 的设备",
        false,
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 截取错误正文前 `n` 字节作为错误详情；设备返回的 UTF-8 可能在任意字节处被
/// 截断（非 ASCII 字符跨界），因此必须回退到最近的字符边界而不是直接切片。
fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_keeps_long_ascii_body() {
        let body = "x".repeat(150);
        assert_eq!(truncate(&body, 200), body);
    }

    #[test]
    fn truncate_does_not_panic_when_cut_crosses_utf8_boundary() {
        // 每个汉字 3 字节：199 字节处是字符中间，200 处也是。
        let body = "电".repeat(300);
        let out = truncate(&body, 200);
        assert!(out.ends_with('…'));
        assert_eq!(out.trim_end_matches('…'), "电".repeat(66));
        assert_eq!(out.len(), 66 * 3 + '…'.len_utf8());
    }

    #[test]
    fn truncate_mixed_body_stays_bounded_and_valid() {
        let body = format!("{}{}", "a".repeat(199), "视".repeat(50));
        let out = truncate(&body, 200);
        assert_eq!(out, format!("{}…", "a".repeat(199)));
        // 截断点恰好落在边界时保留完整前缀。
        assert_eq!(truncate("a视b", 4), "a视…");
        assert_eq!(truncate("a视", 5), "a视");
        assert_eq!(truncate("a视", 3), "a…");
    }
}
