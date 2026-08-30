//! Bounded, on-demand local comic page provider for CBZ and image directories.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use haven_application::services::{
    ComicImageMime, ComicPageBody, ComicPageProvider, PreparedComicPage,
    PreparedComicPageAvailability, PreparedComicPageSource, PreparedSession,
};
use haven_common::{AppError, ErrorKind};
use haven_domain::enums::ResourceType;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zip::ZipArchive;

pub const MAX_COMIC_PAGES: usize = 5_000;
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;
pub const MAX_COMIC_PAGE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_COMIC_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 100;
pub const MAX_ENTRY_NAME_BYTES: usize = 1_024;
pub const MAX_COMIC_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
pub struct LocalComicPageProvider;

impl LocalComicPageProvider {
    pub fn new() -> Self {
        Self
    }

    fn inspect_archive(
        &self,
        session: &PreparedSession,
    ) -> Result<Vec<PreparedComicPage>, AppError> {
        let source = revalidate_source(session, false)?;
        let mut file = File::open(&source).map_err(|_| resource_unavailable())?;
        let source_size = file.metadata().map_err(|_| resource_unavailable())?.len();
        if source_size > MAX_COMIC_ARCHIVE_BYTES {
            return Err(policy_denied("漫画归档超过安全大小限制"));
        }
        let source_sha256 = open_file_sha256(&mut file, source_size)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| resource_unavailable())?;
        let mut archive = ZipArchive::new(file).map_err(|_| unsupported_archive())?;
        if archive.len() > MAX_ARCHIVE_ENTRIES {
            return Err(policy_denied("漫画归档条目数超过安全限制"));
        }

