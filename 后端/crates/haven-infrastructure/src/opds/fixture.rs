//! 离线目录夹具（**仅测试**，生产构建里整个模块不存在）。
//!
//! OPDS 与 CMS10 适配器的测试共用同一套受控夹具：
//! - [`FixtureServer`]：一个**有界**的本地 HTTP 服务器。每个连接在读完请求头之后
//!   立刻写出一个定长响应（`Content-Length` + `Connection: close`）并关闭，读写都有
//!   超时；服务器自己持有关停标志并在 [`Drop`] 时 join。测试因此不会退化成「等客户端
//!   超时」，也不会出现「读到请求头就不再回应」的假成功路径。非阻塞只用在**监听**套接字
//!   上（轮询关停标志）；每条 accepted 连接在第一次读之前显式恢复阻塞，否则 Windows 上
//!   继承下来的非阻塞模式会让读立刻返回 `WouldBlock`，读超时形同虚设。
//! - 回环 URL 夹具判定：`allow_loopback_fixture` 这条 `cfg(test)` 缝隙让离线传输夹具可用，
//!   它**绕过**生产 URL 策略与 DNS（回环地址直接钉到字面地址，只做一份更窄的形状检查）。
//!   因此使用夹具的用例只证明流水线行为，不证明策略/DNS 判定；「没有缝隙时回环地址一律被
//!   拒绝」由独立的生产策略用例钉住（`request_tests`）。
//! - 来源登记替身：直接写 `sources` 设置行的夹具。回环地址是保存路径**故意**拒绝的
//!   目标，因此夹具必须绕过保存校验——但种类、配置版本与凭据归属仍由注册表自己的
//!   代码解释。
//! - [`RecordingImageProxy`]：只记录登记调用、不发任何网络请求的图片登记替身。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use haven_application::services::SourceRegistryService;
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::ImageProxyRepository;

use crate::http_security::ResolvedHttpTarget;

// ---------- 回环夹具地址判定 ----------

pub(crate) fn is_loopback_http_url(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback())
}

/// 夹具专用：这条缝隙**不调用**共享出站策略，而是独立复述一份更窄的形状要求
/// （http(s)、无 userinfo、无 fragment、有端口）。它比共享策略宽松（回环主机照常接受），
/// 因此走这里的用例不能用来证明生产策略。生产路径没有这个入口。
pub(crate) fn parse_loopback_fixture_url(raw: &str) -> Result<reqwest::Url, AppError> {
    let url = reqwest::Url::parse(raw).map_err(|_| {
        AppError::new(
            "SOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "测试目录地址无效",
            true,
        )
    })?;
    if !is_loopback_http_url(raw)
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default().is_none()
    {
        return Err(AppError::new(
            "SOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "测试目录地址不安全",
            true,
        ));
    }
    Ok(url)
}

/// 回环地址直接钉到字面地址，不走 DNS。非回环返回 `None`，调用方回到生产解析路径。
pub(crate) fn loopback_fixture_target(
    url: &reqwest::Url,
) -> Option<Result<ResolvedHttpTarget, AppError>> {
    let host = url.host_str()?;
    let ip = host.parse::<IpAddr>().ok()?;
    if !ip.is_loopback() {
        return None;
    }
    let port = url.port_or_known_default()?;
    Some(Ok(ResolvedHttpTarget {
        url: url.clone(),
        dns_name: None,
        addresses: vec![SocketAddr::new(ip, port)],
    }))
}

// ---------- 有界本地目录服务器 ----------

pub(crate) struct FixtureResponse {
    content_type: &'static str,
    body: String,
    location: Option<String>,
}

impl FixtureResponse {
    pub(crate) fn atom(body: impl Into<String>) -> Self {
        Self {
            content_type: "application/atom+xml; charset=utf-8",
            body: body.into(),
            location: None,
        }
    }

    pub(crate) fn opensearch(body: impl Into<String>) -> Self {
        Self {
            content_type: "application/opensearchdescription+xml; charset=utf-8",
            body: body.into(),
            location: None,
        }
    }

    pub(crate) fn json(body: impl Into<String>) -> Self {
        Self {
            content_type: "application/json; charset=utf-8",
            body: body.into(),
            location: None,
        }
    }

