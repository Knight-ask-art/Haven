use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path};

use haven_application::services::PreparedSession;
use haven_application::services::reader_search::{
    RawBookContent, RawChapter, ReaderSearchProvider,
};
use haven_common::{AppError, ErrorKind};
use zip::{ZipArchive, ZipReadOptions};

const MAX_EPUB_ENTRIES: usize = 10_000;
const MAX_EPUB_TEXT_BYTES: usize = 24 * 1024 * 1024;
const MAX_EPUB_CHAPTER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct LocalReaderSearchProvider;

impl LocalReaderSearchProvider {
    pub fn new() -> Self {
        Self
    }
}

impl ReaderSearchProvider for LocalReaderSearchProvider {
    fn extract(&self, session: &PreparedSession) -> Result<RawBookContent, AppError> {
        let mime = base_mime(session.mime_type.as_deref().unwrap_or(""));
        if mime == "application/epub+zip" {
            let source = crate::comic::revalidate_source_with_message(
                session,
                false,
                "阅读资源路径校验失败",
            )?;
            extract_epub_content(&source)
        } else if mime == "text/plain" || mime == "text/markdown" || mime.is_empty() {
            let source = crate::comic::revalidate_source_with_message(
                session,
                false,
                "阅读资源路径校验失败",
            )?;
            extract_text_content(&source, mime)
        } else {
            Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前格式不支持全文检索",
                false,
            ))
        }
    }
}

fn extract_text_content(path: &Path, mime: &str) -> Result<RawBookContent, AppError> {
    let bytes = std::fs::read(path).map_err(|_| resource_unavailable())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(AppError::new(
            "FORMAT_UNSUPPORTED",
            ErrorKind::Unsupported,
            "文本文件过大",
            false,
        ));
    }
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
    if text.contains('\0') {
        return Err(AppError::new(
            "FORMAT_UNSUPPORTED",
            ErrorKind::Unsupported,
            "文本包含非法字符",
            false,
        ));
    }
    let format = if mime == "text/markdown" {
        "markdown"
    } else {
        "text"
    };
    let chapters = parse_book_text(&text, format);
    Ok(RawBookContent { chapters })
}

/// MIME values from filesystem probes and HTTP responses may carry optional
/// parameters (for example `text/plain; charset=utf-8`).  Reader format
/// dispatch only depends on the media type token, so normalize it once at the
/// boundary instead of requiring every caller to emit a parameter-free value.
fn base_mime(value: &str) -> &str {
    value.split(';').next().unwrap_or("").trim()
}

fn parse_book_text(text: &str, _format: &str) -> Vec<RawChapter> {
    let mut chapters: Vec<(String, Vec<String>)> = Vec::new();
    let mut title = "全文".to_string();
    let mut paragraphs: Vec<String> = Vec::new();
    let mut paragraph_lines: Vec<String> = Vec::new();
    let flush_paragraph = |paragraph_lines: &mut Vec<String>, paragraphs: &mut Vec<String>| {
        if paragraph_lines.is_empty() {
            return;
        }
        paragraphs.push(paragraph_lines.join(" "));
        paragraph_lines.clear();
    };
    let commit = |title: &mut String,
                  paragraphs: &mut Vec<String>,
                  chapters: &mut Vec<(String, Vec<String>)>| {
        if paragraphs.is_empty() && title == "全文" {
            return;
        }
        chapters.push((
            std::mem::replace(title, "全文".to_string()),
            std::mem::take(paragraphs),
        ));
    };
    for raw_line in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let line = raw_line.trim();
        if line.is_empty() {
            flush_paragraph(&mut paragraph_lines, &mut paragraphs);
            continue;
        }
        if let Some(heading) = chapter_title(line) {
            flush_paragraph(&mut paragraph_lines, &mut paragraphs);
            commit(&mut title, &mut paragraphs, &mut chapters);
            title = heading;
            continue;
        }
        paragraph_lines.push(line.to_string());
    }
    flush_paragraph(&mut paragraph_lines, &mut paragraphs);
    commit(&mut title, &mut paragraphs, &mut chapters);
    chapters
        .into_iter()
        .enumerate()
        .map(|(idx, (t, p))| RawChapter {
            id: format!("chapter-{}", idx + 1),
            title: t,
            paragraphs: p,
        })
        .collect()
}

