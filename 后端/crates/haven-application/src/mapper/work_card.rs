//! Work → WorkCardDto 映射（契约 §12 投影规则）。
//!
//! 规则：
//! - 输入为领域切片（由 Application Service 组装）；本模块保持纯函数。
//! - `categories`/`availableMediaTypes` 由 Edition/MediaItem 推导去重。
//! - `primaryAction` 由后端基于进度与可用版本决定（前端不自行遍历 Graph）。
//! - `posterUri` 等只输出应用签发的 opaque artwork URI，禁止本地路径或远程 URL
//!   直接进入高权限 WebView。

use haven_common::AppError;
use haven_domain::entities::{ArtworkRef, Edition, MediaItem, Progress, Work};
use haven_domain::enums::{CompletionState, MediaType};

use crate::mapper::progress::progress_summary;
use crate::wire::{
    ContentCategory, LabelHint, MediaTypeDto, PrimaryActionDto, PrimaryActionKind, WorkCardDto,
};

/// WorkCardDto 组装输入（领域切片，由 Service 聚合后传入）。
pub struct WorkCardInput<'a> {
    pub work: &'a Work,
    pub editions: &'a [Edition],
    pub media_items: &'a [MediaItem],
    pub progress: Option<&'a Progress>,
    pub favorite: bool,
}

pub fn work_card(input: &WorkCardInput<'_>) -> Result<WorkCardDto, AppError> {
    let categories = derive_categories(input);
    let media_types = derive_media_types(input);

    Ok(WorkCardDto {
        work_id: input.work.id.to_string(),
        title: input.work.canonical_title.clone(),
        original_title: input.work.original_title.clone(),
        description: input.work.description.clone(),
        categories,
        available_media_types: media_types,
        poster_uri: controlled_artwork_uri(input.work.artwork.poster.as_ref()),
        backdrop_uri: controlled_artwork_uri(input.work.artwork.backdrop.as_ref()),
        release_year: input.work.release_year,
        rating_value: input.work.rating_value,
        rating_scale: input.work.rating_scale,
        favorite: input.favorite,
        progress: input.progress.map(progress_summary).transpose()?,
        primary_action: primary_action(input)?,
        // v0.2（契约 §36.1）：Enrichment 匹配写入前的诚实空集；V2-B 流水线填充。
        external_ids: Vec::new(),
    })
}

/// 只允许应用受控的 opaque artwork 引用进入高权限 WebView。
/// 本地路径、`file://` 与远程 URL 必须先经过缓存/注册流程转换为该形式。
fn controlled_artwork_uri(reference: Option<&ArtworkRef>) -> Option<String> {
    let uri = reference?.uri.trim();
    let opaque_id = uri.strip_prefix("haven://artwork/")?;
    if opaque_id.is_empty()
        || !opaque_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(uri.to_owned())
}

/// 主操作：必须取"同一个 Edition + 其下 MediaItem"组合，禁止跨版本错配。
/// 规则（审查修复）：遍历 editions，找第一个其下存在 media_item 的版本；
/// 该版本的 kind/editionId 与其第一个 media_item 的 mediaItemId 配对。
/// 完整 Resource 可用性判定在 Application Service 层注入。
pub fn primary_action(input: &WorkCardInput<'_>) -> Result<Option<PrimaryActionDto>, AppError> {
    let progress = input.progress;

    // 找到第一个"其下存在 media_item"的 edition；media_item 必须属于该 edition。
    let mut matched: Option<(&Edition, &MediaItem)> = None;
    for edition in input.editions {
        if let Some(item) = input
            .media_items
            .iter()
            .find(|m| m.edition_id == edition.id)
        {
            matched = Some((edition, item));
            break;
        }
    }

    let (edition, media_item) = match matched {
        Some(pair) => pair,
        None => return Ok(None), // 无任何可消费单元 → 无主操作
    };

    let kind = match edition.edition_type {
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
            PrimaryActionKind::Playback
        }
        MediaType::Book => PrimaryActionKind::Reader,
        MediaType::Comic => PrimaryActionKind::Comic,
        MediaType::Article => PrimaryActionKind::Article,
        MediaType::Document => PrimaryActionKind::Reader,
        MediaType::Unknown => PrimaryActionKind::OpenEdition,
    };

    let label_hint = match progress {
        Some(p) if !matches!(p.completion, CompletionState::NotStarted) => LabelHint::Continue,
        _ => LabelHint::Start,
    };

    Ok(Some(PrimaryActionDto {
        kind,
        label_hint,
        edition_id: edition.id.to_string(),
        media_item_id: Some(media_item.id.to_string()),
        locator: None,
    }))
}

fn derive_categories(input: &WorkCardInput<'_>) -> Vec<ContentCategory> {
    let mut seen = Vec::new();
    for e in input.editions {
        push_unique_option(
            &mut seen,
            wire_category(haven_domain::enums::ContentCategory::from_media_type(
                e.edition_type,
            )),
        );
    }
    for m in input.media_items {
        push_unique_option(
            &mut seen,
            wire_category(haven_domain::enums::ContentCategory::from_media_type(
                m.media_type,
            )),
        );
    }
    seen
}

/// domain ContentCategory（含 All sentinel）→ Option<wire ContentCategory>。
/// `All` 是查询 sentinel，实体推导不应产生（Unknown media 推导为 All）：
/// 必须忽略而非映射成错误分类（审查修复：原实现误映射为 Periodical）。
fn wire_category(domain: haven_domain::enums::ContentCategory) -> Option<ContentCategory> {
    match domain {
        haven_domain::enums::ContentCategory::Video => Some(ContentCategory::Video),
        haven_domain::enums::ContentCategory::Book => Some(ContentCategory::Book),
        haven_domain::enums::ContentCategory::Comic => Some(ContentCategory::Comic),
        haven_domain::enums::ContentCategory::Periodical => Some(ContentCategory::Periodical),
        haven_domain::enums::ContentCategory::All => None,
    }
}