        let mut pages = Vec::new();
        let mut names = HashSet::new();
        let mut total_uncompressed = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|_| unsupported_archive())?;
            let raw_name = entry.name_raw();
            if raw_name.len() > MAX_ENTRY_NAME_BYTES {
                return Err(policy_denied("漫画归档条目名称超过安全限制"));
            }
            let name = std::str::from_utf8(raw_name).map_err(|_| unsupported_archive())?;
            let normalized_name = safe_archive_name(name, entry.enclosed_name().as_deref())?;
            if entry.encrypted() {
                return Err(policy_denied("漫画归档包含加密条目"));
            }
            if entry.unix_mode().is_some_and(is_symlink_mode) {
                return Err(policy_denied("漫画归档包含符号链接"));
            }
            if entry.is_dir() {
                continue;
            }
            if is_nested_archive(&normalized_name) {
                return Err(policy_denied("漫画归档包含嵌套归档"));
            }
            if !is_supported_image_name(&normalized_name) {
                continue;
            }
            if pages.len() >= MAX_COMIC_PAGES {
                return Err(policy_denied("漫画页数超过安全限制"));
            }
            let dedup_key = normalized_sort_key(&normalized_name);
            if !names.insert(dedup_key) {
                return Err(policy_denied("漫画归档包含重复页面名称"));
            }
            let uncompressed_size = entry.size();
            let compressed_size = entry.compressed_size();
            total_uncompressed = total_uncompressed
                .checked_add(uncompressed_size)
                .ok_or_else(|| policy_denied("漫画归档展开大小溢出"))?;
            if total_uncompressed > MAX_COMIC_TOTAL_BYTES {
                return Err(policy_denied("漫画归档总展开大小超过安全限制"));
            }
            if exceeds_ratio(uncompressed_size, compressed_size) {
                return Err(policy_denied("漫画归档压缩比超过安全限制"));
            }
            let availability = if uncompressed_size == 0 || uncompressed_size > MAX_COMIC_PAGE_BYTES
            {
                PreparedComicPageAvailability::Unavailable
            } else {
                let mut prefix = [0u8; 12];
                let prefix_len = entry.read(&mut prefix).ok();
                if prefix_len
                    .and_then(|len| sniff_image(&prefix[..len]))
                    .is_some()
                {
                    PreparedComicPageAvailability::Ready
                } else {
                    PreparedComicPageAvailability::Unavailable
                }
            };
            pages.push(PreparedComicPage {
                availability,
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: u32::try_from(index)
                        .map_err(|_| policy_denied("漫画归档条目数超过安全限制"))?,
                    normalized_name,
                    crc32: entry.crc32(),
                    compressed_size,
                    uncompressed_size,
                    source_size,
                    source_sha256,
                },
            });
        }
        sort_pages(&mut pages);
        if !pages
            .iter()
            .any(|page| page.availability == PreparedComicPageAvailability::Ready)
        {
            return Err(unsupported_archive());
        }
        Ok(pages)
    }

    fn inspect_directory(
        &self,
        session: &PreparedSession,
    ) -> Result<Vec<PreparedComicPage>, AppError> {
        let source = revalidate_source(session, true)?;
        let mut pages = Vec::new();
        let mut names = HashSet::new();
        let mut total = 0u64;
        for item in fs::read_dir(&source).map_err(|_| resource_unavailable())? {
            let item = item.map_err(|_| resource_unavailable())?;
            let file_type = item.file_type().map_err(|_| resource_unavailable())?;
            if file_type.is_symlink() || has_reparse_point(&item.path())? {
                return Err(policy_denied("漫画图片目录包含链接或重解析点"));
            }
            if !file_type.is_file() {
                continue;
            }
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| unsupported_archive())?;
            if name.len() > MAX_ENTRY_NAME_BYTES || name.contains(['/', '\\', ':']) {
                return Err(policy_denied("漫画图片文件名不安全"));
            }
            if !is_supported_image_name(&name) {
                continue;
            }
            if pages.len() >= MAX_COMIC_PAGES {
                return Err(policy_denied("漫画页数超过安全限制"));
            }
            let key = normalized_sort_key(&name);
            if !names.insert(key) {
                return Err(policy_denied("漫画图片目录包含重复规范名"));
            }
            let metadata = item.metadata().map_err(|_| resource_unavailable())?;
            let size = metadata.len();
            total = total
                .checked_add(size)
                .ok_or_else(|| policy_denied("漫画图片总大小溢出"))?;
            if total > MAX_COMIC_TOTAL_BYTES {
                return Err(policy_denied("漫画图片总大小超过安全限制"));
            }
            let (availability, sha256) = if size == 0 || size > MAX_COMIC_PAGE_BYTES {
                (PreparedComicPageAvailability::Unavailable, [0; 32])
            } else if file_has_supported_image_magic(&item.path())? {
                (
                    PreparedComicPageAvailability::Ready,
                    file_sha256(&item.path(), size)?,
                )
            } else {
                (PreparedComicPageAvailability::Unavailable, [0; 32])
            };
            pages.push(PreparedComicPage {
                availability,
                source: PreparedComicPageSource::DirectoryFile {
                    relative_name: name,
                    expected_size: size,
                    sha256,
                },
            });
        }
        sort_pages(&mut pages);
        if !pages
            .iter()
            .any(|page| page.availability == PreparedComicPageAvailability::Ready)
        {
            return Err(unsupported_archive());
        }
        Ok(pages)
    }

    fn read_archive_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        let PreparedComicPageSource::ArchiveEntry {
            entry_index,
            normalized_name,
            crc32,
            compressed_size,
            uncompressed_size,
            source_size,
            source_sha256,
        } = &page.source
        else {
            return Err(unsupported_archive());
        };
        let source = revalidate_source(session, false)?;
        let mut file = File::open(source).map_err(|_| resource_unavailable())?;
        let current_source_size = file.metadata().map_err(|_| resource_unavailable())?.len();
        if current_source_size != *source_size
            || open_file_sha256(&mut file, current_source_size)? != *source_sha256
        {
            return Err(resource_unavailable());
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| resource_unavailable())?;
        let mut archive = ZipArchive::new(file).map_err(|_| unsupported_archive())?;
        let mut entry = archive
            .by_index(*entry_index as usize)
            .map_err(|_| resource_unavailable())?;
        let current_name =
            std::str::from_utf8(entry.name_raw()).map_err(|_| resource_unavailable())?;
        let current_name = safe_archive_name(current_name, entry.enclosed_name().as_deref())?;
        if &current_name != normalized_name
            || entry.crc32() != *crc32
            || entry.compressed_size() != *compressed_size
            || entry.size() != *uncompressed_size
        {
            return Err(resource_unavailable());
        }
        read_bounded_image(&mut entry, *uncompressed_size)
    }

    fn read_directory_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        let PreparedComicPageSource::DirectoryFile {
            relative_name,
            expected_size,
            sha256,
        } = &page.source
        else {
            return Err(unsupported_archive());
        };
        let source = revalidate_source(session, true)?;
        let raw_page = source.join(relative_name);
        let canonical_page = fs::canonicalize(raw_page).map_err(|_| resource_unavailable())?;
        if canonical_page.strip_prefix(&source).is_err()
            || canonical_page
                .strip_prefix(&session.canonical_root)
                .is_err()
            || !canonical_page.is_file()
            || has_reparse_point(&canonical_page)?
        {
            return Err(policy_denied("漫画页面路径校验失败"));
        }
        let mut file = File::open(canonical_page).map_err(|_| resource_unavailable())?;
        let size = file.metadata().map_err(|_| resource_unavailable())?.len();
        if size != *expected_size {
            return Err(resource_unavailable());
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| resource_unavailable())?;
        let body = read_bounded_image(&mut file, size)?;
        if bytes_sha256(&body.bytes) != *sha256 {
            return Err(resource_unavailable());
        }
        Ok(body)
    }
}