    pub(crate) fn text(body: impl Into<String>) -> Self {
        Self {
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
            location: None,
        }
    }

    pub(crate) fn redirect(location: &str) -> Self {
        Self {
            content_type: "text/plain",
            body: String::new(),
            location: Some(location.to_owned()),
        }
    }
}

/// 一次被记录下来的请求：目标（含 query）与请求头（名字小写）。
#[derive(Clone)]
pub(crate) struct FixtureRequest {
    target: String,
    headers: Vec<(String, String)>,
}

impl FixtureRequest {
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    /// 只保留路径段（丢掉 query），便于按路由断言。
    pub(crate) fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("/")
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

pub(crate) struct FixtureServer {
    origin: String,
    requests: Arc<Mutex<Vec<FixtureRequest>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FixtureServer {
    /// 启动服务器。路由在**知道端口之后**才构造，因此响应体里可以引用同一个 origin
    /// （条目 id、图片地址），夹具不需要在测试代码里手工拼端口。
    pub(crate) fn start(build: impl FnOnce(&str) -> Vec<(String, FixtureResponse)>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定离线目录端口");
        listener
            .set_nonblocking(true)
            .expect("离线目录端口设为非阻塞");
        let addr = listener.local_addr().expect("读取离线目录地址");
        let origin = format!("http://{addr}");
        let routes: HashMap<String, FixtureResponse> = build(&origin).into_iter().collect();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let requests = requests.clone();
            let connections = connections.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::Release);
                            serve(stream, &routes, &requests);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => return,
                    }
                }
            })
        };
        Self {
            origin,
            requests,
            connections,
            stop,
            handle: Some(handle),
        }
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    /// 被接受的 TCP 连接数（不要求对方发出可解析的请求）。用它断言「fail closed 的
    /// 路径根本没有建立 socket」，这对 TLS 或畸形请求同样成立。
    pub(crate) fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    pub(crate) fn requests(&self) -> Vec<FixtureRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn paths(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.path().to_owned())
            .collect()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 服务一条**已接受**的连接：读完请求头、写一个定长响应、返回时关闭。
///
/// 归一化必须发生在**任何读之前**：监听套接字在 [`FixtureServer::start`] 里是
/// `set_nonblocking(true)`（accept 轮询关停标志），Windows 上 accepted 套接字会**继承**
/// 这个模式。恢复不了阻塞的连接直接放弃——夹具不做轮询重试，也不记录没有发生的请求；
/// 于是「客户端稍后才发出请求」不会退化成请求行相位立刻 `WouldBlock` 的间歇失败。
fn serve(
    mut stream: std::net::TcpStream,
    routes: &HashMap<String, FixtureResponse>,
    requests: &Mutex<Vec<FixtureRequest>>,
) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    // 读写都有超时：客户端只连不发、或不再读响应时，测试线程不会被挂住。
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let reader_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
                }
            }
        }
    }
    requests
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(FixtureRequest {
            target: target.clone(),
            headers,
        });

    let path = target.split('?').next().unwrap_or("/").to_owned();
    let fallback = FixtureResponse::text("not found");
    let (status, response) = match routes.get(&path) {
        Some(response) => (
            if response.location.is_some() {
                "302 Found"
            } else {
                "200 OK"
            },
            response,
        ),
        None => ("404 Not Found", &fallback),
    };
    let location = response
        .location
        .as_ref()
        .map(|value| format!("Location: {value}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status}\r\n{location}Content-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.content_type,
        response.body.len()
    );
    // 写出失败（客户端提前断开）不改变夹具结论：响应已带 `Connection: close`，
    // 连接在函数返回时关闭，客户端不会等待复用。
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(response.body.as_bytes());
    let _ = stream.flush();
    // 连接在函数返回时关闭。
}

// ---------- 来源登记夹具 ----------

/// 直接写 `sources` 设置行（绕过保存路径的策略校验），返回只读注册表服务。
pub(crate) async fn registry_with_sources_payload(data_json: &str) -> SourceRegistryService {
    let (registry, _) = registry_with_settings(data_json).await;
    registry
}

