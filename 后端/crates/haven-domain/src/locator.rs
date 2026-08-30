//! Universal Locator —— 跨媒介进度、标记、同步的核心抽象。
//!
//! 规范：`plan/DOMAIN_MODEL.md` §31–§40。
//! 铁律：Progress 的事实来源是 Locator，不是 percentage（§30）。
//! Locator 必须版本化（§40），同步协议不得假设结构不变。

use serde::{Deserialize, Serialize};

use crate::ids::MediaItemId;

/// 当前 Locator 结构版本。结构变更时必须升级并保留旧版解析。
pub const LOCATOR_VERSION: u32 = 1;

/// 视频定位：时间位置（毫秒）。
/// 来源：DOMAIN_MODEL §33
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VideoLocator {
    pub media_item_id: MediaItemId,
    pub position_ms: u64,
}

/// 可重排电子书定位：resource + progression + anchor + format locator 组合。
/// 来源：DOMAIN_MODEL §34
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BookLocator {
    pub publication_resource: String,
    pub progression: Option<f32>,
    pub text_anchor: Option<TextAnchor>,
    pub format_locator: Option<String>,
}

/// PDF 定位。
/// 来源：DOMAIN_MODEL §35
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PdfLocator {
    pub page_index: u32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub zoom: Option<f32>,
    pub text_anchor: Option<TextAnchor>,
}

/// 漫画定位（条漫模式用 page_progression 恢复滚动位置）。
/// 来源：DOMAIN_MODEL §36
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicLocator {
    pub chapter_item_id: MediaItemId,
    pub page_index: u32,
    pub page_progression: Option<f32>,
}

/// 文章定位（依赖 Reader Model 生成的稳定 Block ID，不依赖远程 DOM XPath）。
/// 来源：DOMAIN_MODEL §37
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArticleLocator {
    pub block_id: Option<String>,
    pub progression: Option<f32>,
    pub text_anchor: Option<TextAnchor>,
}

/// 未知格式兼容层。核心引擎不得滥用（§38）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GenericLocator {
    pub key: String,
    pub value: String,
}

/// 文本锚点：文本轻微变化时可通过上下文重新定位（§39）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TextAnchor {
    pub exact: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// 统一 Locator。版本化序列化：`{"version":1,"kind":"...","data":{...}}`。
/// 来源：DOMAIN_MODEL §32
#[derive(Debug, Clone, PartialEq)]
pub enum Locator {
    Video(VideoLocator),
    Book(BookLocator),
    Pdf(PdfLocator),
    Comic(ComicLocator),
    Article(ArticleLocator),
    Generic(GenericLocator),
}

#[derive(Serialize, Deserialize)]
struct LocatorEnvelope {
    version: u32,
    #[serde(flatten)]
    payload: LocatorPayload,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
enum LocatorPayload {
    Video(VideoLocator),
    Book(BookLocator),
    Pdf(PdfLocator),
    Comic(ComicLocator),
    Article(ArticleLocator),
    Generic(GenericLocator),
}

impl Serialize for Locator {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        let payload = match self {
            Self::Video(value) => LocatorPayload::Video(value.clone()),
            Self::Book(value) => LocatorPayload::Book(value.clone()),
            Self::Pdf(value) => LocatorPayload::Pdf(value.clone()),
            Self::Comic(value) => LocatorPayload::Comic(value.clone()),
            Self::Article(value) => LocatorPayload::Article(value.clone()),
            Self::Generic(value) => LocatorPayload::Generic(value.clone()),
        };

        LocatorEnvelope {
            version: LOCATOR_VERSION,
            payload,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Locator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let envelope = LocatorEnvelope::deserialize(deserializer)?;
        if envelope.version != LOCATOR_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported locator version: {}",
                envelope.version
            )));
        }

        let locator = match envelope.payload {
            LocatorPayload::Video(value) => Self::Video(value),
            LocatorPayload::Book(value) => Self::Book(value),
            LocatorPayload::Pdf(value) => Self::Pdf(value),
            LocatorPayload::Comic(value) => Self::Comic(value),
            LocatorPayload::Article(value) => Self::Article(value),
            LocatorPayload::Generic(value) => Self::Generic(value),
        };
        locator.validate().map_err(serde::de::Error::custom)?;
        Ok(locator)
    }
}

impl Locator {
    pub fn version(&self) -> u32 {
        LOCATOR_VERSION
    }

    /// 数值范围校验（progression/page_progression 必须有限且在 0..=1 内）。
    /// 由反序列化与写入路径调用；非法值不得覆盖旧的有效 Progress。
    pub fn validate(&self) -> Result<(), &'static str> {
        let progression = match self {
            Self::Book(value) => value.progression,
            Self::Comic(value) => value.page_progression,
            Self::Article(value) => value.progression,
            _ => None,
        };
        if progression.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return Err("locator progression must be finite and within 0..=1");
        }
        Ok(())
    }
}

