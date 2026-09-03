//! Bounded EPUB archive validation for the local scanner, plus the Reader TOC
//! extraction for `reader_toc_get`.
//!
//! This module deliberately validates only the archive boundary.  It does not
//! build a publication model or expose EPUB entries through IPC.  A file must
//! pass this guard before the scanner creates a local Resource.
//!
//! TOC extraction stays bounded: only the container/OPF and a single nav/NCX
//! document are read, every href is percent-decoded and re-validated before
//! use, and node counts/titles are capped by the Application constants.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path};

use haven_application::services::PreparedSession;
use haven_application::services::reader_toc::{
    MAX_TOC_ITEMS, MAX_TOC_TITLE_CHARS, RawEpubToc, RawTocNode, ReaderTocProvider,
};
use haven_common::{AppError, ErrorKind};
use zip::{CompressionMethod, ZipArchive, ZipReadOptions};

pub const MAX_EPUB_ENTRIES: usize = 10_000;
pub const MAX_EPUB_ENTRY_NAME_BYTES: usize = 1_024;
pub const MAX_EPUB_TOTAL_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_EPUB_COMPRESSION_RATIO: u64 = 100;
pub const MAX_EPUB_XML_BYTES: u64 = 4 * 1024 * 1024;

const EPUB_MIMETYPE: &[u8] = b"application/epub+zip";

/// Validate an EPUB file without extracting or exposing any archive entry.
///
/// The scanner uses this as a file-level guard.  Invalid or hostile archives
/// return a stable, path-free error and are not indexed as usable resources.
pub fn validate_epub_file(path: &Path) -> Result<(), AppError> {
    let file = File::open(path).map_err(|_| format_unsupported())?;
    let mut archive = ZipArchive::new(file).map_err(|_| format_unsupported())?;
    if archive.is_empty() || archive.len() > MAX_EPUB_ENTRIES {
        return Err(policy_denied("EPUB 归档条目数超过安全限制"));
    }

    let mut names = HashSet::with_capacity(archive.len());
    let mut total_uncompressed = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index_with_options(index, ZipReadOptions::new().ignore_encryption_flag(true))
            .map_err(|_| format_unsupported())?;
        let raw_name = entry.name_raw();
        if raw_name.len() > MAX_EPUB_ENTRY_NAME_BYTES {
            return Err(policy_denied("EPUB 归档条目名称超过安全限制"));
        }
        let name = std::str::from_utf8(raw_name).map_err(|_| format_unsupported())?;
        let normalized_name = safe_archive_name(name)?;
        if !names.insert(normalized_name.clone()) {
            return Err(policy_denied("EPUB 归档包含重复条目名称"));
        }
        if entry.encrypted() {
            return Err(policy_denied("EPUB 归档包含加密条目"));
        }
        if entry.unix_mode().is_some_and(is_symlink_mode) {
            return Err(policy_denied("EPUB 归档包含符号链接"));
        }
        if is_nested_archive(&normalized_name) {
            return Err(policy_denied("EPUB 归档包含嵌套归档"));
        }

        let uncompressed_size = entry.size();
        total_uncompressed = total_uncompressed
            .checked_add(uncompressed_size)
            .ok_or_else(|| policy_denied("EPUB 归档展开大小溢出"))?;
        if total_uncompressed > MAX_EPUB_TOTAL_UNCOMPRESSED_BYTES {
            return Err(policy_denied("EPUB 归档总展开大小超过安全限制"));
        }
        if exceeds_ratio(uncompressed_size, entry.compressed_size()) {
            return Err(policy_denied("EPUB 归档压缩比超过安全限制"));
        }

        if index == 0 {
            if normalized_name != "mimetype"
                || entry.is_dir()
                || entry.compression() != CompressionMethod::Stored
                || uncompressed_size != EPUB_MIMETYPE.len() as u64
            {
                return Err(format_unsupported());
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|_| format_unsupported())?;
            if bytes != EPUB_MIMETYPE {
                return Err(format_unsupported());
            }
        }
    }

    let container_xml = read_archive_text(&mut archive, &names, "META-INF/container.xml")?;
    let opf_path = parse_container_rootfile(&container_xml)?;
    if !opf_path.to_ascii_lowercase().ends_with(".opf") {
        return Err(format_unsupported());
    }
    let opf_xml = read_archive_text(&mut archive, &names, &opf_path)?;
    validate_package_and_spine(&opf_path, &opf_xml, &names)
}

fn read_archive_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<String>,
    name: &str,
) -> Result<String, AppError> {
    if !names.contains(name) {
        return Err(format_unsupported());
    }
    let entry = archive.by_name(name).map_err(|_| format_unsupported())?;
    if entry.is_dir() || entry.encrypted() || entry.size() > MAX_EPUB_XML_BYTES {
        return Err(format_unsupported());
    }
    let mut bytes = Vec::new();
    entry
        .take(MAX_EPUB_XML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format_unsupported())?;
    if bytes.len() as u64 > MAX_EPUB_XML_BYTES {
        return Err(format_unsupported());
    }
    String::from_utf8(bytes).map_err(|_| format_unsupported())
}

fn parse_container_rootfile(xml: &str) -> Result<String, AppError> {
    let attributes = find_tag_attributes(xml, "rootfile")
        .into_iter()
        .next()
        .ok_or_else(format_unsupported)?;
    let path = attributes
        .get("full-path")
        .filter(|value| !value.is_empty())
        .ok_or_else(format_unsupported)?;
    safe_archive_name(path)
}