/// 与 [`registry_with_sources_payload`] 相同，但把底层存储句柄一起返回，供测试在
/// 暂停窗口里重写同一行（制造「配置版本在异步区间内变化」的事实）。
pub(crate) async fn registry_with_settings(
    data_json: &str,
) -> (SourceRegistryService, SourcesSettingsHandle) {
    let db = Arc::new(crate::Db::open_in_memory().expect("打开内存数据库"));
    let repos = Arc::new(crate::db::repos::SqliteRepositories::new(db));
    upsert_sources_row(&repos, data_json).await;
    (
        SourceRegistryService::new(repos.clone()),
        SourcesSettingsHandle { repos },
    )
}

/// 同一份 `sources` 设置行的写入句柄（仅测试）。
#[derive(Clone)]
pub(crate) struct SourcesSettingsHandle {
    repos: Arc<crate::db::repos::SqliteRepositories>,
}

impl SourcesSettingsHandle {
    pub(crate) async fn write(&self, data_json: &str) {
        upsert_sources_row(&self.repos, data_json).await;
    }
}

async fn upsert_sources_row(repos: &crate::db::repos::SqliteRepositories, data_json: &str) {
    use haven_domain::contracts::{SettingsRepository, SettingsRow};
    let row = SettingsRow {
        section: haven_application::services::source_registry::SOURCES_SETTINGS_SECTION.to_owned(),
        // 与 `SourcesPayload` 当前 schemaVersion 一致（夹具写的是真实负载形状）。
        schema_version: 4,
        revision: "row-1".to_owned(),
        data_json: data_json.to_owned(),
        updated_at: haven_common::UtcMillis::now(),
    };
    SettingsRepository::upsert(repos, &row)
        .await
        .expect("写入来源设置行");
}

/// 一个自定义 OPDS 书源的 `sources` 负载（`revision` 用来模拟配置版本变化）。
pub(crate) fn opds_sources_payload(
    endpoint: &str,
    source_id: &str,
    credential: bool,
    revision: &str,
) -> String {
    let credential_ref = if credential {
        format!("\"haven:opds:{source_id}\"")
    } else {
        "null".to_owned()
    };
    format!(
        r#"{{"enabledSources":["{source_id}"],"endpoints":{{}},"customSources":[{{"sourceId":"{source_id}","displayName":"自建书库","endpoint":"{endpoint}","enabled":true,"credentialRef":{credential_ref},"kind":"opds"}}],"sourceRevisions":{{"{source_id}":"{revision}"}}}}"#
    )
}

/// 一份**不再包含任何自定义来源**的 `sources` 负载（与其它夹具负载同一个 schemaVersion
/// 形状）。测试借凭据解析的暂停窗口用它覆盖同一行，制造「来源在异步区间内被删除」的事实。
pub(crate) fn sources_payload_without_custom_sources() -> String {
    r#"{"enabledSources":[],"endpoints":{},"customSources":[],"sourceRevisions":{}}"#.to_owned()
}

/// 登记一个自定义 OPDS 书源并返回注册表。
pub(crate) async fn seeded_opds_registry(
    endpoint: &str,
    credential: bool,
) -> (SourceRegistryService, String) {
    let (registry, source_id, _) = seeded_opds_registry_with_settings(endpoint, credential).await;
    (registry, source_id)
}

/// 同上，但把设置行写入句柄一并返回（测试用它模拟解析期间的配置变化）。
pub(crate) async fn seeded_opds_registry_with_settings(
    endpoint: &str,
    credential: bool,
) -> (SourceRegistryService, String, SourcesSettingsHandle) {
    let source_id = "custom_0123456789ab".to_owned();
    let data_json = opds_sources_payload(endpoint, &source_id, credential, "rev-1");
    let (registry, settings) = registry_with_settings(&data_json).await;
    (registry, source_id, settings)
}

/// 登记一个已配置端点的 CMS10 采集站（内置来源的端点存在 `endpoints` 表里）。
pub(crate) async fn seeded_cms10_registry(endpoint: &str) -> SourceRegistryService {
    let data_json = format!(
        r#"{{"enabledSources":["cms10"],"endpoints":{{"cms10":"{endpoint}"}},"customSources":[],"sourceRevisions":{{}}}}"#
    );
    registry_with_sources_payload(&data_json).await
}

