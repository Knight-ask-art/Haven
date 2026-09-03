//! 本地 About / Diagnostics 事实提供器。
//!
//! 该模块是唯一拥有应用目录、数据库版本和构建清单读取能力的基础设施
//! 实现。它只向 Application 返回脱敏投影；前端不能提交路径，也不能读取
//! `THIRD_PARTY_NOTICES.md` 或 SQLite。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use haven_application::services::app_info::{
    AppInfoFacts, AppInfoPorts, DirectoryFacts, DirectoryKind, ThirdPartyNoticeFacts,
};
use haven_common::{AppError, ErrorKind};

use crate::db::Db;

const PROTOCOL_VERSION: &str = "ipc-v1";
const SOURCE_PACK_VERSION: &str = "builtin-1";

/// 组合根注入的固定目录集合。构造后不接受来自 IPC 的路径输入。
#[derive(Clone)]
pub struct LocalAppInfoProvider {
    db: Arc<Db>,
    data_dir: PathBuf,
    logs_dir: PathBuf,
    cache_dir: PathBuf,
    directory_launcher: Arc<dyn DirectoryLauncher>,
}

/// 固定目录启动器端口。单测可以注入 Fake，生产实现只调用平台文件管理器，
/// 不接受前端传入的命令或路径。
pub trait DirectoryLauncher: Send + Sync {
    fn launch(&self, path: &Path) -> std::io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformDirectoryLauncher;

impl DirectoryLauncher for PlatformDirectoryLauncher {
    fn launch(&self, path: &Path) -> std::io::Result<()> {
        open_directory_with_platform(path)
    }
}

impl LocalAppInfoProvider {
    pub fn new(db: Arc<Db>, data_dir: PathBuf, logs_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            db,
            data_dir,
            logs_dir,
            cache_dir,
            directory_launcher: Arc::new(PlatformDirectoryLauncher),
        }
    }

    #[cfg(test)]
    fn with_launcher(
        db: Arc<Db>,
        data_dir: PathBuf,
        logs_dir: PathBuf,
        cache_dir: PathBuf,
        directory_launcher: Arc<dyn DirectoryLauncher>,
    ) -> Self {
        Self {
            db,
            data_dir,
            logs_dir,
            cache_dir,
            directory_launcher,
        }
    }

    fn directory_facts(&self, kind: DirectoryKind, path: &Path) -> DirectoryFacts {
        let exists = path.is_dir();
        // 数据目录由组合根在打开数据库前创建；日志/缓存目录可以在用户点击
        // “打开目录”时按固定父目录创建，因此不把尚未创建误报为不可用。
        let can_open = exists || path.parent().is_some_and(Path::is_dir);
        DirectoryFacts {
            kind,
            display_name: display_name(kind).to_owned(),
            display_path: display_path(kind),
            exists,
            can_open,
        }
    }

    fn latest_database_version(&self) -> Result<String, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT version FROM schema_migrations ORDER BY applied_at DESC, rowid DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| {
            AppError::new(
                "APP_INFO_UNAVAILABLE",
                ErrorKind::Database,
                "无法读取数据库版本",
                true,
            )
            .with_source(error)
        })
    }

    fn notices(&self) -> Vec<ThirdPartyNoticeFacts> {
        parse_third_party_notices(include_str!("../../../../THIRD_PARTY_NOTICES.md"))
    }
}

impl AppInfoPorts for LocalAppInfoProvider {
    fn get(&self) -> Result<AppInfoFacts, AppError> {
        Ok(AppInfoFacts {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            build_channel: if cfg!(debug_assertions) {
                "development".to_owned()
            } else {
                "release".to_owned()
            },
            // `builtin_catalog()` 是当前 Source Registry 的静态来源包事实源；
            // 版本号在 Application/Infrastructure 之间固定登记为 builtin-1。
            source_pack_version: Some(SOURCE_PACK_VERSION.to_owned()),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            database_version: self.latest_database_version()?,
            // Haven 自有代码的许可证由仓库根 LICENSE 固定为 MIT；第三方清单
            // 仍然只描述依赖和资源的各自许可证。
            app_license: Some("MIT".to_owned()),
            third_party_notices: self.notices(),
            directories: vec![
                self.directory_facts(DirectoryKind::Data, &self.data_dir),
                self.directory_facts(DirectoryKind::Logs, &self.logs_dir),
                self.directory_facts(DirectoryKind::Cache, &self.cache_dir),
            ],
        })
    }

    fn open_directory(&self, kind: DirectoryKind) -> Result<(), AppError> {
        let path = match kind {
            DirectoryKind::Data => &self.data_dir,
            DirectoryKind::Logs => &self.logs_dir,
            DirectoryKind::Cache => &self.cache_dir,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AppError::new(
                    "APP_DIRECTORY_NOT_FOUND",
                    ErrorKind::Storage,
                    "应用目录不可用",
                    true,
                )
                .with_source(error)
            })?;
        }
        std::fs::create_dir_all(path).map_err(|error| {
            AppError::new(
                "APP_DIRECTORY_NOT_FOUND",
                ErrorKind::Storage,
                "应用目录不可用",
                true,
            )
            .with_source(error)
        })?;

        let result = self.directory_launcher.launch(path);
        result.map_err(|error| {
            AppError::new(
                "APP_DIRECTORY_OPEN_FAILED",
                ErrorKind::Io,
                "无法打开应用目录",
                true,
            )
            .with_source(error)
        })
    }
}