impl ComicPageProvider for LocalComicPageProvider {
    fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError> {
        match session.resource_type {
            ResourceType::ComicArchive => self.inspect_archive(session),
            ResourceType::ImageSequence => self.inspect_directory(session),
            _ => Err(unsupported_archive()),
        }
    }

    fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        if page.availability != PreparedComicPageAvailability::Ready {
            return Err(unsupported_archive());
        }
        match session.resource_type {
            ResourceType::ComicArchive => self.read_archive_page(session, page),
            ResourceType::ImageSequence => self.read_directory_page(session, page),
            _ => Err(unsupported_archive()),
        }
    }
}

fn revalidate_source(
    session: &PreparedSession,
    expects_directory: bool,
) -> Result<std::path::PathBuf, AppError> {
    revalidate_source_with_message(session, expects_directory, "漫画资源路径校验失败")
}

/// 共享的会话资源重校验：规范化为与登记时完全一致、仍位于存储根内且无重解析点。
/// 供漫画与阅读（EPUB TOC）Provider 复用同一安全不变量。
pub(crate) fn revalidate_source_with_message(
    session: &PreparedSession,
    expects_directory: bool,
    denied_message: &'static str,
) -> Result<std::path::PathBuf, AppError> {
    let root = fs::canonicalize(&session.canonical_root).map_err(|_| resource_unavailable())?;
    let source = fs::canonicalize(&session.canonical_file).map_err(|_| resource_unavailable())?;
    if root != session.canonical_root
        || source != session.canonical_file
        || source.strip_prefix(&root).is_err()
        || (expects_directory && !source.is_dir())
        || (!expects_directory && !source.is_file())
        || has_reparse_point(&source)?
    {
        return Err(policy_denied(denied_message));
    }
    Ok(source)
}

fn safe_archive_name(name: &str, enclosed: Option<&Path>) -> Result<String, AppError> {
    if name.is_empty()
        || name.starts_with(['/', '\\'])
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
    {
        return Err(policy_denied("漫画归档条目路径不安全"));
    }
    let path = enclosed.ok_or_else(|| policy_denied("漫画归档条目路径不安全"))?;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                parts.push(part.to_str().ok_or_else(unsupported_archive)?.to_owned())
            }
            _ => return Err(policy_denied("漫画归档条目路径不安全")),
        }
    }
    if parts.is_empty() {
        return Err(policy_denied("漫画归档条目路径不安全"));
    }
    Ok(parts.join("/"))
}

fn is_symlink_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o120000
}

fn is_supported_image_name(name: &str) -> bool {
    extension(name).is_some_and(|ext| matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp"))
}

fn is_nested_archive(name: &str) -> bool {
    extension(name)
        .is_some_and(|ext| matches!(ext.as_str(), "zip" | "cbz" | "cbr" | "cb7" | "rar" | "7z"))
}

fn extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}

fn exceeds_ratio(uncompressed: u64, compressed: u64) -> bool {
    uncompressed > 0
        && (compressed == 0 || uncompressed > compressed.saturating_mul(MAX_COMPRESSION_RATIO))
}

fn page_name(page: &PreparedComicPage) -> &str {
    match &page.source {
        PreparedComicPageSource::ArchiveEntry {
            normalized_name, ..
        } => normalized_name,
        PreparedComicPageSource::DirectoryFile { relative_name, .. } => relative_name,
    }
}