/// 凭据解析器替身：记录被请求的来源 ID，并返回固定 secret。
pub(crate) fn counting_resolver(calls: Arc<Mutex<Vec<String>>>) -> Arc<super::CredentialResolver> {
    Arc::new(move |source_id: &str| {
        let calls = calls.clone();
        let source_id = source_id.to_owned();
        Box::pin(async move {
            calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(source_id);
            Some("p@ss:word".to_owned())
        })
    })
}

/// 可暂停的凭据解析器：进入后发一次信号并等待放行，然后返回 `secret`（`None` 用于覆盖
/// 「解析结果为空」那条路径）。测试借暂停窗口修改来源配置，从而证明异步区间之后的复核
/// 对 **Some 与 None 两种结果**都成立。
pub(crate) fn paused_resolver(
    secret: Option<&str>,
) -> (
    Arc<super::CredentialResolver>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let secret = secret.map(str::to_owned);
    let resolver: Arc<super::CredentialResolver> = Arc::new(move |_source_id: &str| {
        let entered = entered.clone();
        let release = release.clone();
        let secret = secret.clone();
        Box::pin(async move {
            if let Some(tx) = entered
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = tx.send(());
            }
            let pending = release
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            if let Some(pending) = pending {
                let _ = pending.await;
            }
            secret
        })
    });
    (resolver, entered_rx, release_tx)
}

/// 把 origin 拼成端点文本（保留路径，用于夹具登记）。
pub(crate) fn provider_endpoint(origin: &str) -> String {
    format!("{origin}/opds")
}

/// 一个**没有监听者**的回环端口：指向它的明文请求会在连接阶段失败，而
/// 「在发出请求之前 fail closed」的路径连连接都不会尝试。两种结果因此可以用不同的
/// 错误事实区分，测试不需要猜测有没有真的发出请求。
pub(crate) fn closed_loopback_origin() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定临时端口");
    let addr = listener.local_addr().expect("读取临时端口");
    drop(listener);
    format!("http://{addr}")
}

// ---------- 图片登记替身 ----------

/// 登记结果替身：成功 / 成功但 id 形状非法 / 被策略拒绝 / 存储失败。
#[derive(Clone, Copy)]
pub(crate) enum ArtworkOutcome {
    Registered,
    RegisteredInvalidId,
    Rejected,
    Failed,
}

/// 记录登记调用的 ImageProxyRepository 替身。**不发任何网络请求**，因此
/// 「搜索阶段只登记、不抓图」可以被直接断言。
pub(crate) struct RecordingImageProxy {
    outcome: ArtworkOutcome,
    calls: Mutex<Vec<(String, String)>>,
}

impl RecordingImageProxy {
    pub(crate) fn new(outcome: ArtworkOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            calls: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn calls(&self) -> Vec<(String, String)> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[async_trait]
impl ImageProxyRepository for RecordingImageProxy {
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((source_id.to_owned(), target_url.to_owned()));
        match self.outcome {
            ArtworkOutcome::Registered => Ok("artwork-1".to_owned()),
            ArtworkOutcome::RegisteredInvalidId => Ok("../escape".to_owned()),
            ArtworkOutcome::Rejected => Err(haven_common::validation("图片地址被策略拒绝")),
            ArtworkOutcome::Failed => Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "登记受控图片失败",
                true,
            )),
        }
    }

    async fn resolve(&self, _id: &str) -> Result<Option<String>, AppError> {
        Ok(None)
    }
}

/// 投影用例使用的单条 OPDS 条目。
pub(crate) fn opds_entry_with_poster(entry_id: &str, pic: Option<&str>) -> super::OpdsEntry {
    super::OpdsEntry {
        entry_id: entry_id.to_owned(),
        title: "有封面的书".to_owned(),
        author: None,
        summary: None,
        pic: pic.map(str::to_owned),
        epub_href: None,
    }
}