fn validate_package_and_spine(
    opf_path: &str,
    xml: &str,
    names: &HashSet<String>,
) -> Result<(), AppError> {
    if find_tag_attributes(xml, "package").is_empty()
        || find_tag_attributes(xml, "manifest").is_empty()
        || find_tag_attributes(xml, "spine").is_empty()
    {
        return Err(format_unsupported());
    }

    let manifest = find_tag_attributes(xml, "item")
        .into_iter()
        .filter_map(|attrs| {
            let id = attrs.get("id")?.to_owned();
            let href = attrs.get("href")?.to_owned();
            let media_type = attrs
                .get("media-type")
                .map(|value| base_mime(value).to_ascii_lowercase())?;
            Some((id, (href, media_type)))
        })
        .collect::<HashMap<_, _>>();
    if manifest.is_empty() {
        return Err(format_unsupported());
    }

    let spine_refs = find_tag_attributes(xml, "itemref")
        .into_iter()
        .map(|attrs| {
            attrs
                .get("idref")
                .filter(|idref| !idref.is_empty())
                .cloned()
                .ok_or_else(format_unsupported)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if spine_refs.is_empty() {
        return Err(format_unsupported());
    }

    let opf_directory = opf_path.rsplit_once('/').map(|(directory, _)| directory);
    let mut has_document = false;
    for idref in &spine_refs {
        let Some((href, media_type)) = manifest.get(idref) else {
            return Err(format_unsupported());
        };
        if media_type != "application/xhtml+xml" && media_type != "text/html" {
            return Err(format_unsupported());
        }
        let path = resolve_epub_href(opf_directory, href)?;
        if !names.contains(&path) {
            return Err(format_unsupported());
        }
        has_document = true;
    }
    if !has_document {
        return Err(format_unsupported());
    }
    Ok(())
}

fn resolve_epub_href(directory: Option<&str>, href: &str) -> Result<String, AppError> {
    let href = href.split_once('#').map_or(href, |(path, _)| path);
    if href.is_empty()
        || href.starts_with('/')
        || href.contains(['\\', '\0', ':', '?'])
        || href.contains("://")
    {
        return Err(policy_denied("EPUB 文档引用路径不安全"));
    }
    let combined = directory.map_or_else(|| href.to_owned(), |base| format!("{base}/{href}"));
    safe_archive_name(&combined)
}

/// Resolve a manifest document (nav/NCX) using the ZIP entry's decoded name.
///
/// OPF hrefs are URI references, so a document such as `nav%20file.xhtml`
/// may be stored as `nav file.xhtml` in the archive.  Keep the existing raw
/// href validation, then decode and validate once more before opening the
/// archive entry.  The second pass is important because an encoded `..`,
/// separator, colon, NUL, or query marker must not bypass the archive policy.
fn resolve_manifest_document_href(directory: Option<&str>, href: &str) -> Result<String, AppError> {
    let raw = resolve_epub_href(directory, href)?;
    let decoded =
        percent_decode_path(&raw).map_err(|_| policy_denied("EPUB 文档引用路径不安全"))?;
    if decoded.contains('?') {
        return Err(policy_denied("EPUB 文档引用路径不安全"));
    }
    safe_archive_name(&decoded)
}

fn safe_archive_name(name: &str) -> Result<String, AppError> {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains(['\\', ':', '\0']) {
        return Err(policy_denied("EPUB 归档条目路径不安全"));
    }
    let path = Path::new(name);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(format_unsupported)?.trim();
                if part.is_empty() {
                    return Err(policy_denied("EPUB 归档条目路径不安全"));
                }
                parts.push(part.to_owned());
            }
            _ => return Err(policy_denied("EPUB 归档条目路径不安全")),
        }
    }
    if parts.is_empty() {
        return Err(policy_denied("EPUB 归档条目路径不安全"));
    }
    Ok(parts.join("/"))
}

fn find_tag_attributes(xml: &str, wanted_tag: &str) -> Vec<HashMap<String, String>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = xml[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(end) = find_tag_end(xml, start + 1) else {
            break;
        };
        let mut inner = &xml[start + 1..end];
        inner = inner.trim_start();
        if inner.starts_with(['/', '!', '?']) {
            cursor = end + 1;
            continue;
        }
        let tag_end = inner
            .find(|character: char| character.is_ascii_whitespace() || character == '/')
            .unwrap_or(inner.len());
        let tag = &inner[..tag_end];
        if tag == wanted_tag {
            result.push(parse_attributes(&inner[tag_end..]));
        }
        cursor = end + 1;
    }
    result
}

fn find_tag_end(xml: &str, mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < xml.len() {
        let character = xml.as_bytes()[cursor] as char;
        if let Some(expected) = quote {
            if character == expected {
                quote = None;
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character == '>' {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn parse_attributes(mut value: &str) -> HashMap<String, String> {
    let mut attributes = HashMap::new();
    while !value.trim_start().is_empty() {
        value = value.trim_start();
        value = value.trim_start_matches('/').trim_start();
        if value.is_empty() {
            break;
        }
        let key_end = value
            .find(|character: char| character.is_ascii_whitespace() || character == '=')
            .unwrap_or(value.len());
        if key_end == 0 {
            break;
        }
        let key = value[..key_end].to_ascii_lowercase();
        value = value[key_end..].trim_start();
        if !value.starts_with('=') {
            break;
        }
        value = value[1..].trim_start();
        let Some(quote) = value.chars().next() else {
            break;
        };
        if quote != '\'' && quote != '"' {
            break;
        }
        value = &value[quote.len_utf8()..];
        let Some(end) = value.find(quote) else {
            break;
        };
        attributes.insert(key, value[..end].to_owned());
        value = &value[end + quote.len_utf8()..];
    }
    attributes
}

fn is_nested_archive(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "epub" | "cbz" | "cbr" | "cb7" | "rar" | "7z"
            )
        })
}

fn exceeds_ratio(uncompressed: u64, compressed: u64) -> bool {
    uncompressed > 0
        && (compressed == 0 || uncompressed > compressed.saturating_mul(MAX_EPUB_COMPRESSION_RATIO))
}

fn is_symlink_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o120000
}

fn format_unsupported() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "EPUB 文件损坏或不受支持",
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

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "本地资源当前不可用",
        false,
    )
}

fn format_unsupported_toc() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "当前格式不支持章节目录",
        false,
    )
}

/// 阅读会话的 EPUB TOC Provider。只接收 server-only 会话事实。
#[derive(Debug, Default)]
pub struct LocalEpubTocProvider;