fn sort_pages(pages: &mut [PreparedComicPage]) {
    pages.sort_by(|left, right| {
        natural_cmp(page_name(left), page_name(right))
            .then_with(|| page_name(left).cmp(page_name(right)))
    });
}

fn normalized_sort_key(value: &str) -> String {
    value.nfc().flat_map(char::to_lowercase).collect()
}

pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = normalized_sort_key(left);
    let right = normalized_sort_key(right);
    let lb = left.as_bytes();
    let rb = right.as_bytes();
    let (mut li, mut ri) = (0usize, 0usize);
    while li < lb.len() && ri < rb.len() {
        if lb[li].is_ascii_digit() && rb[ri].is_ascii_digit() {
            let (lend, rend) = (digit_end(lb, li), digit_end(rb, ri));
            let ltrim = trim_zeros(&lb[li..lend]);
            let rtrim = trim_zeros(&rb[ri..rend]);
            let order = ltrim
                .len()
                .cmp(&rtrim.len())
                .then_with(|| ltrim.cmp(rtrim))
                .then_with(|| (lend - li).cmp(&(rend - ri)));
            if order != Ordering::Equal {
                return order;
            }
            li = lend;
            ri = rend;
        } else {
            let order = lb[li].cmp(&rb[ri]);
            if order != Ordering::Equal {
                return order;
            }
            li += 1;
            ri += 1;
        }
    }
    lb.len().cmp(&rb.len())
}

fn digit_end(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    index
}

fn trim_zeros(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(bytes.len());
    &bytes[first..]
}

fn read_bounded_image(
    reader: &mut impl Read,
    declared_size: u64,
) -> Result<ComicPageBody, AppError> {
    if declared_size == 0 || declared_size > MAX_COMIC_PAGE_BYTES {
        return Err(unsupported_archive());
    }
    let mut bytes = Vec::with_capacity(declared_size as usize);
    reader
        .take(MAX_COMIC_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unsupported_archive())?;
    if bytes.len() as u64 != declared_size || bytes.len() as u64 > MAX_COMIC_PAGE_BYTES {
        return Err(unsupported_archive());
    }
    let mime_type = sniff_image(&bytes).ok_or_else(unsupported_archive)?;
    Ok(ComicPageBody { mime_type, bytes })
}

fn file_sha256(path: &Path, size: u64) -> Result<[u8; 32], AppError> {
    let mut file = File::open(path).map_err(|_| resource_unavailable())?;
    open_file_sha256(&mut file, size)
}

fn bytes_sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

fn open_file_sha256(file: &mut File, size: u64) -> Result<[u8; 32], AppError> {
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    file.seek(SeekFrom::Start(0))
        .map_err(|_| resource_unavailable())?;
    let mut remaining = size;
    let mut buffer = [0u8; HASH_BUFFER_BYTES];
    while remaining > 0 {
        let chunk_len = usize::try_from(remaining.min(HASH_BUFFER_BYTES as u64))
            .map_err(|_| resource_unavailable())?;
        file.read_exact(&mut buffer[..chunk_len])
            .map_err(|_| resource_unavailable())?;
        hasher.update(&buffer[..chunk_len]);
        remaining -= chunk_len as u64;
    }
    Ok(hasher.finalize().into())
}

fn sniff_image(bytes: &[u8]) -> Option<ComicImageMime> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ComicImageMime::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ComicImageMime::Png)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ComicImageMime::Webp)
    } else {
        None
    }
}

fn file_has_supported_image_magic(path: &Path) -> Result<bool, AppError> {
    let mut file = File::open(path).map_err(|_| resource_unavailable())?;
    let mut prefix = [0u8; 12];
    let prefix_len = file.read(&mut prefix).map_err(|_| resource_unavailable())?;
    Ok(sniff_image(&prefix[..prefix_len]).is_some())
}

#[cfg(windows)]
fn has_reparse_point(path: &Path) -> Result<bool, AppError> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let metadata = fs::symlink_metadata(path).map_err(|_| resource_unavailable())?;
    Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn has_reparse_point(path: &Path) -> Result<bool, AppError> {
    Ok(fs::symlink_metadata(path)
        .map_err(|_| resource_unavailable())?
        .file_type()
        .is_symlink())
}

fn unsupported_archive() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "漫画资源已损坏或不受支持",
        false,
    )
}

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "漫画资源当前不可用，请重新打开",
        false,
    )
}

