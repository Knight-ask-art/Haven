//! Reader TOC boundary（契约 §19.1 `reader_toc_get`）。
//!
//! Infrastructure 只负责从 EPUB 抽取原始目录事实（spine + raw nodes），
//! 本服务负责会话校验、归一化、稳定 ID 与 progression 解析：
//! - 仅保留能解析到 spine 的条目（锚点仍归属其所在文档）；
//! - ID 为 `FNV-1a64(href \0 fragment \0 title \0 occurrence)` 的 16 位 hex，
//!   其中 occurrence 按文档顺序统计同 (href, fragment, title) 出现次数，跨会话稳定；
//! - depth 归一为 `min(原始 depth, prev + 1)`，压平异常跳级；
//! - progression = spine 文档序号 / spine 长度（章节起点估算，0..1）。

use std::collections::HashMap;
use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::enums::MediaType;

use super::session::PreparedSession;
use crate::wire::{SessionEngineDto, TocItemDto};

/// 单次返回的目录条目上限（与 UI 可消费规模对齐，防御性截断）。
pub const MAX_TOC_ITEMS: usize = 8192;
/// 目录条目标题最大字符数（超出截断，避免恶意长标题）。
pub const MAX_TOC_TITLE_CHARS: usize = 512;

/// Server-only 原始目录节点（Infrastructure 产出，禁止穿过 IPC）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTocNode {
    /// 已解码、已校验的归档相对路径（不含 fragment），与 spine 条目同基准。
    pub href: String,
    /// 已解码、已校验的文档内锚点；章节起点为 None。
    pub fragment: Option<String>,
    /// 原始标题文本（未裁剪；是否合法由本服务判定）。
    pub title: String,
    /// 原始层级（nav 嵌套深度 / ncx navPoint 深度；可能异常跳级）。
    pub depth: u32,
}

/// Server-only 原始目录事实。spine 为按阅读顺序解析后的文档路径。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawEpubToc {
    pub spine: Vec<String>,
    pub nodes: Vec<RawTocNode>,
}

/// Runtime IO port，由 Infrastructure 实现。只接收 server-only 会话事实。
pub trait ReaderTocProvider: Send + Sync {
    fn extract(&self, session: &PreparedSession) -> Result<RawEpubToc, AppError>;
}

#[derive(Clone)]
pub struct ReaderTocService {
    provider: Arc<dyn ReaderTocProvider>,
}

impl ReaderTocService {
    pub fn new(provider: Arc<dyn ReaderTocProvider>) -> Self {
        Self { provider }
    }

    pub fn toc(&self, session: &PreparedSession) -> Result<Vec<TocItemDto>, AppError> {
        self.validate_session(session)?;
        let raw = self.provider.extract(session)?;
        Ok(normalize_toc(&raw))
    }

    fn validate_session(&self, session: &PreparedSession) -> Result<(), AppError> {
        if session.engine != SessionEngineDto::Reader || session.media_type != MediaType::Book {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前 Session 不是受支持的图书资源",
                false,
            ));
        }
        Ok(())
    }
}