fn chapter_title(line: &str) -> Option<String> {
    if line.len() > 160 {
        return None;
    }
    if let Some(caps) = line.strip_prefix('#') {
        let trimmed = caps.trim_start_matches('#').trim();
        if !trimmed.is_empty() && caps.starts_with(' ') || caps.starts_with('#') {
            return Some(trimmed.to_string());
        }
        // handle markdown heading like "## Title"
        let re_chars = line.chars().collect::<Vec<_>>();
        if re_chars[0] == '#' {
            let mut i = 0;
            while i < re_chars.len() && re_chars[i] == '#' && i < 6 {
                i += 1;
            }
            if i < re_chars.len() && re_chars[i] == ' ' {
                return Some(
                    re_chars[i + 1..]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_string(),
                );
            }
        }
    }
    // Simplified check for Chinese chapter patterns
    if line.starts_with("第") && (line.contains('章') || line.contains('节')) {
        return Some(line.to_string());
    }
    if ["序章", "楔子", "尾声", "后记", "前言", "引言"]
        .iter()
        .any(|p| line.starts_with(p))
    {
        return Some(line.to_string());
    }
    let lower = line.to_lowercase();
    if lower.starts_with("chapter") || lower.starts_with("part") || lower.starts_with("book") {
        return Some(line.to_string());
    }
    None
}

fn extract_epub_content(path: &Path) -> Result<RawBookContent, AppError> {
    let file = File::open(path).map_err(|_| resource_unavailable())?;
    let mut archive = ZipArchive::new(file).map_err(|_| format_unsupported())?;
    if archive.is_empty() || archive.len() > MAX_EPUB_ENTRIES {
        return Err(policy_denied("EPUB 归档条目数超过安全限制"));
    }
    let mut names = HashSet::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index_with_options(i, ZipReadOptions::new().ignore_encryption_flag(true))
            .map_err(|_| format_unsupported())?;
        let raw = entry.name_raw();
        let name = std::str::from_utf8(raw).map_err(|_| format_unsupported())?;
        let normalized = safe_archive_name(name)?;
        if !names.insert(normalized) {
            return Err(policy_denied("EPUB 归档包含重复条目名称"));
        }
    }
    let container_xml = read_archive_text(&mut archive, &names, "META-INF/container.xml")?;
    let opf_path = parse_container_rootfile(&container_xml)?;
    let opf_xml = read_archive_text(&mut archive, &names, &opf_path)?;
    let opf_dir = opf_path.rsplit_once('/').map(|(d, _)| d);
    let manifest = parse_manifest(&opf_xml);
    let spine_refs = parse_spine(&opf_xml)?;
    let mut chapters = Vec::new();
    let mut total_text_bytes = 0usize;
    for (idx, idref) in spine_refs.iter().enumerate() {
        let Some(item) = manifest.get(idref) else {
            return Err(format_unsupported());
        };
        if item.1 != "application/xhtml+xml" && item.1 != "text/html" {
            return Err(format_unsupported());
        }
        let doc_path = resolve_epub_href(opf_dir, &item.0)?;
        let bytes = read_archive_bytes(&mut archive, &names, &doc_path)?;
        total_text_bytes += bytes.len();
        if total_text_bytes > MAX_EPUB_TEXT_BYTES {
            return Err(policy_denied("EPUB 正文总大小超过安全限制"));
        }
        let html = String::from_utf8(bytes).map_err(|_| format_unsupported())?;
        // Crop Project Gutenberg header/footer boilerplate exactly like the
        // frontend reader (`trimGutenbergBoilerplate`), then derive both the
        // plain text and the title from the surviving document. Otherwise the
        // paragraph indexes computed here would address different paragraphs
        // than the ones the reader renders.
        let trimmed = trim_gutenberg_boilerplate(&html);
        let plain = html_to_plain_text(trimmed);
        if plain.trim().is_empty() {
            continue;
        }
        let mut paragraphs: Vec<String> = plain
            .split("\n\n")
            .map(|p| p.replace('\n', " ").trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if paragraphs.is_empty() {
            continue;
        }
        let title = extract_title(trimmed).unwrap_or_else(|| format!("第 {} 章", idx + 1));
        // The frontend reader drops the leading heading paragraph when it is
        // the chapter title (`chapterFromXhtml`). Mirror that shape so search
        // paragraph indexes address the same DOM nodes, without offset hacks
        // in the caller.
        if paragraphs.first().is_some_and(|first| first == &title) {
            paragraphs.remove(0);
        }
        if paragraphs.is_empty() {
            continue;
        }
        chapters.push(RawChapter {
            id: format!("epub-chapter-{}", idx + 1),
            title,
            paragraphs,
        });
    }
    Ok(RawBookContent { chapters })
}

