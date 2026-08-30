//! 内部稳定 ID（UUID v7 Newtype）。
//!
//! 规范：`plan/DOMAIN_MODEL.md` §16。
//! 原则：
//! - 禁止大量函数接受裸 `Uuid`，ID 类型必须区分。
//! - Haven 内部 ID ≠ 外部 ID（TMDB/ISBN 等走 `ExternalId`）。

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

/// 生成一个新的 UUID v7（时间排序，利于数据库索引局部性）。
pub(crate) fn new_v7() -> Uuid {
    Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
}

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(new_v7())
            }

            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s).map(Self)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.serialize(serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Uuid::deserialize(deserializer).map(Self)
            }
        }
    };
}

id_type!(WorkId, "作品 ID");
id_type!(EditionId, "版本 ID");
id_type!(MediaItemId, "媒体条目 ID");
id_type!(ResourceId, "资源 ID");
id_type!(SourceId, "来源 ID");
id_type!(StorageLocationId, "存储位置 ID");
id_type!(MarkerId, "标记 ID");
id_type!(DownloadTaskId, "下载任务 ID");
id_type!(DownloadBatchId, "下载批次 ID");
id_type!(HistoryEntryId, "历史条目 ID");
id_type!(FeedId, "RSS 订阅 ID");
id_type!(PersonId, "人物 ID");
id_type!(CollectionId, "收藏集 ID");
id_type!(MetadataRecordId, "元数据记录 ID");
id_type!(ProgressId, "进度 ID");

/// 指向系统凭据存储的引用（不存明文凭据本身）。
///
/// 安全不变量（ADR-001 / S-01 修复）：内部字符串私有；唯一构造路径是
/// `new_scoped`（凭据模块）与 `FromStr`/`Deserialize`（均做完整格式校验：
/// `haven:<provider>:<profile-id>`），杜绝任意 Windows Credential target。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef(String);

impl CredentialRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 内部构造（仅凭据模块经校验后使用；外部必须走 FromStr / new_scoped）。
    pub(crate) fn from_inner(inner: String) -> Self {
        Self(inner)
    }
}

impl std::str::FromStr for CredentialRef {
    type Err = haven_common::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        crate::credential::parse_scoped(s)
    }
}

impl serde::Serialize for CredentialRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for CredentialRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        crate::credential::parse_scoped(&s)
            .map_err(|e| serde::de::Error::custom(e.user_message().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_newtypes() {
        let a = WorkId::new();
        let b = WorkId::new();
        assert_ne!(a, b);
        assert_ne!(a.to_string(), b.to_string());
    }

    #[test]
    fn id_roundtrip_through_json() {
        let id = EditionId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: EditionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn id_parses_from_string() {
        let id = MediaItemId::new();
        let parsed: MediaItemId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }
}
