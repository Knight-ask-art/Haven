//! Google Drive 只读能力的生产组合（Infrastructure）。
//!
//! 桌面组合根只需要 [`GoogleDrivePorts::new`]：它不读环境变量、不读配置文件、不发网络
//! 请求、也不打开浏览器。缺少 client id 不是构造错误——未配置会在
//! [`CloudDriveAuthPort::start`] 处以 `CLOUD_OAUTH_NOT_CONFIGURED` 拒绝，应用照常启动。
//!
//! `auth` 与 `drive` 共享**同一个** `Arc<ReqwestGoogleTransport>`：Drive 请求与 OAuth
//! 令牌请求因此走同一套固定端点策略；传输按请求做 DNS pinning，组合根不创建第二条策略路径，
//! 也无法绕开 `http` 模块的端点白名单。令牌、授权码与 state 只活在端口实现内部：本模块
//! 不持有、不读取、不打印任何秘密值。

use std::sync::Arc;

use haven_application::services::cloud_storage::ports::{
    CloudDriveAuthPort, CloudDrivePort, CloudOAuthBrowserPort,
};
use haven_domain::credential::SecretString;

use crate::cloud_drive::auth::GoogleDriveAuth;
use crate::cloud_drive::drive::GoogleDriveClient;
use crate::cloud_drive::http::ReqwestGoogleTransport;
use crate::cloud_drive::tokens::GoogleOAuthTokens;

/// 桌面组合根需要的云盘端口对；两者共享同一份 HTTP 传输。
pub struct GoogleDrivePorts {
    /// OAuth 授权生命周期：单飞发起、loopback 回调、令牌交换与刷新。
    pub auth: Arc<dyn CloudDriveAuthPort>,
    /// Drive 只读访问：list / stat / alt=media / about。
    pub drive: Arc<dyn CloudDrivePort>,
}

impl GoogleDrivePorts {
    /// 组装生产端口。`client_id` / `client_secret` 缺失或非法都不是构造失败：授权入口会在
    /// 打开浏览器之前以固定错误码拒绝，调用方据此提示用户补齐配置。
    pub fn new(
        client_id: Option<String>,
        client_secret: Option<SecretString>,
        browser: Arc<dyn CloudOAuthBrowserPort>,
    ) -> Self {
        // 唯一一份策略传输；每次出网仍按 HTTP owner 重新验证与固定目标地址。
        let transport = Arc::new(ReqwestGoogleTransport::new());
        let auth = Arc::new(GoogleDriveAuth::new(
            Arc::new(GoogleOAuthTokens::new(
                client_id,
                client_secret,
                transport.clone(),
            )),
            browser,
        ));
        let drive = Arc::new(GoogleDriveClient::new(transport));
        Self { auth, drive }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use haven_common::{AppError, ErrorKind};

    /// 只满足端口形状、不打开任何东西的浏览器替身。
    struct NoopBrowser;

    #[async_trait::async_trait]
    impl CloudOAuthBrowserPort for NoopBrowser {
        async fn open_google_authorization(&self, _url: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 配置缺失是 **start** 时的固定错误，不是构造时的 panic。
    #[tokio::test]
    async fn missing_client_id_surfaces_at_start_not_at_construction() {
        let ports = GoogleDrivePorts::new(None, None, Arc::new(NoopBrowser));
        let error = ports
            .auth
            .start(&[])
            .await
            .expect_err("未配置 OAuth 客户端时必须拒绝 start");
        assert_eq!(error.code().as_str(), "CLOUD_OAUTH_NOT_CONFIGURED");
        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }
}