/// Crop Project Gutenberg front/back matter the same way the frontend reader
/// does. Only the genuine `*** START OF … ***` / `*** END OF THE PROJECT
/// GUTENBERG EBOOK …` markers crop a document: prose that merely mentions the
/// Project Gutenberg name stays searchable, and a document is skipped only
/// when cropping leaves no text at all. Markers are usually wrapped in an HTML
/// comment, so the enclosing comment bounds are resolved before slicing to
/// avoid leaving an unterminated comment (or a stray `-->`) behind.
fn trim_gutenberg_boilerplate(html: &str) -> &str {
    let mut body = html;
    if let Some((marker_start, marker_end)) = find_pg_start_marker(body) {
        let cut = match enclosing_comment_end(body, marker_start) {
            Some(comment_end) if comment_end > marker_end => comment_end,
            _ => marker_end,
        };
        body = &body[cut..];
    }
    if let Some(marker_start) = find_pg_end_marker(body) {
        let cut = enclosing_comment_start(body, marker_start).unwrap_or(marker_start);
        body = &body[..cut];
    }
    body
}

/// `\*{3}\s*START OF (?:THE|THIS) PROJECT GUTENBERG EBOOK\b[^\r\n]{0,200}?\*{3}`
/// Returns the byte range of the first genuine marker. Matching runs on an
/// ASCII-lowercased copy, which keeps byte offsets identical to `html`.
fn find_pg_start_marker(html: &str) -> Option<(usize, usize)> {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(relative) = lower[search..].find("***") {
        let stars = search + relative;
        let after_stars = skip_whitespace(&lower, stars + 3);
        for phrase in [
            "start of the project gutenberg ebook",
            "start of this project gutenberg ebook",
        ] {
            let after_phrase = after_stars + phrase.len();
            if lower[after_stars..].starts_with(phrase) && is_word_boundary(&lower, after_phrase) {
                if let Some(end) = find_marker_close(&lower, after_phrase) {
                    return Some((stars, end));
                }
            }
        }
        search = stars + 3;
    }
    None
}

/// `\*{3}\s*END OF THE PROJECT GUTENBERG EBOOK` — returns the marker start.
fn find_pg_end_marker(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(relative) = lower[search..].find("***") {
        let stars = search + relative;
        let after_stars = skip_whitespace(&lower, stars + 3);
        if lower[after_stars..].starts_with("end of the project gutenberg ebook") {
            return Some(stars);
        }
        search = stars + 3;
    }
    None
}

/// `[^\r\n]{0,200}?\*{3}` — the earliest closing marker on the same line.
fn find_marker_close(text: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    let mut consumed = 0usize;
    while consumed <= 200 {
        if text[cursor..].starts_with("***") {
            return Some(cursor + 3);
        }
        let character = text[cursor..].chars().next()?;
        if character == '\r' || character == '\n' {
            return None;
        }
        cursor += character.len_utf8();
        consumed += 1;
    }
    None
}

