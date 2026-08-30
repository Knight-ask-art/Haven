//! Locator 映射：domain envelope（version+kind+data）→ Wire LocatorDto（契约 §22）。
//!
//! 规则：
//! - wire 只携带媒介语义字段；`media_item_id` 由上层投影（ProgressSummaryDto.mediaItemId）承载。
//! - domain `Generic` 无 wire 对应物 → 明确错误（LOCATOR_MAP_UNSUPPORTED），不允许静默降级。

use haven_common::AppError;
use haven_domain::locator::Locator;

use crate::wire::{
    ArticleLocatorDto, BookLocatorDto, ComicLocatorDto, LocatorDto, PdfLocatorDto, TextAnchorDto,
    VideoLocatorDto,
};

pub fn locator_to_dto(locator: &Locator) -> Result<LocatorDto, AppError> {
    let dto = match locator {
        Locator::Video(v) => LocatorDto::Video(VideoLocatorDto {
            position_ms: v.position_ms,
        }),
        Locator::Book(b) => LocatorDto::Book(BookLocatorDto {
            publication_resource: b.publication_resource.clone(),
            progression: b.progression.map(|v| v as f64),
            text_anchor: b.text_anchor.as_ref().map(text_anchor_to_dto),
            format_locator: b.format_locator.clone(),
        }),
        Locator::Pdf(p) => LocatorDto::Pdf(PdfLocatorDto {
            page_index: p.page_index,
            x: p.x.map(|v| v as f64),
            y: p.y.map(|v| v as f64),
            zoom: p.zoom.map(|v| v as f64),
            text_anchor: p.text_anchor.as_ref().map(text_anchor_to_dto),
        }),
        Locator::Comic(c) => LocatorDto::Comic(ComicLocatorDto {
            chapter_item_id: c.chapter_item_id.to_string(),
            page_index: c.page_index,
            page_progression: c.page_progression.map(|v| v as f64),
        }),
        Locator::Article(a) => LocatorDto::Article(ArticleLocatorDto {
            block_id: a.block_id.clone(),
            progression: a.progression.map(|v| v as f64),
            text_anchor: a.text_anchor.as_ref().map(text_anchor_to_dto),
        }),
        Locator::Generic(_) => {
            return Err(AppError::new(
                "LOCATOR_MAP_UNSUPPORTED",
                haven_common::ErrorKind::Unsupported,
                "Generic Locator 没有 wire 对应形态",
                false,
            ));
        }
    };
    Ok(dto)
}

fn text_anchor_to_dto(anchor: &haven_domain::locator::TextAnchor) -> TextAnchorDto {
    TextAnchorDto {
        exact: anchor.exact.clone(),
        prefix: anchor.prefix.clone(),
        suffix: anchor.suffix.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::MediaItemId;
    use haven_domain::locator::{ComicLocator, VideoLocator};

    #[test]
    fn video_locator_maps_without_media_item_id() {
        let loc = Locator::Video(VideoLocator {
            media_item_id: MediaItemId::new(),
            position_ms: 90_000,
        });
        let dto = locator_to_dto(&loc).unwrap();
        match &dto {
            LocatorDto::Video(v) => assert_eq!(v.position_ms, 90_000),
            _ => panic!("应为 video"),
        }
        let json = serde_json::to_string(&dto).unwrap();
        assert!(
            !json.contains("media_item_id"),
            "wire 不应携带 media_item_id: {json}"
        );
        assert!(json.contains("\"version\":1"), "{json}");
    }

    #[test]
    fn comic_locator_preserves_page_progression() {
        let loc = Locator::Comic(ComicLocator {
            chapter_item_id: MediaItemId::new(),
            page_index: 3,
            page_progression: Some(0.5),
        });
        let dto = locator_to_dto(&loc).unwrap();
        match dto {
            LocatorDto::Comic(c) => {
                assert_eq!(c.page_index, 3);
                assert_eq!(c.page_progression, Some(0.5));
            }
            _ => panic!("应为 comic"),
        }
    }

    #[test]
    fn generic_locator_errors() {
        let loc = Locator::Generic(haven_domain::locator::GenericLocator {
            key: "k".into(),
            value: "v".into(),
        });
        let err = locator_to_dto(&loc).unwrap_err();
        assert_eq!(err.code().as_str(), "LOCATOR_MAP_UNSUPPORTED");
    }
}
