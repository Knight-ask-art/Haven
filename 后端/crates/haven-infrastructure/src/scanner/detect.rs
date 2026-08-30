//! 格式检测（Detector 顺序的第一级：Extension Hint）。
//!
//! 规范：LIBRARY_AND_STORAGE §49、§207（Extension → Magic → Probe → Classifier）。
//! 第一版只做 Extension Hint；Magic 检测（cbz=zip、pdf 头等）后续扩展。
//! 未知扩展名 → None（P0 默认忽略非支持类型，扫描日志可显示，§3233）。

use haven_domain::enums::{ContentCategory, MediaType};

/// 检测结果：文件应如何进入统一内容模型。
#[derive(Debug, Clone, PartialEq)]
pub struct DetectResult {
    pub media_type: MediaType,
    pub resource_type: haven_domain::enums::ResourceType,
    pub category: ContentCategory,
    /// The MIME sent to the controlled resource protocol.  This is derived
    /// from the allowlisted format, never from user supplied metadata.
    pub mime_type: &'static str,
    /// Maximum size accepted by the v0.1.0 local scanner for this format.
    pub max_size_bytes: u64,
    /// 由文件名推导的标题提示（供 Work/Edition title 使用，可被元数据覆盖）。
    pub title_hint: Option<String>,
}

/// 已识别资源类型的扩展名集合。
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "webm", "flv", "ts", "m2ts",
];
const EPUB_EXTS: &[&str] = &["epub"];
const TXT_EXTS: &[&str] = &["txt"];
const MD_EXTS: &[&str] = &["md", "markdown"];
const PDF_EXTS: &[&str] = &["pdf"];
const COMIC_EXTS: &[&str] = &["cbz"];
const HTML_EXTS: &[&str] = &["html", "htm"];

pub const MAX_VIDEO_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_PUBLICATION_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_COMIC_BYTES: u64 = 1024 * 1024 * 1024;
/// 图片目录的总大小上限与 Comic Resource Provider 保持一致。
pub const MAX_IMAGE_SEQUENCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 按扩展名检测文件类型。无扩展名或未知 → None。
pub fn detect_by_extension(file_name: &str) -> Option<DetectResult> {
    let (stem, ext) = split_name(file_name);
    let ext = ext.to_ascii_lowercase();
    let title_hint = parse_title_hint(stem);

    let (media_type, resource_type, mime_type, max_size_bytes) =
        if VIDEO_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Movie,
                haven_domain::enums::ResourceType::LocalFile,
                match ext.as_str() {
                    "webm" => "video/webm",
                    "mkv" => "video/x-matroska",
                    "avi" => "video/x-msvideo",
                    "mov" => "video/quicktime",
                    "wmv" => "video/x-ms-wmv",
                    "flv" => "video/x-flv",
                    "ts" | "m2ts" => "video/mp2t",
                    _ => "video/mp4",
                },
                MAX_VIDEO_BYTES,
            )
        } else if EPUB_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Book,
                haven_domain::enums::ResourceType::PublicationFile,
                "application/epub+zip",
                MAX_PUBLICATION_BYTES,
            )
        } else if TXT_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Book,
                haven_domain::enums::ResourceType::LocalFile,
                "text/plain; charset=utf-8",
                MAX_TEXT_BYTES,
            )
        } else if MD_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Document,
                haven_domain::enums::ResourceType::LocalFile,
                "text/markdown; charset=utf-8",
                MAX_TEXT_BYTES,
            )
        } else if PDF_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Document,
                haven_domain::enums::ResourceType::PublicationFile,
                "application/pdf",
                MAX_PUBLICATION_BYTES,
            )
        } else if COMIC_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Comic,
                haven_domain::enums::ResourceType::ComicArchive,
                "application/vnd.comicbook+zip",
                MAX_COMIC_BYTES,
            )
        } else if HTML_EXTS.contains(&ext.as_str()) {
            (
                MediaType::Article,
                haven_domain::enums::ResourceType::LocalFile,
                "text/html; charset=utf-8",
                MAX_TEXT_BYTES,
            )
        } else {
            return None;
        };

    Some(DetectResult {
        media_type,
        resource_type,
        category: ContentCategory::from_media_type(media_type),
        mime_type,
        max_size_bytes,
        title_hint,
    })
}