fn skip_whitespace(text: &str, mut cursor: usize) -> usize {
    while let Some(character) = text[cursor..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

/// `\b` for the ASCII markers: the next character must not continue a word.
fn is_word_boundary(text: &str, index: usize) -> bool {
    match text[index..].chars().next() {
        Some(character) => !(character.is_ascii_alphanumeric() || character == '_'),
        None => true,
    }
}

/// Byte offset just past the comment enclosing `index`, when there is one.
fn enclosing_comment_end(html: &str, index: usize) -> Option<usize> {
    enclosing_comment_start(html, index)?;
    let comment_end = html[index..].find("-->")?;
    Some(index + comment_end + 3)
}

/// Byte offset of the `<!--` enclosing `index`, when `index` is inside a
/// comment rather than after a completed one.
fn enclosing_comment_start(html: &str, index: usize) -> Option<usize> {
    let comment_start = html[..index].rfind("<!--")?;
    if html[..index].rfind("-->").is_some_and(|end| end > comment_start) {
        return None;
    }
    Some(comment_start)
}

/// Convert untrusted XHTML to the same paragraph-preserving plain-text shape
/// used by the frontend EPUB parser.  In particular, block boundaries remain
/// as newlines until `extract_epub_content` splits them into paragraphs.  A
/// whitespace-only `split_whitespace()` would collapse every chapter into one
/// paragraph and make `paragraph_index` useless for search navigation.
fn html_to_plain_text(html: &str) -> String {
    const ACTIVE_TAGS: &[&str] = &[
        "head", "script", "style", "iframe", "object", "embed", "svg", "math", "form", "textarea",
    ];
    const BOUNDARY_TAGS: &[&str] = &[
        "br",
        "p",
        "div",
        "section",
        "article",
        "header",
        "footer",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "blockquote",
        "pre",
        "tr",
        "td",
        "th",
    ];

    let mut result = String::with_capacity(html.len());
    let mut cursor = 0usize;
    let mut skipped_tag: Option<String> = None;

    while cursor < html.len() {
        let Some(relative_start) = html[cursor..].find('<') else {
            if skipped_tag.is_none() {
                result.push_str(&html[cursor..]);
            }
            break;
        };
        let tag_start = cursor + relative_start;
        if skipped_tag.is_none() {
            result.push_str(&html[cursor..tag_start]);
        }

        // Comments are never content.  If a malformed comment has no end,
        // discard the remainder rather than exposing active markup.
        if html[tag_start..].starts_with("<!--") {
            if let Some(end) = html[tag_start + 4..].find("-->") {
                cursor = tag_start + 4 + end + 3;
                continue;
            }
            break;
        }

        let Some(tag_end) = find_html_tag_end(html, tag_start + 1) else {
            if skipped_tag.is_none() {
                result.push_str(&html[tag_start..]);
            }
            break;
        };
        let raw_tag = html[tag_start + 1..tag_end].trim();
        let is_closing = raw_tag.starts_with('/');
        let tag_body = raw_tag.strip_prefix('/').unwrap_or(raw_tag).trim_start();
        let tag_name = tag_body
            .split(|ch: char| ch.is_ascii_whitespace() || ch == '/' || ch == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let self_closing = raw_tag.ends_with('/');

        if let Some(active) = skipped_tag.as_deref() {
            if is_closing && tag_name == active {
                skipped_tag = None;
            }
            cursor = tag_end + 1;
            continue;
        }

        if ACTIVE_TAGS.iter().any(|candidate| *candidate == tag_name) {
            if !is_closing && !self_closing {
                skipped_tag = Some(tag_name);
            }
            cursor = tag_end + 1;
            continue;
        }

        if BOUNDARY_TAGS.iter().any(|candidate| *candidate == tag_name) {
            result.push('\n');
        }
        cursor = tag_end + 1;
    }

    normalize_html_text(&decode_html_entities(&result))
}

fn find_html_tag_end(html: &str, mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < html.len() {
        let ch = html.as_bytes()[cursor] as char;
        if let Some(expected) = quote {
            if ch == expected {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn decode_html_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '&' {
            let mut end = index + 1;
            while end < chars.len() && end - index <= 32 && chars[end] != ';' {
                end += 1;
            }
            if end < chars.len() && chars[end] == ';' {
                let entity: String = chars[index + 1..end].iter().collect();
                let decoded = match entity.to_ascii_lowercase().as_str() {
                    "nbsp" => Some(' '),
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    index = end + 1;
                    continue;
                }
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn normalize_html_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut newline_count = 0usize;
    for ch in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
        match ch {
            '\n' => {
                while normalized.ends_with([' ', '\t']) {
                    normalized.pop();
                }
                pending_space = false;
                if newline_count < 2 {
                    normalized.push('\n');
                    newline_count += 1;
                }
            }
            ' ' | '\t' => pending_space = true,
            _ => {
                if pending_space && !normalized.is_empty() && !normalized.ends_with('\n') {
                    normalized.push(' ');
                }
                pending_space = false;
                normalized.push(ch);
                newline_count = 0;
            }
        }
    }
    normalized.trim().to_string()
}

fn extract_title(html: &str) -> Option<String> {
    ["h1", "h2", "h3", "title"]
        .iter()
        .find_map(|tag| extract_tag_text(html, tag))
}

/// Mirrors the frontend `<h1\b[^>]*>…</h1>` scan: the opening tag must end at a
/// real tag-name boundary, so `<h10>` is not mistaken for `<h1>`. As in the
/// frontend, the first boundary-valid opening tag decides that tag name; an
/// empty candidate falls through to the next tag in h1 → h2 → h3 → title order.
fn extract_tag_text(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let opening = format!("<{tag}");
    let closing = format!("</{tag}>");
    let mut cursor = 0usize;
    while let Some(relative) = lower[cursor..].find(&opening) {
        let after_name = cursor + relative + opening.len();
        cursor = after_name;
        let boundary = lower[after_name..]
            .chars()
            .next()
            .is_some_and(|character| {
                character.is_ascii_whitespace() || character == '>' || character == '/'
            });
        if !boundary {
            continue;
        }
        let Some(tag_end) = lower[after_name..].find('>') else {
            return None;
        };
        let content_start = after_name + tag_end + 1;
        let Some(content_end) = lower[content_start..].find(&closing) else {
            // An unterminated candidate leaves the regex engine scanning for a
            // later, complete opening/closing pair.
            continue;
        };
        let plain = html_to_plain_text(&html[content_start..content_start + content_end]);
        let first_line = plain.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            return None;
        }
        return Some(first_line.chars().take(160).collect());
    }
    None
}

fn parse_manifest(xml: &str) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    for attrs in find_tag_attributes(xml, "item") {
        if let (Some(id), Some(href), Some(media)) = (
            attrs.get("id").cloned(),
            attrs.get("href").cloned(),
            attrs.get("media-type").cloned(),
        ) {
            map.insert(id, (href, base_mime(&media).to_ascii_lowercase()));
        }
    }
    map
}

fn parse_spine(xml: &str) -> Result<Vec<String>, AppError> {
    let mut refs = Vec::new();
    for attrs in find_tag_attributes(xml, "itemref") {
        if let Some(idref) = attrs.get("idref").cloned() {
            if idref.is_empty() {
                return Err(format_unsupported());
            }
            refs.push(idref);
        } else {
            return Err(format_unsupported());
        }
    }
    if refs.is_empty() {
        return Err(format_unsupported());
    }
    Ok(refs)
}

fn read_archive_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<String>,
    name: &str,
) -> Result<String, AppError> {
    if !names.contains(name) {
        return Err(format_unsupported());
    }
    let mut entry = archive.by_name(name).map_err(|_| format_unsupported())?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|_| format_unsupported())?;
    String::from_utf8(bytes).map_err(|_| format_unsupported())
}

fn read_archive_bytes<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<String>,
    name: &str,
) -> Result<Vec<u8>, AppError> {
    if !names.contains(name) {
        return Err(format_unsupported());
    }
    let mut entry = archive.by_name(name).map_err(|_| format_unsupported())?;
    if entry.size() > MAX_EPUB_CHAPTER_BYTES as u64 {
        return Err(policy_denied("EPUB 条目超过安全限制"));
    }
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|_| format_unsupported())?;
    Ok(bytes)
}

fn parse_container_rootfile(xml: &str) -> Result<String, AppError> {
    for attrs in find_tag_attributes(xml, "rootfile") {
        if let Some(path) = attrs.get("full-path") {
            if !path.is_empty() {
                return safe_archive_name(path);
            }
        }
    }
    Err(format_unsupported())
}

fn resolve_epub_href(directory: Option<&str>, href: &str) -> Result<String, AppError> {
    let href = href.split_once('#').map_or(href, |(p, _)| p);
    if href.is_empty()
        || href.starts_with('/')
        || href.contains(['\\', '\0', ':', '?'])
        || href.contains("://")
    {
        return Err(policy_denied("EPUB 文档引用路径不安全"));
    }
    let combined = directory.map_or_else(|| href.to_owned(), |d| format!("{d}/{href}"));
    safe_archive_name(&combined)
}

fn safe_archive_name(name: &str) -> Result<String, AppError> {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains(['\\', ':', '\0']) {
        return Err(policy_denied("EPUB 归档条目路径不安全"));
    }
    let path = Path::new(name);
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(p) => {
                let s = p.to_str().ok_or_else(format_unsupported)?.trim();
                if s.is_empty() {
                    return Err(policy_denied("EPUB 归档条目路径不安全"));
                }
                parts.push(s.to_owned());
            }
            _ => return Err(policy_denied("EPUB 归档条目路径不安全")),
        }
    }
    if parts.is_empty() {
        return Err(policy_denied("EPUB 归档条目路径不安全"));
    }
    Ok(parts.join("/"))
}