impl LocalEpubTocProvider {
    pub fn new() -> Self {
        Self
    }
}

impl ReaderTocProvider for LocalEpubTocProvider {
    fn extract(&self, session: &PreparedSession) -> Result<RawEpubToc, AppError> {
        if base_mime(session.mime_type.as_deref().unwrap_or("")) != "application/epub+zip" {
            return Err(format_unsupported_toc());
        }
        let source =
            crate::comic::revalidate_source_with_message(session, false, "阅读资源路径校验失败")?;
        extract_epub_toc(&source)
    }
}

struct ManifestItem {
    href: String,
    media_type: String,
    properties: String,
}

/// 从已通过扫描校验的 EPUB 抽取原始目录事实。反复执行扫描时的边界检查，
/// 因为文件可能在扫描后发生变化；任何不安全引用都按策略拒绝。
pub fn extract_epub_toc(path: &Path) -> Result<RawEpubToc, AppError> {
    let file = File::open(path).map_err(|_| resource_unavailable())?;
    let mut archive = ZipArchive::new(file).map_err(|_| format_unsupported())?;
    if archive.is_empty() || archive.len() > MAX_EPUB_ENTRIES {
        return Err(policy_denied("EPUB 归档条目数超过安全限制"));
    }
    let mut names = HashSet::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index_with_options(index, ZipReadOptions::new().ignore_encryption_flag(true))
            .map_err(|_| format_unsupported())?;
        let raw_name = entry.name_raw();
        if raw_name.len() > MAX_EPUB_ENTRY_NAME_BYTES {
            return Err(policy_denied("EPUB 归档条目名称超过安全限制"));
        }
        let name = std::str::from_utf8(raw_name).map_err(|_| format_unsupported())?;
        let normalized = safe_archive_name(name)?;
        if !names.insert(normalized.clone()) {
            return Err(policy_denied("EPUB 归档包含重复条目名称"));
        }
        if entry.encrypted() {
            return Err(policy_denied("EPUB 归档包含加密条目"));
        }
    }
    let container_xml = read_archive_text(&mut archive, &names, "META-INF/container.xml")?;
    let opf_path = parse_container_rootfile(&container_xml)?;
    let opf_xml = read_archive_text(&mut archive, &names, &opf_path)?;
    parse_publication_toc(&mut archive, &names, &opf_path, &opf_xml)
}