/// Locator kind 与媒介类型兼容性规则（契约 §22.6：后端必须校验）。
///
/// ```text
/// Movie/Series/Episode/Audio → Video
/// Book                        → Book
/// Document                    → Pdf
/// Comic                       → Comic
/// Article                     → Article
/// ```
pub fn locator_kind_compatible(media_type: crate::enums::MediaType, locator: &Locator) -> bool {
    let expected_video = matches!(
        media_type,
        crate::enums::MediaType::Movie
            | crate::enums::MediaType::Series
            | crate::enums::MediaType::Episode
            | crate::enums::MediaType::Audio
    );
    match locator {
        Locator::Video(_) => expected_video,
        Locator::Book(_) => media_type == crate::enums::MediaType::Book,
        Locator::Pdf(_) => media_type == crate::enums::MediaType::Document,
        Locator::Comic(_) => media_type == crate::enums::MediaType::Comic,
        Locator::Article(_) => media_type == crate::enums::MediaType::Article,
        Locator::Generic(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MediaItemId;

    #[test]
    fn locator_json_roundtrip() {
        let loc = Locator::Video(VideoLocator {
            media_item_id: MediaItemId::new(),
            position_ms: 5_784_000,
        });
        let json = serde_json::to_string(&loc).unwrap();
        let back: Locator = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, back);
        assert_eq!(loc.version(), 1);
    }

    #[test]
    fn locator_uses_stable_tags() {
        let loc = Locator::Book(BookLocator {
            publication_resource: "chapter-03.xhtml".into(),
            progression: Some(0.42),
            text_anchor: None,
            format_locator: None,
        });
        let json = serde_json::to_string(&loc).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["kind"], "book");
        assert_eq!(value["data"]["publication_resource"], "chapter-03.xhtml");
    }

    #[test]
    fn locator_rejects_unknown_versions() {
        let json = r#"{"version":2,"kind":"article","data":{"block_id":null,"progression":null,"text_anchor":null}}"#;
        let error = serde_json::from_str::<Locator>(json).unwrap_err();
        assert!(error.to_string().contains("unsupported locator version: 2"));
    }

    #[test]
    fn locator_kind_compatibility_matrix() {
        use crate::enums::MediaType;
        let video = Locator::Video(VideoLocator {
            media_item_id: MediaItemId::new(),
            position_ms: 0,
        });
        let book = Locator::Book(BookLocator {
            publication_resource: "c.xhtml".into(),
            progression: None,
            text_anchor: None,
            format_locator: None,
        });
        let pdf = Locator::Pdf(PdfLocator {
            page_index: 0,
            x: None,
            y: None,
            zoom: None,
            text_anchor: None,
        });
        let comic = Locator::Comic(ComicLocator {
            chapter_item_id: MediaItemId::new(),
            page_index: 0,
            page_progression: None,
        });
        let article = Locator::Article(ArticleLocator {
            block_id: None,
            progression: None,
            text_anchor: None,
        });
        let generic = Locator::Generic(GenericLocator {
            key: "k".into(),
            value: "v".into(),
        });

        assert!(locator_kind_compatible(MediaType::Movie, &video));
        assert!(locator_kind_compatible(MediaType::Episode, &video));
        assert!(!locator_kind_compatible(MediaType::Movie, &book));
        assert!(locator_kind_compatible(MediaType::Book, &book));
        assert!(!locator_kind_compatible(MediaType::Book, &pdf));
        assert!(locator_kind_compatible(MediaType::Document, &pdf));
        assert!(locator_kind_compatible(MediaType::Comic, &comic));
        assert!(locator_kind_compatible(MediaType::Article, &article));
        assert!(!locator_kind_compatible(MediaType::Movie, &generic));
        assert!(!locator_kind_compatible(MediaType::Unknown, &video));
    }

    #[test]
    fn locator_rejects_out_of_range_progression() {
        let json = r#"{"version":1,"kind":"book","data":{"publication_resource":"chapter.xhtml","progression":1.2,"text_anchor":null,"format_locator":null}}"#;
        let error = serde_json::from_str::<Locator>(json).unwrap_err();
        assert!(error.to_string().contains("progression"));
    }

    #[test]
    fn locator_refuses_to_serialize_non_finite_progression() {
        let locator = Locator::Article(ArticleLocator {
            block_id: None,
            progression: Some(f32::NAN),
            text_anchor: None,
        });
        let error = serde_json::to_string(&locator).unwrap_err();
        assert!(error.to_string().contains("progression"));
    }
}