/// 归一化并投影为 Wire DTO。纯函数，便于单元测试。
pub fn normalize_toc(raw: &RawEpubToc) -> Vec<TocItemDto> {
    let spine_index: HashMap<&str, usize> = raw
        .spine
        .iter()
        .enumerate()
        .map(|(index, href)| (href.as_str(), index))
        .collect();
    let spine_len = raw.spine.len();
    let mut items: Vec<TocItemDto> = Vec::new();
    let mut prev_depth = 0u32;
    let mut occurrences: HashMap<(String, Option<String>, String), u32> = HashMap::new();
    for node in &raw.nodes {
        if items.len() >= MAX_TOC_ITEMS {
            break;
        }
        let Some(&index) = spine_index.get(node.href.as_str()) else {
            continue;
        };
        let title = node.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TOC_TITLE_CHARS {
            continue;
        }
        let depth = node.depth.min(prev_depth.saturating_add(1));
        prev_depth = depth;
        let occurrence = occurrences
            .entry((node.href.clone(), node.fragment.clone(), title.to_owned()))
            .or_insert(0);
        let hash = fnv1a64(
            format!(
                "{}\0{}\0{}\0{}",
                node.href,
                node.fragment.as_deref().unwrap_or(""),
                title,
                occurrence
            )
            .as_bytes(),
        );
        *occurrence += 1;
        items.push(TocItemDto {
            id: format!("{hash:016x}"),
            title: title.to_owned(),
            depth,
            fragment: node.fragment.clone(),
            progression: if spine_len == 0 {
                0.0
            } else {
                index as f64 / spine_len as f64
            },
        });
    }
    items
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::{ResourceId, StorageLocationId};

    fn session(engine: SessionEngineDto, media_type: MediaType) -> PreparedSession {
        PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "0196f0d2-0000-7000-8000-000000000001".into(),
            engine,
            resource_id: ResourceId::new(),
            storage_location_id: Some(StorageLocationId::new()),
            canonical_root: Some("/root".into()),
            canonical_file: Some("/root/book.epub".into()),
            subtitle_tracks: Vec::new(),
            source: super::super::session::PreparedSessionSource::Local,
            mime_type: Some("application/epub+zip".into()),
            media_type,
            resource_type: haven_domain::enums::ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        }
    }

    struct StubProvider(RawEpubToc);

    impl ReaderTocProvider for StubProvider {
        fn extract(&self, _session: &PreparedSession) -> Result<RawEpubToc, AppError> {
            Ok(self.0.clone())
        }
    }

    fn service(raw: RawEpubToc) -> ReaderTocService {
        ReaderTocService::new(Arc::new(StubProvider(raw)))
    }

    fn raw_node(href: &str, title: &str, depth: u32) -> RawTocNode {
        RawTocNode {
            href: href.into(),
            fragment: None,
            title: title.into(),
            depth,
        }
    }

    fn raw_node_with_fragment(
        href: &str,
        fragment: Option<&str>,
        title: &str,
        depth: u32,
    ) -> RawTocNode {
        RawTocNode {
            href: href.into(),
            fragment: fragment.map(str::to_owned),
            title: title.into(),
            depth,
        }
    }

    #[test]
    fn session_validation_rejects_non_reader_engine_and_non_book() {
        let svc = service(RawEpubToc::default());
        let err = svc
            .toc(&session(SessionEngineDto::Playback, MediaType::Book))
            .unwrap_err();
        assert_eq!(err.code().as_str(), "FORMAT_UNSUPPORTED");
        let err = svc
            .toc(&session(SessionEngineDto::Reader, MediaType::Comic))
            .unwrap_err();
        assert_eq!(err.code().as_str(), "FORMAT_UNSUPPORTED");
    }

    #[test]
    fn ids_are_stable_across_calls_and_occurrence_disambiguates() {
        let raw = RawEpubToc {
            spine: vec!["a.xhtml".into(), "b.xhtml".into()],
            nodes: vec![
                raw_node("a.xhtml", "第一章", 0),
                raw_node("b.xhtml", "第一章", 0),
                raw_node("a.xhtml", "第一章", 0),
            ],
        };
        let first = service(raw.clone())
            .toc(&session(SessionEngineDto::Reader, MediaType::Book))
            .unwrap();
        let second = service(raw)
            .toc(&session(SessionEngineDto::Reader, MediaType::Book))
            .unwrap();
        assert_eq!(first, second, "同文件两次解析必须产出完全一致的目录");
        let ids: Vec<&str> = first.iter().map(|item| item.id.as_str()).collect();
        assert_ne!(ids[0], ids[1], "不同文档的相同标题必须不同 ID");
        assert_ne!(
            ids[0], ids[2],
            "同 (href,title) 的重复条目必须因 occurrence 而 ID 不同"
        );
        assert_ne!(ids[1], ids[2]);
        assert!(
            ids.iter()
                .all(|id| id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit()))
        );
    }

    #[test]
    fn depth_is_normalized_by_capping_upward_jumps() {
        let raw = RawEpubToc {
            spine: vec!["a.xhtml".into()],
            nodes: vec![
                raw_node("a.xhtml", "一级", 0),
                raw_node("a.xhtml", "跳级", 7),
                raw_node("a.xhtml", "回落", 3),
            ],
        };
        let items = normalize_toc(&raw);
        assert_eq!(
            items.iter().map(|item| item.depth).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "原始深度上跳被压平为 prev+1，合法回落按 min(prev+1) 保持"
        );
    }

    #[test]
    fn nodes_outside_spine_are_dropped_and_empty_titles_skipped() {
        let raw = RawEpubToc {
            spine: vec!["a.xhtml".into()],
            nodes: vec![
                raw_node("a.xhtml", "保留", 0),
                raw_node("missing.xhtml", "丢弃", 0),
                raw_node("a.xhtml", "   ", 0),
            ],
        };
        let items = normalize_toc(&raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "保留");
    }

    #[test]
    fn progression_maps_spine_order_and_empty_spine_yields_zero() {
        let raw = RawEpubToc {
            spine: vec!["a.xhtml".into(), "b.xhtml".into(), "c.xhtml".into()],
            nodes: vec![
                raw_node("c.xhtml", "尾", 0),
                raw_node("a.xhtml", "头", 0),
                raw_node("b.xhtml", "中", 0),
            ],
        };
        let items = normalize_toc(&raw);
        assert_eq!(
            items
                .iter()
                .map(|item| item.progression)
                .collect::<Vec<_>>(),
            vec![2.0 / 3.0, 0.0, 1.0 / 3.0]
        );
        let empty = normalize_toc(&RawEpubToc {
            spine: vec![],
            nodes: vec![raw_node("a.xhtml", "孤立", 0)],
        });
        assert!(empty.is_empty(), "spine 为空时无任何可解析条目");
    }

    #[test]
    fn fragment_is_projected_and_part_of_stable_id() {
        let raw = RawEpubToc {
            spine: vec!["a.xhtml".into()],
            nodes: vec![
                raw_node_with_fragment("a.xhtml", None, "第一章", 0),
                raw_node_with_fragment("a.xhtml", Some("part-1"), "第一章", 0),
            ],
        };
        let items = normalize_toc(&raw);
        assert_eq!(items[0].fragment, None);
        assert_eq!(items[1].fragment.as_deref(), Some("part-1"));
        assert_ne!(items[0].id, items[1].id);
    }

    #[test]
    fn item_count_is_capped_and_titles_are_trimmed() {
        let nodes = (0..MAX_TOC_ITEMS + 10)
            .map(|index| raw_node("a.xhtml", &format!("第 {index} 章"), 0))
            .collect();
        let items = normalize_toc(&RawEpubToc {
            spine: vec!["a.xhtml".into()],
            nodes,
        });
        assert_eq!(items.len(), MAX_TOC_ITEMS);
        assert_eq!(items[0].title, "第 0 章");
        let long = normalize_toc(&RawEpubToc {
            spine: vec!["a.xhtml".into()],
            nodes: vec![raw_node(
                "a.xhtml",
                &"长".repeat(MAX_TOC_TITLE_CHARS + 1),
                0,
            )],
        });
        assert!(long.is_empty(), "超长标题必须被跳过");
    }
}