fn parse_publication_toc<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<String>,
    opf_path: &str,
    opf_xml: &str,
) -> Result<RawEpubToc, AppError> {
    let mut manifest = HashMap::new();
    for attributes in find_tag_attributes(opf_xml, "item") {
        let Some(id) = attributes.get("id").filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(href) = attributes.get("href").filter(|value| !value.is_empty()) else {
            continue;
        };
        let media_type = attributes
            .get("media-type")
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let media_type = base_mime(&media_type).to_owned();
        let properties = attributes
            .get("properties")
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        manifest.insert(
            id.to_owned(),
            ManifestItem {
                href: href.to_owned(),
                media_type,
                properties,
            },
        );
    }
    if manifest.is_empty() {
        return Err(format_unsupported());
    }
    let spine_refs = find_tag_attributes(opf_xml, "itemref")
        .into_iter()
        .map(|attributes| {
            attributes
                .get("idref")
                .filter(|idref| !idref.is_empty())
                .cloned()
                .ok_or_else(format_unsupported)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if spine_refs.is_empty() {
        return Err(format_unsupported());
    }
    let opf_directory = opf_path.rsplit_once('/').map(|(directory, _)| directory);

    let mut spine = Vec::with_capacity(spine_refs.len());
    for idref in &spine_refs {
        let item = manifest.get(idref).ok_or_else(format_unsupported)?;
        let raw = resolve_epub_href(opf_directory, &item.href)?;
        let decoded = percent_decode_path(&raw).map_err(|_| format_unsupported())?;
        spine.push(safe_archive_name(&decoded)?);
    }

    let mut nodes: Vec<(String, String, u32)> = Vec::new();
    if let Some(nav_item) = manifest
        .values()
        .find(|item| has_property_token(&item.properties, "nav"))
    {
        let doc = resolve_manifest_document_href(opf_directory, &nav_item.href)?;
        let nav_xml = read_archive_text(archive, names, &doc)?;
        if let Some(region) = find_toc_nav_region(&nav_xml) {
            collect_nav_entries(&nav_xml[region..], &mut nodes);
        }
    }
    if nodes.is_empty() {
        if let Some(ncx_item) = manifest
            .values()
            .find(|item| item.media_type == "application/x-dtbncx+xml")
        {
            let doc = resolve_manifest_document_href(opf_directory, &ncx_item.href)?;
            let ncx_xml = read_archive_text(archive, names, &doc)?;
            parse_ncx_navpoints(&ncx_xml, 0, &mut nodes);
        }
    }

    let mut resolved = Vec::new();
    if nodes.is_empty() {
        // 无显式目录：spine 扁平兜底，标题与前端章节命名一致。
        for (index, href) in spine.iter().enumerate() {
            resolved.push(RawTocNode {
                href: href.clone(),
                fragment: None,
                title: format!("第 {} 章", index + 1),
                depth: 0,
            });
        }
    } else {
        for (href, title, depth) in nodes {
            if resolved.len() >= MAX_TOC_ITEMS {
                break;
            }
            let Ok((path, fragment)) = resolve_toc_href(opf_directory, &href) else {
                continue;
            };
            let title = normalize_title(&title);
            if title.is_empty() || title.chars().count() > MAX_TOC_TITLE_CHARS {
                continue;
            }
            resolved.push(RawTocNode {
                href: path,
                fragment,
                title,
                depth,
            });
        }
    }
    Ok(RawEpubToc {
        spine,
        nodes: resolved,
    })
}

fn has_property_token(properties: &str, wanted: &str) -> bool {
    properties
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case(wanted))
}

/// 找到 `<nav epub:type="toc">` 的平衡区域；返回切片起点。
fn find_toc_nav_region(xml: &str) -> Option<usize> {
    let mut cursor = 0;
    while let Some(relative_start) = xml[cursor..].find("<nav") {
        let start = cursor + relative_start;
        let end = find_tag_end(xml, start + 1)?;
        let inner = xml[start + 1..end].trim_start();
        if inner.starts_with(['/', '!', '?']) || inner.trim_end().ends_with('/') {
            cursor = end + 1;
            continue;
        }
        let attributes = parse_attributes(&inner[tag_name(inner).len()..]);
        let toc = attributes
            .get("epub:type")
            .is_some_and(|value| has_property_token(value, "toc"));
        if !toc || !tag_name(inner).eq_ignore_ascii_case("nav") {
            cursor = end + 1;
            continue;
        }
        return Some(start);
    }
    None
}

fn tag_name(inner: &str) -> &str {
    let end = inner
        .find(|character: char| character.is_ascii_whitespace() || character == '/')
        .unwrap_or(inner.len());
    &inner[..end]
}

/// 从 `open_start` 起找同名标签的平衡闭合（处理嵌套），返回 (open_start, close_start)。
fn find_balanced_region(xml: &str, open_start: usize, name: &str) -> Option<(usize, usize)> {
    let end = find_tag_end(xml, open_start + 1)?;
    let mut depth = 1usize;
    let mut cursor = end + 1;
    while cursor < xml.len() {
        let relative = xml[cursor..].find('<')? + cursor;
        let tag_end = find_tag_end(xml, relative + 1)?;
        let mut inner = xml[relative + 1..tag_end].trim_start();
        let closing = inner.starts_with('/');
        if closing {
            inner = inner.trim_start_matches('/');
        }
        if inner.starts_with(['!', '?']) {
            cursor = tag_end + 1;
            continue;
        }
        if tag_name(inner).eq_ignore_ascii_case(name) {
            if closing {
                depth -= 1;
                if depth == 0 {
                    return Some((open_start, relative));
                }
            } else if !inner.trim_end().ends_with('/') {
                depth += 1;
            }
        }
        cursor = tag_end + 1;
    }
    None
}

/// 扁平扫描 nav 的 `<ol>/<li>` 结构：depth = 当前 ol 嵌套深度减一。
fn collect_nav_entries(xml: &str, out: &mut Vec<(String, String, u32)>) {
    let mut cursor = 0;
    let mut ol_depth = 0u32;
    while let Some(relative) = xml[cursor..].find('<') {
        let start = cursor + relative;
        let Some(end) = find_tag_end(xml, start + 1) else {
            break;
        };
        let mut inner = xml[start + 1..end].trim_start();
        if inner.starts_with(['!', '?']) {
            cursor = end + 1;
            continue;
        }
        let closing = inner.starts_with('/');
        if closing {
            inner = inner.trim_start_matches('/');
        }
        if closing {
            if tag_name(inner).eq_ignore_ascii_case("ol") {
                ol_depth = ol_depth.saturating_sub(1);
            }
            cursor = end + 1;
            continue;
        }
        if inner.trim_end().ends_with('/') {
            cursor = end + 1;
            continue;
        }
        let name = tag_name(inner);
        if name.eq_ignore_ascii_case("ol") {
            ol_depth = ol_depth.saturating_add(1);
            cursor = end + 1;
        } else if name.eq_ignore_ascii_case("li") && out.len() < MAX_TOC_ITEMS {
            let region_end = find_balanced_region(xml, start, "li")
                .map(|(_, close)| close)
                .unwrap_or(end);
            let depth = ol_depth.saturating_sub(1);
            if let Some((href, text)) = find_first_anchor(xml, start, region_end) {
                let title = normalize_title(&text);
                if !title.is_empty() {
                    out.push((href, title, depth));
                }
            }
            cursor = end + 1;
        } else {
            cursor = end + 1;
        }
    }
}

/// 在 [start, end) 内找第一个带 href 的 `<a>`，返回 (href, 内部文本)。
fn find_first_anchor(xml: &str, start: usize, end: usize) -> Option<(String, String)> {
    let mut cursor = start;
    while cursor < end {
        let relative = xml[cursor..end].find('<')? + cursor;
        let tag_end = find_tag_end(xml, relative + 1)?;
        if tag_end > end {
            return None;
        }
        let inner = xml[relative + 1..tag_end].trim_start();
        if inner.starts_with(['/', '!', '?']) || inner.trim_end().ends_with('/') {
            cursor = tag_end + 1;
            continue;
        }
        if !tag_name(inner).eq_ignore_ascii_case("a") {
            cursor = tag_end + 1;
            continue;
        }
        let attributes = parse_attributes(&inner[tag_name(inner).len()..]);
        let Some(href) = attributes.get("href").cloned() else {
            cursor = tag_end + 1;
            continue;
        };
        let (_, close) = find_balanced_region(xml, relative, "a")?;
        if close > end {
            return None;
        }
        let text = &xml[tag_end + 1..close];
        return Some((href, strip_html_tags(text)));
    }
    None
}

/// 递归解析 NCX navPoint 树。`pos` 为切片内绝对偏移。
fn parse_ncx_navpoints(xml: &str, depth: u32, out: &mut Vec<(String, String, u32)>) {
    let mut cursor = 0;
    while let Some(relative) = xml[cursor..].find('<') {
        let start = cursor + relative;
        let Some(end) = find_tag_end(xml, start + 1) else {
            break;
        };
        let inner = xml[start + 1..end].trim_start();
        if inner.starts_with(['/', '!', '?']) || inner.trim_end().ends_with('/') {
            cursor = end + 1;
            continue;
        }
        if !tag_name(inner).eq_ignore_ascii_case("navpoint") {
            cursor = end + 1;
            continue;
        }
        let (region_start, region_end) =
            find_balanced_region(xml, start, "navpoint").unwrap_or((start, end));
        let title = find_element_text(xml, region_start, region_end, "text");
        let href = find_element_attribute(xml, region_start, region_end, "content", "src");
        if let (Some(href), Some(title)) = (href, title) {
            let title = normalize_title(&title);
            if !title.is_empty() && out.len() < MAX_TOC_ITEMS {
                out.push((href, title, depth));
            }
        }
        parse_ncx_navpoints(&xml[region_start..region_end], depth.saturating_add(1), out);
        cursor = region_end;
    }
}

/// 在 [start, end) 内找第一个 `<name>` 的纯文本内容（元素不嵌套）。
fn find_element_text(xml: &str, start: usize, end: usize, name: &str) -> Option<String> {
    let mut cursor = start;
    while cursor < end {
        let relative = xml[cursor..end].find('<')? + cursor;
        let tag_end = find_tag_end(xml, relative + 1)?;
        if tag_end > end {
            return None;
        }
        let inner = xml[relative + 1..tag_end].trim_start();
        if inner.starts_with(['/', '!', '?']) {
            cursor = tag_end + 1;
            continue;
        }
        if tag_name(inner) != name {
            cursor = tag_end + 1;
            continue;
        }
        let close_pattern = format!("</{name}");
        let close = xml[tag_end + 1..end].find(&close_pattern)?;
        return Some(strip_html_tags(&xml[tag_end + 1..tag_end + 1 + close]));
    }
    None
}

/// 在 [start, end) 内找第一个 `<name>` 的指定属性（含自闭合标签）。
fn find_element_attribute(
    xml: &str,
    start: usize,
    end: usize,
    name: &str,
    attribute: &str,
) -> Option<String> {
    let mut cursor = start;
    while cursor < end {
        let relative = xml[cursor..end].find('<')? + cursor;
        let tag_end = find_tag_end(xml, relative + 1)?;
        if tag_end > end {
            return None;
        }
        let inner = xml[relative + 1..tag_end].trim_start();
        if inner.starts_with(['/', '!', '?']) {
            cursor = tag_end + 1;
            continue;
        }
        if tag_name(inner) != name {
            cursor = tag_end + 1;
            continue;
        }
        let attributes = parse_attributes(&inner[tag_name(inner).len()..]);
        return attributes.get(attribute).cloned();
    }
    None
}

/// 去除脚本/样式与标签，解码实体，折叠空白。用于目录标题，不做 HTML 渲染。
fn strip_html_tags(raw: &str) -> String {
    let without_blocks = raw
        .replace("<head", "\n")
        .replace("</head>", "\n")
        .replace("<script", "\n")
        .replace("</script>", "\n")
        .replace("<style", "\n")
        .replace("</style>", "\n");
    let without_blocks = strip_block_contents(&without_blocks);
    let mut text = String::with_capacity(without_blocks.len());
    let mut in_tag = false;
    for character in without_blocks.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    decode_xml_entities(&text)
}

/// 去掉 `<head|script|style>…</…>` 块内容（标题里不应有正文块，防御性剥离）。
fn strip_block_contents(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut search_from = 0;
    let mut cursor = 0;
    for block in ["head", "script", "style"] {
        let open = format!("<{block}");
        let close = format!("</{block}>");
        while let Some(relative) = raw[search_from..].find(&open) {
            let open_at = search_from + relative;
            let Some(close_at) = raw[open_at..].find(&close) else {
                break;
            };
            let close_at = open_at + close_at + close.len();
            cleaned.push_str(&raw[cursor..open_at]);
            cleaned.push(' ');
            cursor = close_at;
            search_from = close_at;
        }
    }
    if cursor == 0 {
        raw.to_owned()
    } else {
        cleaned.push_str(&raw[cursor..]);
        cleaned
    }
}

fn decode_xml_entities(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find('&') {
        let amp = cursor + relative;
        let Some(close) = value[amp..].find(';') else {
            break;
        };
        let close = amp + close;
        let entity = &value[amp + 1..close];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => decode_numeric_entity(entity),
        };
        result.push_str(&value[cursor..amp]);
        if let Some(character) = decoded {
            result.push(character);
        } else {
            result.push_str(&value[amp..=close]);
        }
        cursor = close + 1;
    }
    result.push_str(&value[cursor..]);
    result
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let code = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else if let Some(decimal) = entity.strip_prefix('#') {
        decimal.parse::<u32>().ok()?
    } else {
        return None;
    };
    char::from_u32(code)
}