// ---------- 夹具自身的不变式 ----------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{FixtureRequest, FixtureResponse, serve};

    /// 客户端沉默期里的读超时（Windows 报 `TimedOut`，Unix 报 `WouldBlock`）：这条连接仍在
    /// 等待输入。其它读错误不算「等待」，不能当成通过。
    fn is_read_timeout(error: &std::io::Error) -> bool {
        matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    }

    /// 接受来的连接必须在**第一次读之前**恢复阻塞模式。
    ///
    /// 病根在夹具自身：`FixtureServer::start` 把**监听**套接字设为非阻塞（accept 要轮询
    /// 关停标志），Windows 上 accepted 套接字会继承该模式；于是客户端还没发字节时，请求行
    /// 相位的 `read_line` 立刻返回 `WouldBlock`（os_code 10035），5 秒读超时根本轮不到，
    /// 「客户端稍后才发出请求」就变成间歇失败。
    ///
    /// 用例把一条**处于非阻塞模式的已接受连接**交给夹具自己的 `serve`，此刻客户端**一个
    /// 字节都没发**。正确行为是连接保持等待，客户端随后补发请求即换来一次完整的有界
    /// HTTP 交换；旧行为是 `serve` 立刻返回、连接在客户端发数据之前就关掉。
    ///
    /// 顺序与调度不确定性（显式说明）：用事件（通道）而不是任意 sleep 做起点对齐——服务器
    /// 线程进入 `serve` 前发一次就绪信号。客户端随后那段「沉默期」是**有界**确认：250ms
    /// 读超时内既不能读到 EOF（旧症状）也不能读到字节。它的区分力来自「非阻塞读在微秒级
    /// 返回」，上界比它高三个数量级；若线程在信号与真正进入读之间被调度器挂起超过这个
    /// 上界，这次确认会失去区分力（这是有界等待的固有代价，不能用猜测替代）。
    #[test]
    fn accepted_socket_is_normalized_to_blocking_io_before_the_first_read() {
        // 复现 FixtureServer::start 的监听配置：只有监听套接字是非阻塞的。
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回归端口");
        listener
            .set_nonblocking(true)
            .expect("监听端口必须设为非阻塞模式");
        let addr = listener.local_addr().expect("读取回归端口");
        let mut client = TcpStream::connect(addr).expect("连接回归端口");
        // 连接此时已在 backlog 里，非阻塞 accept 立刻拿得到；客户端尚未发出任何字节。
        let (accepted, _) = listener.accept().expect("接受回归连接");

        let mut routes = HashMap::new();
        routes.insert(
            "/opds".to_owned(),
            FixtureResponse::text("fixture catalog body"),
        );
        let requests = Arc::new(Mutex::new(Vec::<FixtureRequest>::new()));
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let worker = {
            let requests = requests.clone();
            std::thread::spawn(move || {
                let _ = entered_tx.send(());
                serve(accepted, &routes, &requests);
            })
        };
        entered_rx.recv().expect("服务器线程必须启动");

        // 沉默期：夹具不得返回，更不得在客户端发出请求之前关闭连接。
        client
            .set_read_timeout(Some(Duration::from_millis(250)))
            .expect("设置客户端读超时");
        let mut probe = [0u8; 1];
        match client.read(&mut probe) {
            Ok(0) => panic!("夹具在客户端发出请求之前就关闭了连接：接受后没有恢复阻塞读"),
            Ok(n) => panic!("夹具在客户端发出请求之前就写了 {n} 字节"),
            // 读超时：连接仍在等待客户端输入，正是有界阻塞读的预期。
            Err(error) if is_read_timeout(&error) => {}
            Err(error) => panic!("沉默期读失败: {error:?}"),
        }

        // 迟到的客户端输入照常换来一次完整、有界的 HTTP 响应。
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("恢复客户端读超时");
        client
            .write_all(b"GET /opds HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
            .expect("发送回归请求");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("有界时间内必须收到完整响应");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Connection: close\r\n"), "{response}");
        assert!(response.ends_with("fixture catalog body"), "{response}");
        worker.join().expect("服务器线程必须结束");

        let recorded = requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(recorded.len(), 1, "只应记录一次请求");
        assert_eq!(recorded[0].path(), "/opds");
    }
}
