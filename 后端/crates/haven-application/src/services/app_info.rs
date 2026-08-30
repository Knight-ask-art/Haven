//! About / Diagnostics 应用服务（V02-SETTINGS-ABOUT-DIAGNOSTICS-008）。
//!
//! 该服务只编排受控的构建/运行时信息端口，并把内部事实映射为 Wire DTO。
//! 具体路径、数据库读取和系统目录打开由 Infrastructure 实现；前端永远不能
//! 提交任意路径。

use std::sync::Arc;

use haven_common::AppError;

use crate::wire::{AppDirectoryDto, AppDirectoryKindDto, AppInfoDto, ThirdPartyNoticeDto};

/// Application 内部使用的固定目录身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryKind {
    Data,
    Logs,
    Cache,
}

/// Infrastructure 返回的脱敏目录投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryFacts {
    pub kind: DirectoryKind,
    pub display_name: String,
    pub display_path: String,
    pub exists: bool,
    pub can_open: bool,
}

/// Infrastructure 返回的构建与运行时信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfoFacts {
    pub app_version: String,
    pub build_channel: String,
    pub source_pack_version: Option<String>,
    pub protocol_version: String,
    pub database_version: String,
    pub app_license: Option<String>,
    pub third_party_notices: Vec<ThirdPartyNoticeFacts>,
    pub directories: Vec<DirectoryFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThirdPartyNoticeFacts {
    pub name: String,
    pub license: String,
}

/// About/Diagnostics 端口。实现方必须保证路径范围固定且不接受用户路径。
pub trait AppInfoPorts: Send + Sync {
    fn get(&self) -> Result<AppInfoFacts, AppError>;
    fn open_directory(&self, kind: DirectoryKind) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct AppInfoService {
    ports: Arc<dyn AppInfoPorts>,
}

impl AppInfoService {
    pub fn new(ports: Arc<dyn AppInfoPorts>) -> Self {
        Self { ports }
    }

    pub fn get(&self) -> Result<AppInfoDto, AppError> {
        let facts = self.ports.get()?;
        Ok(AppInfoDto {
            schema_version: 1,
            app_version: facts.app_version,
            build_channel: facts.build_channel,
            source_pack_version: facts.source_pack_version,
            protocol_version: facts.protocol_version,
            database_version: facts.database_version,
            app_license: facts.app_license,
            third_party_notices: facts
                .third_party_notices
                .into_iter()
                .map(|notice| ThirdPartyNoticeDto {
                    name: notice.name,
                    license: notice.license,
                })
                .collect(),
            directories: facts
                .directories
                .into_iter()
                .map(|directory| AppDirectoryDto {
                    kind: match directory.kind {
                        DirectoryKind::Data => AppDirectoryKindDto::Data,
                        DirectoryKind::Logs => AppDirectoryKindDto::Logs,
                        DirectoryKind::Cache => AppDirectoryKindDto::Cache,
                    },
                    display_name: directory.display_name,
                    display_path: directory.display_path,
                    exists: directory.exists,
                    can_open: directory.can_open,
                })
                .collect(),
        })
    }

    pub fn open_directory(&self, kind: DirectoryKind) -> Result<(), AppError> {
        self.ports.open_directory(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakePorts {
        opened: Mutex<Vec<DirectoryKind>>,
    }

    impl AppInfoPorts for FakePorts {
        fn get(&self) -> Result<AppInfoFacts, AppError> {
            Ok(AppInfoFacts {
                app_version: "0.1.0-test".into(),
                build_channel: "test".into(),
                source_pack_version: Some("builtin-1".into()),
                protocol_version: "ipc-v1".into(),
                database_version: "024_resource_preferences".into(),
                app_license: Some("MIT".into()),
                third_party_notices: vec![ThirdPartyNoticeFacts {
                    name: "Example".into(),
                    license: "MIT".into(),
                }],
                directories: vec![DirectoryFacts {
                    kind: DirectoryKind::Data,
                    display_name: "应用数据".into(),
                    display_path: "%APPDATA%\\com.haven.reader".into(),
                    exists: true,
                    can_open: true,
                }],
            })
        }

        fn open_directory(&self, kind: DirectoryKind) -> Result<(), AppError> {
            self.opened.lock().unwrap().push(kind);
            Ok(())
        }
    }

    #[test]
    fn maps_facts_to_safe_wire_projection() {
        let service = AppInfoService::new(Arc::new(FakePorts {
            opened: Mutex::new(Vec::new()),
        }));
        let dto = service.get().unwrap();
        assert_eq!(dto.schema_version, 1);
        assert_eq!(dto.database_version, "024_resource_preferences");
        assert_eq!(
            dto.directories[0].display_path,
            "%APPDATA%\\com.haven.reader"
        );
        assert_eq!(dto.app_license.as_deref(), Some("MIT"));
    }

    #[test]
    fn opens_only_semantic_directory_kind() {
        let ports = Arc::new(FakePorts {
            opened: Mutex::new(Vec::new()),
        });
        let service = AppInfoService::new(ports.clone());
        service.open_directory(DirectoryKind::Cache).unwrap();
        assert_eq!(
            ports.opened.lock().unwrap().as_slice(),
            &[DirectoryKind::Cache]
        );
    }
}