fn find_tag_attributes(xml: &str, wanted: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(rel) = xml[cursor..].find('<') {
        let start = cursor + rel;
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
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(inner.len());
        if inner[..tag_end].eq_ignore_ascii_case(wanted) {
            out.push(parse_attributes(&inner[tag_end..]));
        }
        cursor = end + 1;
    }
    out
}

fn find_tag_end(xml: &str, mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < xml.len() {
        let ch = xml.as_bytes()[cursor] as char;
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn parse_attributes(mut s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    while !s.trim_start().is_empty() {
        s = s.trim_start().trim_start_matches('/').trim_start();
        if s.is_empty() {
            break;
        }
        let key_end = s
            .find(|c: char| c.is_ascii_whitespace() || c == '=')
            .unwrap_or(s.len());
        if key_end == 0 {
            break;
        }
        let key = s[..key_end].to_ascii_lowercase();
        s = s[key_end..].trim_start();
        if !s.starts_with('=') {
            break;
        }
        s = s[1..].trim_start();
        let Some(q) = s.chars().next() else { break };
        if q != '\'' && q != '"' {
            break;
        }
        s = &s[q.len_utf8()..];
        let Some(end) = s.find(q) else { break };
        map.insert(key, s[..end].to_owned());
        s = &s[end + q.len_utf8()..];
    }
    map
}

fn format_unsupported() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "EPUB 文件损坏或不受支持",
        false,
    )
}