fn display_name(kind: DirectoryKind) -> &'static str {
    match kind {
        DirectoryKind::Data => "应用数据目录",
        DirectoryKind::Logs => "日志目录",
        DirectoryKind::Cache => "缓存目录",
    }
}

/// 只返回稳定的逻辑路径，不暴露用户名、盘符或数据库文件名。
fn display_path(kind: DirectoryKind) -> String {
    let suffix = match kind {
        DirectoryKind::Data => "",
        DirectoryKind::Logs => "/Logs",
        DirectoryKind::Cache => "/Cache",
    };
    if cfg!(windows) {
        format!("%APPDATA%/com.haven.reader{suffix}")
    } else if cfg!(target_os = "macos") {
        format!("~/Library/Application Support/com.haven.reader{suffix}")
    } else {
        format!("$XDG_DATA_HOME/com.haven.reader{suffix}")
    }
}

fn open_directory_with_platform(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer.exe").arg(path).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

/// 从仓库清单提取最小摘要，不把完整许可正文或 URL 送进 IPC。
fn parse_third_party_notices(markdown: &str) -> Vec<ThirdPartyNoticeFacts> {
    let mut notices = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_license: Option<String> = None;

    let flush = |notices: &mut Vec<ThirdPartyNoticeFacts>,
                 name: &mut Option<String>,
                 license: &mut Option<String>| {
        if let (Some(name), Some(license)) = (name.take(), license.take()) {
            notices.push(ThirdPartyNoticeFacts { name, license });
        } else {
            *name = None;
            *license = None;
        }
    };

    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            flush(&mut notices, &mut current_name, &mut current_license);
            current_name = Some(title.trim().to_owned());
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("- Publisher-declared license:") {
            let license = value.trim().trim_matches('`').trim();
            if !license.is_empty() {
                current_license = Some(license.to_owned());
            }
        }
    }
    flush(&mut notices, &mut current_name, &mut current_license);
    notices
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLauncher {
        result: std::io::Result<()>,
        calls: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl DirectoryLauncher for FakeLauncher {
        fn launch(&self, path: &Path) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            match &self.result {
                Ok(()) => Ok(()),
                Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
            }
        }
    }

    #[test]
    fn parses_only_named_license_summaries() {
        let notices = parse_third_party_notices(
            "## Package A\n- Publisher-declared license: `MIT`\n\n## Assets\n- Other: owner\n",
        );
        assert_eq!(
            notices,
            vec![ThirdPartyNoticeFacts {
                name: "Package A".to_owned(),
                license: "MIT".to_owned(),
            }]
        );
    }

    #[test]
    fn display_paths_are_redacted() {
        for kind in [
            DirectoryKind::Data,
            DirectoryKind::Logs,
            DirectoryKind::Cache,
        ] {
            let path = display_path(kind);
            assert!(!path.contains("Users"));
            assert!(!path.contains("\\"));
            assert!(path.contains("com.haven.reader"));
        }
    }

    #[test]
    fn in_memory_provider_maps_latest_schema_without_vendored_notices() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let provider = LocalAppInfoProvider::new(
            db,
            std::env::temp_dir().join("haven-app-info-data"),
            std::env::temp_dir().join("haven-app-info-logs"),
            std::env::temp_dir().join("haven-app-info-cache"),
        );
        let facts = provider.get().unwrap();
        assert_eq!(
            facts.database_version,
            "036_comic_chapter_profile_observation"
        );
        assert_eq!(facts.source_pack_version.as_deref(), Some("builtin-1"));
        assert!(facts.third_party_notices.is_empty());
        assert_eq!(facts.app_license.as_deref(), Some("MIT"));
    }

    #[test]
    fn injected_launcher_receives_only_fixed_directory() {
        let root = tempfile::tempdir().unwrap();
        let calls = std::sync::Mutex::new(Vec::new());
        let launcher = Arc::new(FakeLauncher {
            result: Ok(()),
            calls,
        });
        let provider = LocalAppInfoProvider::with_launcher(
            Arc::new(Db::open_in_memory().unwrap()),
            root.path().join("data"),
            root.path().join("logs"),
            root.path().join("cache"),
            launcher.clone(),
        );
        provider.open_directory(DirectoryKind::Logs).unwrap();
        let calls = launcher.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &[root.path().join("logs")]);
        assert!(calls[0].is_dir());
    }

    #[test]
    fn launcher_failure_maps_to_stable_open_error() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalAppInfoProvider::with_launcher(
            Arc::new(Db::open_in_memory().unwrap()),
            root.path().join("data"),
            root.path().join("logs"),
            root.path().join("cache"),
            Arc::new(FakeLauncher {
                result: Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                )),
                calls: std::sync::Mutex::new(Vec::new()),
            }),
        );
        let error = provider.open_directory(DirectoryKind::Cache).unwrap_err();
        assert_eq!(error.code().as_str(), "APP_DIRECTORY_OPEN_FAILED");
        assert!(error.user_message().contains("打开"));
    }
}