/// 标题归一：解码实体后的文本折叠空白并裁剪长度。
fn normalize_title(raw: &str) -> String {
    let mut collapsed = String::with_capacity(raw.len());
    let mut pending_space = false;
    for character in raw.chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        pending_space = false;
        collapsed.push(character);
    }
    let trimmed = collapsed.trim();
    let mut capped = String::with_capacity(trimmed.len());
    for character in trimmed.chars().take(MAX_TOC_TITLE_CHARS) {
        capped.push(character);
    }
    capped
}

/// 对路径做 percent-decode；解码失败（非法转义 / 非 UTF-8）返回 Err。
fn percent_decode_path(value: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iterator = value.bytes();
    while let Some(byte) = iterator.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let high = iterator.next().ok_or(())?;
        let low = iterator.next().ok_or(())?;
        let high = hex_value(high).ok_or(())?;
        let low = hex_value(low).ok_or(())?;
        bytes.push(high * 16 + low);
    }
    String::from_utf8(bytes).map_err(|_| ())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 解析 nav/ncx 的 href：先 percent-decode，再执行与 spine 相同的安全校验，
/// 同时保留安全的文档内 fragment 供前端精确定位。
fn resolve_toc_href(
    directory: Option<&str>,
    href: &str,
) -> Result<(String, Option<String>), AppError> {
    let (path, raw_fragment) = href
        .split_once('#')
        .map_or((href, None), |(path, fragment)| (path, Some(fragment)));
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0', ':', '?'])
        || path.contains("://")
    {
        return Err(policy_denied("EPUB 文档引用路径不安全"));
    }
    let combined = directory.map_or_else(|| path.to_owned(), |base| format!("{base}/{path}"));
    let decoded =
        percent_decode_path(&combined).map_err(|_| policy_denied("EPUB 文档引用路径不安全"))?;
    let normalized_path = safe_archive_name(&decoded)?;
    let fragment = raw_fragment
        .filter(|value| !value.is_empty())
        .map(percent_decode_path)
        .transpose()
        .map_err(|_| policy_denied("EPUB 文档锚点不安全"))?
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if fragment.as_deref().is_some_and(|value| {
        value.chars().count() > 512
            || value.chars().any(|character| {
                character.is_control()
                    || matches!(character, '/' | '\\' | '?' | '#' | '<' | '>' | '"' | '\'')
            })
    }) {
        return Err(policy_denied("EPUB 文档锚点不安全"));
    }
    Ok((normalized_path, fragment))
}