fn policy_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::wire::SessionEngineDto;
    use haven_domain::enums::{MediaType, ResourceType};
    use haven_domain::ids::{ResourceId, StorageLocationId};
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn png(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    fn prepared_session(
        root: &Path,
        source: &Path,
        resource_type: ResourceType,
    ) -> PreparedSession {
        PreparedSession {
            work_id: "work".into(),
            edition_id: "edition".into(),
            media_item_id: uuid::Uuid::now_v7().to_string(),
            engine: SessionEngineDto::Comic,
            resource_id: ResourceId::new(),
            storage_location_id: StorageLocationId::new(),
            canonical_root: fs::canonicalize(root).unwrap(),
            canonical_file: fs::canonicalize(source).unwrap(),
            mime_type: None,
            media_type: MediaType::Comic,
            resource_type,
            comic_pages: None,
            progress: None,
        }
    }

    fn write_archive(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn natural_order_keeps_persisted_page_index_human_stable() {
        let mut names = vec!["page10.jpg", "Page2.jpg", "page01.jpg", "page1.jpg"];
        names.sort_by(|left, right| natural_cmp(left, right).then_with(|| left.cmp(right)));
        assert_eq!(
            names,
            ["page1.jpg", "page01.jpg", "Page2.jpg", "page10.jpg"]
        );
    }

    #[test]
    fn image_magic_is_narrow() {
        assert_eq!(
            sniff_image(&[0xff, 0xd8, 0xff, 0]),
            Some(ComicImageMime::Jpeg)
        );
        assert_eq!(
            sniff_image(b"\x89PNG\r\n\x1a\nrest"),
            Some(ComicImageMime::Png)
        );
        assert_eq!(sniff_image(b"RIFF0000WEBPrest"), Some(ComicImageMime::Webp));
        assert_eq!(sniff_image(b"<svg></svg>"), None);
    }

    #[test]
    fn archive_names_reject_escape_and_windows_path_forms() {
        for name in ["../page.jpg", "/page.jpg", "C:/page.jpg", "dir\\page.jpg"] {
            assert!(safe_archive_name(name, None).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn compression_ratio_is_bounded() {
        assert!(!exceeds_ratio(100, 1));
        assert!(exceeds_ratio(101, 1));
        assert!(exceeds_ratio(1, 0));
    }

    #[test]
    fn archive_inspection_sorts_pages_and_reads_only_the_selected_image() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("chapter.cbz");
        write_archive(
            &archive,
            &[
                ("page10.png", png(b"ten")),
                ("notes.txt", b"ignored".to_vec()),
                ("page2.png", png(b"two")),
            ],
        );
        let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        let provider = LocalComicPageProvider::new();

        let pages = provider.inspect(&session).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(page_name(&pages[0]), "page2.png");
        assert_eq!(page_name(&pages[1]), "page10.png");
        let body = provider.read_page(&session, &pages[0]).unwrap();
        assert_eq!(body.mime_type, ComicImageMime::Png);
        assert_eq!(body.bytes, png(b"two"));
    }

    #[test]
    fn archive_guard_rejects_escape_nested_symlink_and_normalized_duplicates() {
        for (label, entries) in [
            ("escape", vec![("../page.jpg", vec![0xff, 0xd8, 0xff])]),
            ("nested", vec![("nested.cbz", b"zip".to_vec())]),
            (
                "duplicate",
                vec![
                    ("Page1.jpg", vec![0xff, 0xd8, 0xff]),
                    ("page1.jpg", vec![0xff, 0xd8, 0xff]),
                ],
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join(format!("{label}.cbz"));
            write_archive(&archive, &entries);
            let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
            assert_eq!(
                LocalComicPageProvider::new()
                    .inspect(&session)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "SECURITY_POLICY_DENIED",
                "{label} archive was accepted"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("symlink.cbz");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .add_symlink(
                "page.jpg",
                "target.jpg",
                SimpleFileOptions::default().unix_permissions(0o120777),
            )
            .unwrap();
        writer.finish().unwrap();
        let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        assert_eq!(
            LocalComicPageProvider::new()
                .inspect(&session)
                .unwrap_err()
                .code()
                .as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }

    #[test]
    fn invalid_image_magic_is_unavailable_without_reordering_valid_pages() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad-magic.cbz");
        write_archive(
            &archive,
            &[
                ("page1.jpg", b"not-a-jpeg".to_vec()),
                ("page2.png", png(b"valid")),
            ],
        );
        let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        let provider = LocalComicPageProvider::new();
        let pages = provider.inspect(&session).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(
            pages[0].availability,
            PreparedComicPageAvailability::Unavailable
        );
        assert_eq!(pages[1].availability, PreparedComicPageAvailability::Ready);
        assert_eq!(
            provider.read_page(&session, &pages[1]).unwrap().bytes,
            png(b"valid")
        );
    }

    #[test]
    fn resources_without_any_supported_page_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("empty.cbz");
        write_archive(&archive, &[("notes.txt", b"metadata".to_vec())]);
        let archive_session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        assert_eq!(
            LocalComicPageProvider::new()
                .inspect(&archive_session)
                .unwrap_err()
                .code()
                .as_str(),
            "FORMAT_UNSUPPORTED"
        );

        let chapter = dir.path().join("empty-directory");
        fs::create_dir(&chapter).unwrap();
        fs::write(chapter.join("page1.jpg"), b"not-a-jpeg").unwrap();
        let directory_session = prepared_session(dir.path(), &chapter, ResourceType::ImageSequence);
        assert_eq!(
            LocalComicPageProvider::new()
                .inspect(&directory_session)
                .unwrap_err()
                .code()
                .as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn image_sequence_uses_direct_children_and_rejects_same_size_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("chapter");
        let nested = chapter.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let page = chapter.join("page1.png");
        let mut original = png(&vec![b'a'; 16 * 1024]);
        fs::write(&page, &original).unwrap();
        let oversized = File::create(chapter.join("page2.jpg")).unwrap();
        oversized.set_len(MAX_COMIC_PAGE_BYTES + 1).unwrap();
        drop(oversized);
        fs::write(nested.join("page0.png"), png(b"nested")).unwrap();
        let session = prepared_session(dir.path(), &chapter, ResourceType::ImageSequence);
        let provider = LocalComicPageProvider::new();

        let pages = provider.inspect(&session).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(page_name(&pages[0]), "page1.png");
        assert_eq!(
            pages[1].availability,
            PreparedComicPageAvailability::Unavailable
        );

        original[8 * 1024] = b'b';
        fs::write(&page, &original).unwrap();
        let error = provider
            .read_page(&session, &pages[0])
            .err()
            .expect("same-size replacement must invalidate the prepared page");
        assert_eq!(error.code().as_str(), "RESOURCE_UNAVAILABLE");
    }

    #[test]
    fn archive_snapshot_rejects_same_size_replacement_and_truncated_directory() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("chapter.cbz");
        write_archive(&archive, &[("page1.png", png(&vec![b'a'; 32 * 1024]))]);
        let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        let provider = LocalComicPageProvider::new();
        let pages = provider.inspect(&session).unwrap();

        let data_start = {
            let file = File::open(&archive).unwrap();
            let mut zip = ZipArchive::new(file).unwrap();
            zip.by_index(0).unwrap().data_start()
        };
        let mut file = File::options().write(true).open(&archive).unwrap();
        file.seek(SeekFrom::Start(data_start + 16 * 1024)).unwrap();
        file.write_all(b"b").unwrap();
        drop(file);
        let error = provider
            .read_page(&session, &pages[0])
            .err()
            .expect("same-size archive replacement must invalidate the grant");
        assert_eq!(error.code().as_str(), "RESOURCE_UNAVAILABLE");

        let truncated = dir.path().join("truncated.cbz");
        write_archive(&truncated, &[("page1.png", png(b"page"))]);
        let file = File::options().write(true).open(&truncated).unwrap();
        let size = file.metadata().unwrap().len();
        file.set_len(size - 5).unwrap();
        let truncated_session =
            prepared_session(dir.path(), &truncated, ResourceType::ComicArchive);
        assert_eq!(
            provider
                .inspect(&truncated_session)
                .unwrap_err()
                .code()
                .as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn archive_guard_rejects_a_real_deflate_ratio_bomb() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("ratio.cbz");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "page1.png",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        let mut payload = png(&[]);
        payload.resize(1024 * 1024, 0);
        writer.write_all(&payload).unwrap();
        writer.finish().unwrap();

        let session = prepared_session(dir.path(), &archive, ResourceType::ComicArchive);
        assert_eq!(
            LocalComicPageProvider::new()
                .inspect(&session)
                .unwrap_err()
                .code()
                .as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }
}
