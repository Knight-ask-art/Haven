//! 系统字体枚举与字体文件识别（BE-INTERFACE-FONT-001）。
//!
//! 设计约束：
//! - **只返回族名**：枚举结果、IPC 载荷与设置 JSON 都不携带文件路径，WebView 也
//!   永远拿不到路径。选择结果用族名交给 CSS `font-family`，由系统字体栈解析。
//! - **不引入字体解析依赖**：只读 SFNT `name` 表（TTF/OTF/TTC 的族名来源），
//!   解析失败时降级为「跳过该文件」而不是报错，保证一台机器上的个别坏字体不会
//!   让整个字体列表不可用。
//! - **导入边界三重校验**：扩展名（闭合集合）+ 文件签名 + 大小上限。三者必须
//!   自洽，否则拒绝导入；识别族名失败时退化为文件名（仅展示用）。
//! - 目录枚举失败（不存在/无权限）视为空来源；只有全部目录都无法访问时才返回空列表，
//!   由上层渲染成「未找到字体」而不是错误页。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    FontFileInspector, InspectedFont, SystemFontCatalog, SystemFontFamily,
};

/// 单个字体文件大小上限（与 `resource_protocol::MAX_RESPONSE_BYTES` 保持一致）。
pub const MAX_FONT_FILE_BYTES: usize = 32 * 1024 * 1024;

/// 允许导入的扩展名（闭合集合；与迁移 045 的 CHECK 约束一致）。
pub const SUPPORTED_FONT_EXTENSIONS: [&str; 3] = ["ttf", "otf", "woff2"];

/// 族名长度上限（与迁移 045 / Domain 校验一致）。
const MAX_FAMILY_NAME_CHARS: usize = 120;

/// 单次导入去重/枚举时允许扫描的最大文件数，避免异常目录把启动路径拖死。
const MAX_SCANNED_FILES: usize = 4096;

/// 本机字体目录枚举（只读；进程内记忆化）。
pub struct LocalSystemFontCatalog {
    dirs: Vec<PathBuf>,
    cache: Mutex<Option<Vec<SystemFontFamily>>>,
}

impl Default for LocalSystemFontCatalog {
    fn default() -> Self {
        Self::with_dirs(Self::env_dirs())
    }
}

impl LocalSystemFontCatalog {
    /// 使用平台默认字体目录。
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用显式目录（测试与诊断用；`HAVEN_SYSTEM_FONT_DIRS` 也走这里）。
    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        let dirs = if dirs.is_empty() {
            platform_font_dirs()
        } else {
            dirs
        };
        Self {
            dirs,
            cache: Mutex::new(None),
        }
    }

    /// 读取 `HAVEN_SYSTEM_FONT_DIRS`（路径分隔符按平台解析）。
    fn env_dirs() -> Vec<PathBuf> {
        std::env::var_os("HAVEN_SYSTEM_FONT_DIRS")
            .map(|raw| std::env::split_paths(&raw).collect())
            .unwrap_or_default()
    }

    fn scan(&self) -> Vec<SystemFontFamily> {
        // BTreeMap 保证结果按族名稳定排序；同族多文件（如常规/粗体）只登记一次。
        let mut families: BTreeMap<String, SystemFontFamily> = BTreeMap::new();
        let mut scanned = 0usize;
        for dir in &self.dirs {
            for entry in WalkDir::new(dir)
                .max_depth(4)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if scanned >= MAX_SCANNED_FILES {
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if !has_sfnt_extension(path) {
                    continue;
                }
                scanned += 1;
                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                for (family, localized) in parse_font_families(&bytes) {
                    if is_hidden_family(&family) {
                        continue;
                    }
                    match families.get_mut(&family) {
                        Some(existing) => {
                            if existing.localized_family.is_none() {
                                existing.localized_family = localized;
                            }
                        }
                        None => {
                            families.insert(
                                family.clone(),
                                SystemFontFamily {
                                    family,
                                    localized_family: localized,
                                },
                            );
                        }
                    }
                }
            }
        }
        families.into_values().collect()
    }
}