/// 为包含直接图片子项的目录构造漫画 `ImageSequence` 检测结果。
///
/// 目录本身没有可用于 MIME 判断的扩展名，因此由扫描器在确认存在受支持的
/// 直接图片子项后调用本函数。页面内容仍由受控 Comic Resource Provider 做魔数
/// 和路径复核，扫描阶段只负责登记资源类型和安全大小上限。
pub fn detect_image_sequence(directory_name: &str) -> DetectResult {
    DetectResult {
        media_type: MediaType::Comic,
        resource_type: haven_domain::enums::ResourceType::ImageSequence,
        category: ContentCategory::from_media_type(MediaType::Comic),
        mime_type: "application/x-haven-image-sequence",
        max_size_bytes: MAX_IMAGE_SEQUENCE_BYTES,
        title_hint: parse_title_hint(directory_name),
    }
}

/// 扫描器和其他本地来源共用的窄图片扩展名判断。
pub fn is_supported_image_file_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp"
        ),
        _ => false,
    }
}

/// 拆出 (stem, ext)。`name.tar.gz` 取最后一段扩展名。
fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && !stem.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

/// 标题提示：去扩展名后清洗（替换 `.`/`_`/`-` 为空格，去掉年份等噪音保留原样第一版）。
/// 例：`三体.2008.txt` → `三体.2008`；`The.Matrix.1999.mkv` → `The.Matrix.1999`。
/// 第一版只做最小清洗，语义解析（剧集 S01E01 / 年份）后续任务。
pub fn parse_title_hint(stem: &str) -> Option<String> {
    let cleaned = stem.trim().trim_end_matches(['.', '_', '-', ' ']);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_maps_to_movie_video() {
        let r = detect_by_extension("Dune.2021.mkv").unwrap();
        assert_eq!(r.media_type, MediaType::Movie);
        assert_eq!(r.category, ContentCategory::Video);
        assert_eq!(r.title_hint.as_deref(), Some("Dune.2021"));
    }

    #[test]
    fn epub_is_book_publication() {
        let r = detect_by_extension("三体.epub").unwrap();
        assert_eq!(r.media_type, MediaType::Book);
        assert_eq!(
            r.resource_type,
            haven_domain::enums::ResourceType::PublicationFile
        );
        assert_eq!(r.category, ContentCategory::Book);
        assert_eq!(r.mime_type, "application/epub+zip");
        assert_eq!(r.max_size_bytes, MAX_PUBLICATION_BYTES);
    }

    #[test]
    fn md_and_pdf_are_documents() {
        assert_eq!(
            detect_by_extension("notes.md").unwrap().media_type,
            MediaType::Document
        );
        assert_eq!(
            detect_by_extension("paper.pdf").unwrap().media_type,
            MediaType::Document
        );
        assert_eq!(
            detect_by_extension("paper.pdf").unwrap().category,
            ContentCategory::Periodical
        );
    }

    #[test]
    fn comic_maps_to_comic_archive() {
        let r = detect_by_extension("Frieren.vol1.cbz").unwrap();
        assert_eq!(r.media_type, MediaType::Comic);
        assert_eq!(
            r.resource_type,
            haven_domain::enums::ResourceType::ComicArchive
        );
        assert_eq!(r.mime_type, "application/vnd.comicbook+zip");
        assert!(detect_by_extension("Frieren.vol1.cbr").is_none());
        assert!(detect_by_extension("Frieren.vol1.cb7").is_none());
    }

    #[test]
    fn image_directory_maps_to_image_sequence() {
        let result = detect_image_sequence("进击的巨人");
        assert_eq!(result.media_type, MediaType::Comic);
        assert_eq!(
            result.resource_type,
            haven_domain::enums::ResourceType::ImageSequence
        );
        assert_eq!(result.max_size_bytes, MAX_IMAGE_SEQUENCE_BYTES);
        assert!(is_supported_image_file_name("page01.JPG"));
        assert!(is_supported_image_file_name("page02.webp"));
        assert!(!is_supported_image_file_name("notes.txt"));
    }

    #[test]
    fn html_is_article() {
        let r = detect_by_extension("article.html").unwrap();
        assert_eq!(r.media_type, MediaType::Article);
        assert_eq!(r.category, ContentCategory::Periodical);
    }

    #[test]
    fn unknown_and_hidden_ignored() {
        assert!(detect_by_extension("random.xyz").is_none());
        assert!(detect_by_extension("noext").is_none());
        assert!(detect_by_extension(".gitignore").is_none());
    }

    #[test]
    fn title_hint_trims_noise() {
        assert_eq!(
            parse_title_hint("The.Matrix.").as_deref(),
            Some("The.Matrix")
        );
        assert_eq!(parse_title_hint("  "), None);
        assert_eq!(parse_title_hint("…").as_deref(), Some("…"));
    }
}