fn policy_denied(msg: &'static str) -> AppError {
    AppError::new("SECURITY_POLICY_DENIED", ErrorKind::Security, msg, false)
}

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "本地资源当前不可用",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::reader_search::{build_book_search_index, search_book};
    use haven_application::services::{PreparedSession, PreparedSessionSource};
    use haven_application::wire::SessionEngineDto;
    use haven_domain::enums::{MediaType, ResourceType};
    use haven_domain::ids::{ResourceId, StorageLocationId};
    use tempfile::TempDir;

    fn local_session(root: &Path, file: &Path, mime: &str) -> PreparedSession {
        PreparedSession {
            work_id: "work".into(),
            edition_id: "edition".into(),
            media_item_id: "media".into(),
            engine: SessionEngineDto::Reader,
            resource_id: ResourceId::new(),
            storage_location_id: Some(StorageLocationId::new()),
            canonical_root: Some(std::fs::canonicalize(root).unwrap()),
            canonical_file: Some(std::fs::canonicalize(file).unwrap()),
            subtitle_tracks: Vec::new(),
            source: PreparedSessionSource::Local,
            mime_type: Some(mime.into()),
            media_type: MediaType::Book,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        }
    }

    #[test]
    fn reader_search_accepts_text_mime_parameters() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "第一章\n\n目标词").unwrap();
        let session = local_session(dir.path(), &file, "text/plain; charset=utf-8");

        let content = LocalReaderSearchProvider::new().extract(&session).unwrap();
        assert_eq!(content.chapters.len(), 1);
        assert_eq!(content.chapters[0].title, "第一章");
        assert_eq!(content.chapters[0].paragraphs[0], "目标词");
    }

    #[test]
    fn html_text_preserves_paragraph_boundaries_for_search() {
        let html = r#"
            <html><head><title>忽略的标题</title></head>
            <body><p>第一段保留换行。</p><p>第二段包含目标词。</p>
            <script>目标词不应进入索引</script><div>第三段&nbsp;也保留。</div></body>
        "#;
        let plain = html_to_plain_text(html);
        assert_eq!(
            plain,
            "第一段保留换行。\n\n第二段包含目标词。\n\n第三段 也保留。"
        );

        let paragraphs = plain
            .split("\n\n")
            .map(|paragraph| paragraph.replace('\n', " ").trim().to_string())
            .filter(|paragraph| !paragraph.is_empty())
            .collect::<Vec<_>>();
        let chapters = vec![RawChapter {
            id: "epub-chapter-1".to_string(),
            title: "测试章节".to_string(),
            paragraphs,
        }];
        let index = build_book_search_index(&chapters);
        let hits = search_book(&chapters, &index, "目标词");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].paragraph_index, 1);
        assert!(hits[0].progression_in_chapter > 0.0);
    }

    #[test]
    fn html_text_removes_active_blocks_and_decodes_entities() {
        let html = r#"<p>A &amp; B &#x4E2D;&#25991;</p><iframe>隐藏</iframe><p>C<br>D</p>"#;
        assert_eq!(html_to_plain_text(html), "A & B 中文\n\nC\nD");
    }
}