/// Compare MIME values by their media-type token.  Scanner probes and remote
/// responses commonly append parameters such as `charset=utf-8`; those
/// parameters must not make an otherwise supported EPUB unreadable.
fn base_mime(value: &str) -> &str {
    value.split(';').next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    fn write_epub(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("mimetype", stored).unwrap();
        writer.write_all(EPUB_MIMETYPE).unwrap();
        for (name, bytes) in entries {
            writer.start_file(*name, stored).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn mark_entry_encrypted(path: &Path, target: &[u8]) {
        let mut bytes = std::fs::read(path).unwrap();
        let mut marked = 0;
        for offset in 0..=bytes.len().saturating_sub(4) {
            let (flag_offset, name_len_offset, extra_len_offset, header_len) =
                if &bytes[offset..offset + 4] == b"PK\x03\x04" {
                    (offset + 6, offset + 26, offset + 28, 30)
                } else if &bytes[offset..offset + 4] == b"PK\x01\x02" {
                    (offset + 8, offset + 28, offset + 30, 46)
                } else {
                    continue;
                };
            if extra_len_offset + 2 > bytes.len() || flag_offset >= bytes.len() {
                continue;
            }
            let name_len =
                u16::from_le_bytes([bytes[name_len_offset], bytes[name_len_offset + 1]]) as usize;
            let extra_len =
                u16::from_le_bytes([bytes[extra_len_offset], bytes[extra_len_offset + 1]]) as usize;
            let name_start = offset + header_len;
            if name_start + name_len + extra_len > bytes.len() {
                continue;
            }
            if &bytes[name_start..name_start + name_len] == target {
                bytes[flag_offset] |= 0x01;
                marked += 1;
            }
        }
        assert_eq!(
            marked, 2,
            "encrypted fixture must mark local and central headers"
        );
        std::fs::write(path, bytes).unwrap();
    }

    fn valid_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/content.opf",
                br#"<package><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
            ),
            ("OEBPS/chapter.xhtml", b"<html><body>content</body></html>"),
        ]
    }

    #[test]
    fn accepts_bounded_epub_with_spine_document() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path, &valid_entries());
        validate_epub_file(&path).unwrap();
    }

    #[test]
    fn accepts_spine_document_with_media_type_parameters() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("book-with-mime-parameters.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml; charset=utf-8\"/></manifest><spine><itemref idref=\"chapter\"/></spine></package>".as_slice(),
            ),
            ("OEBPS/chapter.xhtml", b"<html><body>content</body></html>".as_slice()),
        ];
        write_epub(&path, &entries);
        validate_epub_file(&path).unwrap();
    }

    #[test]
    fn rejects_non_zip_disguised_as_epub() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("book.epub");
        std::fs::write(&path, b"not an epub").unwrap();
        assert_eq!(
            validate_epub_file(&path).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn rejects_missing_or_compressed_mimetype() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.epub");
        let file = File::create(&missing).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("META-INF/container.xml", stored).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            validate_epub_file(&missing).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );

        let compressed = dir.path().join("compressed.epub");
        let file = File::create(&compressed).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("mimetype", deflated).unwrap();
        writer.write_all(EPUB_MIMETYPE).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            validate_epub_file(&compressed).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn rejects_unsafe_archive_names_and_missing_spine_document() {
        let dir = TempDir::new().unwrap();
        let unsafe_path = dir.path().join("unsafe.epub");
        let entries = [(
            "../container.xml",
            b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
        )];
        write_epub(&unsafe_path, &entries);
        assert_eq!(
            validate_epub_file(&unsafe_path)
                .unwrap_err()
                .code()
                .as_str(),
            "SECURITY_POLICY_DENIED"
        );

        let no_spine = dir.path().join("no-spine.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest></manifest><spine></spine></package>".as_slice(),
            ),
        ];
        write_epub(&no_spine, &entries);
        assert_eq!(
            validate_epub_file(&no_spine).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn rejects_any_invalid_spine_itemref_even_when_another_document_is_valid() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invalid-spine.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"chapter\"/><itemref idref=\"missing\"/></spine></package>".as_slice(),
            ),
            ("OEBPS/chapter.xhtml", b"<html/>".as_slice()),
        ];
        write_epub(&path, &entries);
        assert_eq!(
            validate_epub_file(&path).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn rejects_nested_archive_and_zip_bomb_ratio() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("nested.epub");
        let entries = [("META-INF/container.xml", b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice()),
            ("OEBPS/content.opf", b"<package><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"chapter\"/></spine></package>".as_slice()),
            ("OEBPS/chapter.xhtml", b"<html/>".as_slice()),
            ("OEBPS/inner.zip", b"inner archive".as_slice())];
        write_epub(&nested, &entries);
        assert_eq!(
            validate_epub_file(&nested).unwrap_err().code().as_str(),
            "SECURITY_POLICY_DENIED"
        );

        let bomb = dir.path().join("bomb.epub");
        let file = File::create(&bomb).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("mimetype", stored).unwrap();
        writer.write_all(EPUB_MIMETYPE).unwrap();
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer
            .start_file("META-INF/container.xml", deflated)
            .unwrap();
        writer
            .write_all(b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>")
            .unwrap();
        writer.start_file("OEBPS/content.opf", deflated).unwrap();
        writer.write_all(b"<package><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"chapter\"/></spine></package>").unwrap();
        writer.start_file("OEBPS/chapter.xhtml", deflated).unwrap();
        writer.write_all(&vec![b'a'; 20 * 1024]).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            validate_epub_file(&bomb).unwrap_err().code().as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }

    #[test]
    fn rejects_encrypted_entries_without_reading_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("encrypted.epub");
        write_epub(&path, &valid_entries());
        mark_entry_encrypted(&path, b"OEBPS/content.opf");
        assert_eq!(
            validate_epub_file(&path).unwrap_err().code().as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }

    // ---- Reader TOC extraction ----

    fn toc_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            (
                "META-INF/container.xml",
                br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/content.opf",
                br#"<package><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
            ),
            (
                "OEBPS/nav.xhtml",
                "<html xmlns:epub=\"http://www.idpf.org/2007/ops\"><body><nav epub:type=\"toc\"><h1>目录</h1><ol><li><a href=\"chapter1.xhtml\">第一章</a></li><li><a href=\"chapter2.xhtml\">第二章 &amp; 番外</a><ol><li><a href=\"chapter2.xhtml#s1\">第一节</a></li></ol></li></ol></nav></body></html>".as_bytes(),
            ),
            ("OEBPS/chapter1.xhtml", b"<html><body>one</body></html>"),
            ("OEBPS/chapter2.xhtml", b"<html><body>two</body></html>"),
        ]
    }

    #[test]
    fn toc_extracts_nav3_tree_with_entity_titles_and_fragments() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nav3.epub");
        write_epub(&path, &toc_entries());
        let toc = extract_epub_toc(&path).unwrap();
        assert_eq!(
            toc.spine,
            vec!["OEBPS/chapter1.xhtml", "OEBPS/chapter2.xhtml"]
        );
        assert_eq!(toc.nodes.len(), 3);
        assert_eq!(toc.nodes[0].href, "OEBPS/chapter1.xhtml");
        assert_eq!(toc.nodes[0].fragment, None);
        assert_eq!(toc.nodes[0].title, "第一章");
        assert_eq!(toc.nodes[0].depth, 0);
        assert_eq!(toc.nodes[1].title, "第二章 & 番外");
        assert_eq!(toc.nodes[1].depth, 0);
        assert_eq!(toc.nodes[2].href, "OEBPS/chapter2.xhtml");
        assert_eq!(toc.nodes[2].fragment.as_deref(), Some("s1"));
        assert_eq!(toc.nodes[2].depth, 1);
    }

    #[test]
    fn toc_falls_back_to_ncx_when_nav_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ncx.epub");
        let entries = vec![
            (
                "META-INF/container.xml",
                br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.as_slice(),
            ),
            (
                "OEBPS/content.opf",
                br#"<package><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#.as_slice(),
            ),
            (
                "OEBPS/toc.ncx",
                "<ncx><navMap><navPoint id=\"n1\"><navLabel><text>第一章</text></navLabel><content src=\"chapter1.xhtml\"/></navPoint><navPoint id=\"n2\"><navLabel><text>第二章</text></navLabel><content src=\"chapter2.xhtml\"/><navPoint id=\"n2s\"><navLabel><text>第一节</text></navLabel><content src=\"chapter2.xhtml#s1\"/></navPoint></navPoint></navMap></ncx>".as_bytes(),
            ),
            ("OEBPS/chapter1.xhtml", b"<html><body>one</body></html>".as_slice()),
            ("OEBPS/chapter2.xhtml", b"<html><body>two</body></html>".as_slice()),
        ];
        write_epub(&path, &entries);
        let toc = extract_epub_toc(&path).unwrap();
        assert_eq!(toc.nodes.len(), 3);
        assert_eq!(toc.nodes[0].title, "第一章");
        assert_eq!(toc.nodes[0].depth, 0);
        assert_eq!(toc.nodes[2].title, "第一节");
        assert_eq!(toc.nodes[2].fragment.as_deref(), Some("s1"));
        assert_eq!(toc.nodes[2].depth, 1);
    }

    #[test]
    fn toc_decodes_percent_encoded_manifest_documents_before_opening() {
        let dir = TempDir::new().unwrap();

        let nav_path = dir.path().join("encoded-nav.epub");
        let nav_entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>"
                    .as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"nav\" href=\"nav%20file.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"c1\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>".as_slice(),
            ),
            (
                "OEBPS/nav file.xhtml",
                "<html xmlns:epub=\"http://www.idpf.org/2007/ops\"><body><nav epub:type=\"toc\"><ol><li><a href=\"chapter.xhtml\">编码 nav</a></li></ol></nav></body></html>".as_bytes(),
            ),
            ("OEBPS/chapter.xhtml", b"<html/>".as_slice()),
        ];
        write_epub(&nav_path, &nav_entries);
        let nav_toc = extract_epub_toc(&nav_path).unwrap();
        assert_eq!(nav_toc.nodes.len(), 1);
        assert_eq!(nav_toc.nodes[0].title, "编码 nav");

        let ncx_path = dir.path().join("encoded-ncx.epub");
        let ncx_entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>"
                    .as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"ncx\" href=\"toc%20file.ncx\" media-type=\"application/x-dtbncx+xml\"/><item id=\"c1\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>".as_slice(),
            ),
            (
                "OEBPS/toc file.ncx",
                "<ncx><navMap><navPoint><navLabel><text>编码 ncx</text></navLabel><content src=\"chapter.xhtml\"/></navPoint></navMap></ncx>".as_bytes(),
            ),
            ("OEBPS/chapter.xhtml", b"<html/>".as_slice()),
        ];
        write_epub(&ncx_path, &ncx_entries);
        let ncx_toc = extract_epub_toc(&ncx_path).unwrap();
        assert_eq!(ncx_toc.nodes.len(), 1);
        assert_eq!(ncx_toc.nodes[0].title, "编码 ncx");
    }

    #[test]
    fn toc_rejects_encoded_query_in_manifest_document_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("unsafe-manifest.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>"
                    .as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"nav\" href=\"nav%3Ftoken.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"c1\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>".as_slice(),
            ),
            ("OEBPS/chapter.xhtml", b"<html/>".as_slice()),
        ];
        write_epub(&path, &entries);
        assert_eq!(
            extract_epub_toc(&path).unwrap_err().code().as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }

    #[test]
    fn toc_spine_fallback_is_flat_numbered() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-toc.epub");
        write_epub(&path, &valid_entries());
        let toc = extract_epub_toc(&path).unwrap();
        assert_eq!(toc.spine, vec!["OEBPS/chapter.xhtml"]);
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "第 1 章");
        assert_eq!(toc.nodes[0].depth, 0);
    }

    #[test]
    fn toc_percent_decodes_and_rejects_traversal_in_nav_hrefs() {
        let dir = TempDir::new().unwrap();
        let encoded = dir.path().join("encoded.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"c1\" href=\"chapter1.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>".as_slice(),
            ),
            (
                "OEBPS/nav.xhtml",
                "<html><body><nav epub:type=\"toc\"><ol><li><a href=\"chapter%201.xhtml\">编码章节</a></li><li><a href=\"%2e%2e/escape.xhtml\">逃逸</a></li></ol></nav></body></html>".as_bytes(),
            ),
            ("OEBPS/chapter1.xhtml", b"<html/>".as_slice()),
        ];
        write_epub(&encoded, &entries);
        // 编码路径解码后必须能匹配 spine；traversal 引用被整条丢弃。
        let toc = extract_epub_toc(&encoded).unwrap();
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "编码章节");

        let evil = dir.path().join("evil.epub");
        let entries = [
            (
                "META-INF/container.xml",
                b"<container><rootfile full-path=\"OEBPS/content.opf\"/></container>".as_slice(),
            ),
            (
                "OEBPS/content.opf",
                b"<package><manifest><item id=\"c1\" href=\"%2e%2e/escape.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c1\"/></spine></package>".as_slice(),
            ),
        ];
        write_epub(&evil, &entries);
        assert_eq!(
            extract_epub_toc(&evil).unwrap_err().code().as_str(),
            "SECURITY_POLICY_DENIED",
            "spine 解码后越界必须整文件拒绝"
        );
    }

    #[test]
    fn toc_provider_rejects_non_epub_sessions() {
        use haven_domain::enums::MediaType;
        use haven_domain::ids::{ResourceId, StorageLocationId};

        let provider = LocalEpubTocProvider::new();
        let session = PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            engine: haven_application::wire::SessionEngineDto::Reader,
            resource_id: ResourceId::new(),
            storage_location_id: Some(StorageLocationId::new()),
            canonical_root: Some(std::path::PathBuf::from("/root")),
            canonical_file: Some(std::path::PathBuf::from("/root/book.txt")),
            subtitle_tracks: Vec::new(),
            source: haven_application::services::PreparedSessionSource::Local,
            mime_type: Some("text/plain".into()),
            media_type: MediaType::Book,
            resource_type: haven_domain::enums::ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        };
        assert_eq!(
            provider.extract(&session).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn toc_provider_accepts_epub_mime_parameters() {
        use haven_domain::enums::MediaType;
        use haven_domain::ids::{ResourceId, StorageLocationId};

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path, &valid_entries());
        let provider = LocalEpubTocProvider::new();
        let session = PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            engine: haven_application::wire::SessionEngineDto::Reader,
            resource_id: ResourceId::new(),
            storage_location_id: Some(StorageLocationId::new()),
            canonical_root: Some(std::fs::canonicalize(dir.path()).unwrap()),
            canonical_file: Some(std::fs::canonicalize(path).unwrap()),
            subtitle_tracks: Vec::new(),
            source: haven_application::services::PreparedSessionSource::Local,
            mime_type: Some("application/epub+zip; charset=binary".into()),
            media_type: MediaType::Book,
            resource_type: haven_domain::enums::ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        };

        let toc = provider.extract(&session).unwrap();
        assert_eq!(toc.spine, vec!["OEBPS/chapter.xhtml"]);
    }

    #[test]
    fn toc_120_pages_stability() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("120.epub");
        let mut entries: Vec<(&str, &[u8])> = Vec::new();
        entries.push((
            "META-INF/container.xml",
            br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        ));
        let mut manifest = String::from("<package><manifest>");
        let mut spine = String::from("<spine>");
        for i in 0..120 {
            manifest.push_str(&format!(
                r#"<item id="c{i}" href="chapter{i}.xhtml" media-type="application/xhtml+xml"/>"#
            ));
            spine.push_str(&format!(r#"<itemref idref="c{i}"/>"#));
        }
        manifest.push_str("</manifest>");
        spine.push_str("</spine>");
        let opf = format!("<package>{manifest}{spine}</package>");
        entries.push(("OEBPS/content.opf", opf.as_bytes()));
        for i in 0..120 {
            let html = format!(
                "<html><body><p>Chapter {i} content {}</p></body></html>",
                "x".repeat(100)
            );
            // Leak to satisfy 'static lifetime for test helper
            let leaked: &'static [u8] = Box::leak(html.into_bytes().into_boxed_slice());
            entries.push((
                Box::leak(format!("OEBPS/chapter{i}.xhtml").into_boxed_str()) as &str,
                leaked,
            ));
        }
        // Use a custom write that handles owned data
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stored =
            zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("mimetype", stored).unwrap();
        writer.write_all(EPUB_MIMETYPE).unwrap();
        for (name, bytes) in entries {
            writer.start_file(name, stored).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        validate_epub_file(&path).unwrap();
        let toc = extract_epub_toc(&path).unwrap();
        assert_eq!(toc.spine.len(), 120);
        assert_eq!(toc.nodes.len(), 120);
        // Also verify reader_search can extract 120 chapters without hitting 24MiB limit
        let toc2 = extract_epub_toc(&path).unwrap();
        assert_eq!(toc2.spine.len(), 120);
    }
}