fn derive_media_types(input: &WorkCardInput<'_>) -> Vec<MediaTypeDto> {
    let mut seen = Vec::new();
    for e in input.editions {
        push_unique(&mut seen, media_type_dto(e.edition_type));
    }
    for m in input.media_items {
        push_unique(&mut seen, media_type_dto(m.media_type));
    }
    seen
}

fn media_type_dto(media_type: MediaType) -> MediaTypeDto {
    match media_type {
        MediaType::Movie => MediaTypeDto::Movie,
        MediaType::Series => MediaTypeDto::Series,
        MediaType::Episode => MediaTypeDto::Episode,
        MediaType::Book => MediaTypeDto::Book,
        MediaType::Document => MediaTypeDto::Document,
        MediaType::Comic => MediaTypeDto::Comic,
        MediaType::Article => MediaTypeDto::Article,
        MediaType::Audio => MediaTypeDto::Audio,
        MediaType::Unknown => MediaTypeDto::Unknown,
    }
}

fn push_unique<T: PartialEq>(seen: &mut Vec<T>, value: T) {
    if !seen.contains(&value) {
        seen.push(value);
    }
}

fn push_unique_option<T: PartialEq>(seen: &mut Vec<T>, value: Option<T>) {
    if let Some(value) = value {
        push_unique(seen, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::entities::{
        ArtworkKind, ArtworkRef, ArtworkSet, Edition, MediaIndex, MediaItem,
    };
    use haven_domain::enums::{MediaItemStatus, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId, WorkId};

    fn sample_work() -> Work {
        Work {
            id: WorkId::new(),
            canonical_title: "三体".into(),
            original_title: None,
            sort_title: None,
            description: Some("描述".into()),
            work_type: WorkType::Fiction,
            release_year: Some(2008),
            language: Some("zh".into()),
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: ArtworkSet {
                poster: Some(ArtworkRef {
                    kind: ArtworkKind::Poster,
                    uri: "haven://artwork/p1".into(),
                    provider: None,
                }),
                ..Default::default()
            },
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    fn sample_edition(work_id: WorkId, media_type: haven_domain::enums::MediaType) -> Edition {
        Edition {
            id: EditionId::new(),
            work_id,
            title: "版本".into(),
            subtitle: None,
            edition_type: media_type,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    fn sample_item(edition_id: EditionId, media_type: haven_domain::enums::MediaType) -> MediaItem {
        MediaItem {
            id: MediaItemId::new(),
            edition_id,
            parent_id: None,
            media_type,
            title: "项".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    #[test]
    fn card_derives_categories_and_media_types() {
        let work = sample_work();
        let book_edition = sample_edition(work.id, haven_domain::enums::MediaType::Book);
        let series_edition = sample_edition(work.id, haven_domain::enums::MediaType::Series);
        let series_item = sample_item(series_edition.id, haven_domain::enums::MediaType::Episode);

        let dto = work_card(&WorkCardInput {
            work: &work,
            editions: &[book_edition, series_edition],
            media_items: &[series_item],
            progress: None,
            favorite: true,
        })
        .unwrap();

        assert_eq!(dto.work_id, work.id.to_string());
        assert!(dto.favorite);
        assert_eq!(dto.poster_uri.as_deref(), Some("haven://artwork/p1"));
        assert!(dto.categories.contains(&crate::wire::ContentCategory::Book));
        assert!(
            dto.categories
                .contains(&crate::wire::ContentCategory::Video)
        );
        assert!(dto.available_media_types.contains(&MediaTypeDto::Book));
        assert!(dto.available_media_types.contains(&MediaTypeDto::Series));
        assert!(dto.available_media_types.contains(&MediaTypeDto::Episode));

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"workId\""), "{json}");
        assert!(json.contains("\"availableMediaTypes\""), "{json}");
    }

    #[test]
    fn card_drops_uncontrolled_artwork_uris() {
        let mut work = sample_work();
        work.artwork.poster.as_mut().unwrap().uri = r"C:\Users\reader\poster.jpg".into();
        work.artwork.backdrop = Some(ArtworkRef {
            kind: ArtworkKind::Backdrop,
            uri: "https://tracker.invalid/backdrop.jpg".into(),
            provider: Some("untrusted".into()),
        });

        let dto = work_card(&WorkCardInput {
            work: &work,
            editions: &[],
            media_items: &[],
            progress: None,
            favorite: false,
        })
        .unwrap();

        assert!(dto.poster_uri.is_none());
        assert!(dto.backdrop_uri.is_none());
    }

    #[test]
    fn primary_action_follows_media_type_and_progress() {
        let work = sample_work();
        let book_edition = sample_edition(work.id, haven_domain::enums::MediaType::Book);
        let item = sample_item(book_edition.id, haven_domain::enums::MediaType::Book);

        let input = WorkCardInput {
            work: &work,
            editions: &[book_edition],
            media_items: &[item],
            progress: None,
            favorite: false,
        };
        let action = primary_action(&input).unwrap().unwrap();
        assert_eq!(action.kind, PrimaryActionKind::Reader);
        assert_eq!(action.label_hint, LabelHint::Start);
    }

    #[test]
    fn empty_editions_yield_no_primary_action() {
        let work = sample_work();
        let dto = work_card(&WorkCardInput {
            work: &work,
            editions: &[],
            media_items: &[],
            progress: None,
            favorite: false,
        })
        .unwrap();
        assert!(dto.primary_action.is_none());
        assert!(dto.categories.is_empty());
    }
}