impl SystemFontCatalog for LocalSystemFontCatalog {
    fn list_families(&self) -> Result<Vec<SystemFontFamily>, AppError> {
        if let Ok(guard) = self.cache.lock()
            && let Some(cached) = guard.as_ref()
        {
            return Ok(cached.clone());
        }
        let families = self.scan();
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(families.clone());
        }
        Ok(families)
    }
}

/// 平台默认字体目录（失败/不存在时返回空目录项，枚举自然得到空结果）。
fn platform_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(PathBuf::from(&windir).join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Fonts"));
        }
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        dirs.push(PathBuf::from("/usr/share/X11/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
            dirs.push(PathBuf::from(&home).join(".fonts"));
        }
    }
    dirs
}

fn has_sfnt_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("ttf" | "otf" | "ttc" | "otc")
    )
}

/// macOS 内部字体（`.SF NS` 等）以 `.` 开头，无法可靠地在 CSS 中引用。
fn is_hidden_family(family: &str) -> bool {
    family.starts_with('.')
}

/// 导入字体识别（扩展名 + 签名 + 大小）。
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFontFileInspector;

impl FontFileInspector for LocalFontFileInspector {
    fn inspect(&self, file_name: &str, bytes: &[u8]) -> Result<InspectedFont, AppError> {
        let extension = Path::new(file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        let kind = match extension.as_str() {
            "ttf" => SfntKind::TrueType,
            "otf" => SfntKind::Cff,
            "woff2" => SfntKind::Woff2,
            _ => {
                return Err(AppError::new(
                    "FONT_FORMAT_UNSUPPORTED",
                    ErrorKind::Validation,
                    "只支持 .ttf、.otf 与 .woff2 字体文件",
                    false,
                ));
            }
        };
        if bytes.is_empty() {
            return Err(font_file_invalid("字体文件为空"));
        }
        if bytes.len() > MAX_FONT_FILE_BYTES {
            return Err(font_file_invalid("字体文件超过 32 MiB 上限"));
        }
        if !signature_matches(kind, bytes) {
            return Err(font_file_invalid("字体文件内容与扩展名不匹配"));
        }
        // WOFF2 的字体表是 Brotli 压缩的，这里不引入解压依赖：族名退化为文件名。
        let parsed = match kind {
            SfntKind::Woff2 => None,
            _ => parse_font_families(bytes)
                .into_iter()
                .next()
                .map(|(family, _)| family),
        };
        let family_name = parsed.unwrap_or_else(|| family_from_file_name(file_name));
        Ok(InspectedFont {
            family_name,
            extension,
            mime_type: kind.mime_type().to_owned(),
            sha256: sha256_hex(bytes),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SfntKind {
    TrueType,
    Cff,
    Woff2,
}

impl SfntKind {
    fn mime_type(self) -> &'static str {
        match self {
            Self::TrueType => "font/ttf",
            Self::Cff => "font/otf",
            Self::Woff2 => "font/woff2",
        }
    }
}

fn signature_matches(kind: SfntKind, bytes: &[u8]) -> bool {
    let Some(tag) = tag_at(bytes, 0) else {
        return false;
    };
    match kind {
        SfntKind::TrueType => matches!(&tag, b"\x00\x01\x00\x00" | b"true"),
        SfntKind::Cff => &tag == b"OTTO",
        SfntKind::Woff2 => &tag == b"wOF2",
    }
}

fn font_file_invalid(message: &'static str) -> AppError {
    AppError::new("FONT_FILE_INVALID", ErrorKind::Validation, message, false)
}

/// 内容摘要（小写十六进制）。与 `bytes` 同一次读取，供导入去重与持久化列使用。
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 文件名（去扩展名）退化为展示族名：只用于展示，永不参与路径拼接。
fn family_from_file_name(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("导入字体");
    let cleaned = normalize_family(&stem.replace(['_', '-'], " "));
    if cleaned.is_empty() {
        "导入字体".to_owned()
    } else {
        cleaned
    }
}

/// 去掉控制字符、折叠空白并截断到长度上限。
fn normalize_family(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut length = 0usize;
    for ch in raw.chars() {
        if ch.is_control() {
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        let additional = 1 + usize::from(pending_space);
        if length + additional > MAX_FAMILY_NAME_CHARS {
            break;
        }
        if pending_space {
            out.push(' ');
        }
        out.push(ch);
        length += additional;
        pending_space = false;
    }
    out.trim().to_owned()
}

fn tag_at(bytes: &[u8], offset: usize) -> Option<[u8; 4]> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some([slice[0], slice[1], slice[2], slice[3]])
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// 解析字体里所有 face 的族名（TTC 会有多个 face）。
///
/// 返回 `(family, localized_family)`：`family` 优先英文名，`localized_family` 是同一
/// face 的中文名（存在且与英文不同时才会返回）。
pub fn parse_font_families(bytes: &[u8]) -> Vec<(String, Option<String>)> {
    face_offsets(bytes)
        .into_iter()
        .filter_map(|face| face_family_names(bytes, face))
        .collect()
}

/// 各 face 的 offset table 起始位置（`ttcf` 容器按头部偏移表；单 face 字体为 0）。
fn face_offsets(bytes: &[u8]) -> Vec<usize> {
    let Some(tag) = tag_at(bytes, 0) else {
        return Vec::new();
    };
    if &tag == b"ttcf" {
        let Some(count) = u32_at(bytes, 8) else {
            return Vec::new();
        };
        let count = (count as usize).min(64);
        let mut offsets = Vec::with_capacity(count);
        for index in 0..count {
            if let Some(offset) = u32_at(bytes, 12 + index * 4) {
                offsets.push(offset as usize);
            }
        }
        return offsets;
    }
    if matches!(&tag, b"\x00\x01\x00\x00" | b"true" | b"OTTO" | b"typ1") {
        vec![0]
    } else {
        Vec::new()
    }
}

/// 只有一个 `name` 表记录被读取；表中 nameID 16（typographic family）优先于 1（legacy family）。
fn face_family_names(bytes: &[u8], face_offset: usize) -> Option<(String, Option<String>)> {
    let num_tables = u16_at(bytes, face_offset.checked_add(4)?)? as usize;
    for index in 0..num_tables.min(512) {
        let record = face_offset.checked_add(12 + index * 16)?;
        let Some(tag) = tag_at(bytes, record) else {
            return None;
        };
        if &tag != b"name" {
            continue;
        }
        let offset = u32_at(bytes, record + 8)? as usize;
        let length = u32_at(bytes, record + 12)? as usize;
        return parse_name_table(bytes, offset, length);
    }
    None
}

fn parse_name_table(
    bytes: &[u8],
    offset: usize,
    length: usize,
) -> Option<(String, Option<String>)> {
    let end = offset.checked_add(length)?.min(bytes.len());
    if offset >= end {
        return None;
    }
    let count = u16_at(bytes, offset.checked_add(2)?)? as usize;
    let string_offset = u16_at(bytes, offset.checked_add(4)?)? as usize;
    let storage = offset.checked_add(string_offset)?;

    // score：nameID 16 优先于 1；同优先级下先到先得（Windows 记录通常排在前面）。
    let mut english: Option<(u8, String)> = None;
    let mut localized: Option<(u8, String)> = None;
    for index in 0..count.min(4096) {
        let record = offset.checked_add(6 + index * 12)?;
        let platform = u16_at(bytes, record)?;
        let encoding = u16_at(bytes, record.checked_add(2)?)?;
        let language = u16_at(bytes, record.checked_add(4)?)?;
        let name_id = u16_at(bytes, record.checked_add(6)?)?;
        let text_len = u16_at(bytes, record.checked_add(8)?)? as usize;
        let text_offset = u16_at(bytes, record.checked_add(10)?)? as usize;
        if !matches!(name_id, 1 | 16) {
            continue;
        }
        let start = storage.checked_add(text_offset)?;
        let slice = bytes.get(start..start.checked_add(text_len)?)?;
        let Some(text) = decode_name(platform, encoding, slice) else {
            continue;
        };
        let text = normalize_family(&text);
        if text.is_empty() {
            continue;
        }
        let score = if name_id == 16 { 2 } else { 1 };
        if is_chinese_language(platform, language) {
            if localized.as_ref().is_none_or(|(best, _)| score > *best) {
                localized = Some((score, text));
            }
        } else if is_english_language(platform, language)
            && english.as_ref().is_none_or(|(best, _)| score > *best)
        {
            english = Some((score, text));
        }
    }

    let localized = localized.map(|(_, value)| value);
    let family = english
        .map(|(_, value)| value)
        .or_else(|| localized.clone())?;
    let localized = localized.filter(|value| *value != family);
    Some((family, localized))
}

fn decode_name(platform: u16, _encoding: u16, bytes: &[u8]) -> Option<String> {
    match platform {
        // Windows（3）与 Unicode（0）平台使用 UTF-16BE。
        0 | 3 => {
            if bytes.len() % 2 != 0 {
                return None;
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16(&units).ok()
        }
        // Mac（1）与 ISO（2）平台的族名在 ASCII 范围内完全一致；高位字节按 Latin-1 兜底。
        _ => Some(bytes.iter().map(|byte| *byte as char).collect()),
    }
}

fn is_chinese_language(platform: u16, language: u16) -> bool {
    match platform {
        3 => matches!(language, 0x0804 | 0x0404 | 0x0C04 | 0x1004 | 0x1404),
        1 => matches!(language, 19 | 33),
        _ => false,
    }
}

fn is_english_language(platform: u16, language: u16) -> bool {
    match platform {
        3 => language == 0x0409,
        0 => true,
        1 => language == 0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个仅含 `name` 表的 SFNT 字体（足以验证族名解析）。
    fn build_sfnt(tag: &[u8; 4], name_table: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(tag);
        out.extend_from_slice(&1u16.to_be_bytes()); // numTables
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        // 表记录：tag + checksum + offset + length
        out.extend_from_slice(b"name");
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&28u32.to_be_bytes());
        out.extend_from_slice(&(name_table.len() as u32).to_be_bytes());
        out.extend_from_slice(name_table);
        out
    }

    fn utf16be(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .flat_map(|unit| unit.to_be_bytes())
            .collect()
    }

    /// `records` = (platform, encoding, language, nameID, text)
    fn build_name_table(records: &[(u16, u16, u16, u16, &str)]) -> Vec<u8> {
        let mut strings: Vec<u8> = Vec::new();
        let mut entries: Vec<(u16, u16, u16, u16, usize, usize)> = Vec::new();
        for (platform, encoding, language, name_id, text) in records {
            let offset = strings.len();
            let encoded = if *platform == 3 || *platform == 0 {
                utf16be(text)
            } else {
                text.as_bytes().to_vec()
            };
            let length = encoded.len();
            strings.extend_from_slice(&encoded);
            entries.push((*platform, *encoding, *language, *name_id, offset, length));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // format
        out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        out.extend_from_slice(&((6 + entries.len() * 12) as u16).to_be_bytes());
        for (platform, encoding, language, name_id, offset, length) in entries {
            out.extend_from_slice(&platform.to_be_bytes());
            out.extend_from_slice(&encoding.to_be_bytes());
            out.extend_from_slice(&language.to_be_bytes());
            out.extend_from_slice(&name_id.to_be_bytes());
            out.extend_from_slice(&(length as u16).to_be_bytes());
            out.extend_from_slice(&(offset as u16).to_be_bytes());
        }
        out.extend_from_slice(&strings);
        out
    }

    #[test]
    fn parses_english_and_chinese_family_names() {
        let table = build_name_table(&[
            (3, 1, 0x0409, 1, "Microsoft YaHei"),
            (3, 1, 0x0804, 1, "微软雅黑"),
            (1, 0, 0, 16, "Microsoft YaHei UI"),
        ]);
        let font = build_sfnt(b"\x00\x01\x00\x00", &table);
        let parsed = parse_font_families(&font);
        assert_eq!(parsed.len(), 1, "单 face 字体只应产出一个族");
        // nameID 16 优先于 1。
        assert_eq!(parsed[0].0, "Microsoft YaHei UI");
        assert_eq!(parsed[0].1.as_deref(), Some("微软雅黑"));
    }

    #[test]
    fn chinese_name_is_used_when_no_english_name_exists() {
        let table = build_name_table(&[(3, 1, 0x0804, 1, "思源黑体")]);
        let font = build_sfnt(b"\x00\x01\x00\x00", &table);
        let parsed = parse_font_families(&font);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "思源黑体");
        assert_eq!(parsed[0].1, None, "族名与本地化名相同时不重复返回");
    }

    #[test]
    fn parses_every_face_of_a_collection() {
        let mut first = build_sfnt(
            b"\x00\x01\x00\x00",
            &build_name_table(&[(3, 1, 0x0409, 1, "Face One")]),
        );
        let mut second = build_sfnt(
            b"\x00\x01\x00\x00",
            &build_name_table(&[(3, 1, 0x0409, 1, "Face Two")]),
        );
        // 组装最小 TTC：ttcf + version + numFonts + offsets + 两个 face。
        let header_len = 12 + 8;
        let first_offset = header_len;
        let second_offset = first_offset + first.len();
        // TTC table record 的偏移是相对整个集合文件，而不是各 face 起点。
        first[20..24].copy_from_slice(&((first_offset + 28) as u32).to_be_bytes());
        second[20..24].copy_from_slice(&((second_offset + 28) as u32).to_be_bytes());
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&0x00010000u32.to_be_bytes());
        ttc.extend_from_slice(&2u32.to_be_bytes());
        ttc.extend_from_slice(&(first_offset as u32).to_be_bytes());
        ttc.extend_from_slice(&(second_offset as u32).to_be_bytes());
        ttc.extend_from_slice(&first);
        ttc.extend_from_slice(&second);

        let parsed = parse_font_families(&ttc);
        let families: Vec<&str> = parsed.iter().map(|(family, _)| family.as_str()).collect();
        assert_eq!(families, vec!["Face One", "Face Two"]);
    }

    #[test]
    fn malformed_bytes_do_not_panic() {
        assert!(parse_font_families(&[]).is_empty());
        assert!(parse_font_families(b"not-a-font").is_empty());
        assert!(parse_font_families(b"\x00\x01\x00\x00").is_empty());
        // 声明了 name 表但表体越界
        let mut truncated = build_sfnt(b"\x00\x01\x00\x00", &[0, 0, 0, 0]);
        truncated.truncate(30);
        assert!(parse_font_families(&truncated).is_empty());
    }

    #[test]
    fn family_normalization_never_exceeds_character_limit_at_whitespace_boundary() {
        let input = format!("{} X", "x".repeat(MAX_FAMILY_NAME_CHARS - 1));
        let normalized = normalize_family(&input);
        assert_eq!(normalized, "x".repeat(MAX_FAMILY_NAME_CHARS - 1));
        assert!(normalized.chars().count() <= MAX_FAMILY_NAME_CHARS);
    }

    #[test]
    fn inspector_accepts_supported_signatures_only() {
        let inspector = LocalFontFileInspector;
        let ttf = build_sfnt(
            b"\x00\x01\x00\x00",
            &build_name_table(&[(3, 1, 0x0409, 1, "Example Sans")]),
        );
        let inspected = inspector.inspect("ExampleSans-Regular.ttf", &ttf).unwrap();
        assert_eq!(inspected.family_name, "Example Sans");
        assert_eq!(inspected.extension, "ttf");
        assert_eq!(inspected.mime_type, "font/ttf");
        // 摘要必须是小写十六进制 64 字符，且同一份字节稳定复现。
        assert_eq!(inspected.sha256.len(), 64);
        assert!(
            inspected
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(inspected.sha256, sha256_hex(&ttf));
        assert_eq!(
            sha256_hex(&ttf),
            inspector.inspect("renamed.ttf", &ttf).unwrap().sha256,
            "摘要只取决于字节，不取决于文件名"
        );
        // 已知向量：空输入。
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // OTF：签名 OTTO
        let otf = build_sfnt(
            b"OTTO",
            &build_name_table(&[(3, 1, 0x0409, 1, "Example Serif")]),
        );
        let inspected = inspector.inspect("serif.otf", &otf).unwrap();
        assert_eq!(inspected.mime_type, "font/otf");

        // WOFF2：签名匹配，但表体是压缩的 → 族名退化为文件名
        let mut woff2 = Vec::from(*b"wOF2");
        woff2.extend_from_slice(&[0u8; 32]);
        let inspected = inspector.inspect("My_Display-Font.woff2", &woff2).unwrap();
        assert_eq!(inspected.family_name, "My Display Font");
        assert_eq!(inspected.mime_type, "font/woff2");
    }

    #[test]
    fn inspector_rejects_bad_input() {
        let inspector = LocalFontFileInspector;
        let ttf = build_sfnt(b"\x00\x01\x00\x00", &build_name_table(&[]));

        let err = inspector.inspect("font.ttc", &ttf).unwrap_err();
        assert_eq!(err.code().as_str(), "FONT_FORMAT_UNSUPPORTED");
        let err = inspector.inspect("font", &ttf).unwrap_err();
        assert_eq!(err.code().as_str(), "FONT_FORMAT_UNSUPPORTED");
        // 扩展名与签名不匹配（把 TTF 改名成 woff2）
        let err = inspector.inspect("font.woff2", &ttf).unwrap_err();
        assert_eq!(err.code().as_str(), "FONT_FILE_INVALID");
        // 空文件
        let err = inspector.inspect("font.ttf", &[]).unwrap_err();
        assert_eq!(err.code().as_str(), "FONT_FILE_INVALID");
        // 超出大小上限
        let oversized = vec![0u8; MAX_FONT_FILE_BYTES + 1];
        let err = inspector.inspect("big.ttf", &oversized).unwrap_err();
        assert_eq!(err.code().as_str(), "FONT_FILE_INVALID");
    }

    #[test]
    fn catalog_enumerates_only_font_files_and_never_leaks_paths() {
        let dir = tempfile::tempdir().unwrap();
        let fonts = dir.path().join("nested");
        std::fs::create_dir_all(&fonts).unwrap();
        let font = build_sfnt(
            b"\x00\x01\x00\x00",
            &build_name_table(&[
                (3, 1, 0x0409, 1, "Example Sans"),
                (3, 1, 0x0804, 1, "示例黑体"),
            ]),
        );
        std::fs::write(fonts.join("example.ttf"), &font).unwrap();
        std::fs::write(fonts.join("notes.txt"), b"not a font").unwrap();
        std::fs::write(fonts.join("broken.ttf"), b"\x00\x01\x00\x00truncated").unwrap();

        let catalog = LocalSystemFontCatalog::with_dirs(vec![dir.path().to_path_buf()]);
        let listed = catalog.list_families().unwrap();
        assert_eq!(listed.len(), 1, "坏字体与非字体文件必须被跳过");
        assert_eq!(listed[0].family, "Example Sans");
        assert_eq!(listed[0].localized_family.as_deref(), Some("示例黑体"));
        assert!(!listed[0].family.contains('/'));
        assert!(!listed[0].family.contains('\\'));
    }

    #[test]
    fn catalog_caches_and_tolerates_missing_directories() {
        let catalog = LocalSystemFontCatalog::with_dirs(vec![PathBuf::from(
            "/definitely/not/a/real/font/directory",
        )]);
        assert!(catalog.list_families().unwrap().is_empty());
        // 第二次读取走缓存，结果稳定。
        assert!(catalog.list_families().unwrap().is_empty());
    }

    #[test]
    fn hidden_system_families_are_skipped() {
        assert!(is_hidden_family(".SF NS Text"));
        assert!(!is_hidden_family("SF Pro Text"));
    }
}
