//! 受控外观资产存储（Appearance Stage 1B）。
//!
//! 这个模块是外观资产**字节**的唯一落点，它承担三件领域与 UI 都不该管的事：
//!
//! 1. **路径由后端决定**。目标文件名只由不透明资产 ID 与**嗅探出的**格式扩展名拼成，
//!    用户提供的文件名不参与任何一段。前端即使伪造 `C:/…` 或 `../../` 也无处施加影响
//!    ——公开命令签名里根本没有路径参数（见 `src-tauri/commands/appearance.rs`）。
//! 2. **导入前先校验，再落盘**。先看大小（按种类分档的上限），再嗅探文件签名，且签名
//!    必须与资产种类匹配：扩展名随手就能改，签名不行。不匹配的文件一个字节都不会落盘。
//!    校验的语义是**大小 + 格式签名/容器头（受控嗅探窗口 [`SNIFF_BYTES`]）**：静态与
//!    动态壁纸的格式集合不重叠（动态只接受可由 `<video>` 播放的 MP4 / WebM，APNG、
//!    GIF 与动画 WebP 不收），但它不是完整解码验证——超出窗口的字节不参与判定，通过
//!    校验既不代表「文件里没有更靠后的动画标记」，也不代表这份字节一定能解码播放。
//! 3. **先写临时文件再原子替换**。任何失败路径都清理临时文件；成功路径用一次 rename
//!    把完整的字节换上去，读者永远看不到半个文件。
//!
//! 本模块不接触 SQLite：登记行与字节的一致性由 Application 层（`AppearanceService`）
//! 负责编排，两侧的失败顺序在那里有明确约定。
//!
//! 稳定错误码（命令层原样映射为 ErrorDto.code）：
//! - `APPEARANCE_ASSET_SIGNATURE_MISMATCH`：字节格式不是该资产种类的受支持格式；
//! - `APPEARANCE_ASSET_TOO_LARGE`：体积超出该种类上限（或为空）；
//! - `APPEARANCE_ASSET_STORAGE_FAILED`：受控存储的读写失败（磁盘/权限）。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use haven_application::services::appearance::{AppearanceAssetFile, AppearanceAssetStorage};
use haven_common::{AppError, ErrorKind};
use haven_domain::appearance::{AppearanceAssetId, AppearanceAssetKind};

/// 字节格式不是该资产种类的受支持格式。
pub const APPEARANCE_ASSET_SIGNATURE_MISMATCH: &str = "APPEARANCE_ASSET_SIGNATURE_MISMATCH";
/// 资产体积为空或超出该种类上限。
pub const APPEARANCE_ASSET_TOO_LARGE: &str = "APPEARANCE_ASSET_TOO_LARGE";
/// 受控存储读写失败。
pub const APPEARANCE_ASSET_STORAGE_FAILED: &str = "APPEARANCE_ASSET_STORAGE_FAILED";

/// 嗅探签名与容器头时读取的最大字节数（64 KiB）。
///
/// 这个窗口决定「容器头验证」到底能看到多少：签名、WebM 的 `DocType`（紧随 4 字节
/// EBML 魔数）、PNG 的块序列（`acTL` / `IDAT`）、WebP 的 `VP8X` 标志位都落在最前面
/// 几 KB 内；64 KiB 给的是余量——一个体积正常的 PNG 不可能把 `acTL` 推到这么后面，
/// 因为规范要求它在第一个 `IDAT` 之前，而 IDAT 之前不可能塞下 64 KiB 的元数据还不
/// 影响可解码性。
///
/// 但它是**有界窗口，不是完整解码**：超出窗口的字节不会被读来判断格式，因此这份
/// 校验既不承诺「文件里没有更靠后的动画标记」，也不承诺「通过校验的字节一定能解码
/// 播放」。窗口越大越慢（每个导入都要多读一点），64 KiB 是这两者之间明确的取值，
/// 而不是「已经足够证明」。
const SNIFF_BYTES: usize = 64 * 1024;
/// 复制字节时使用的缓冲区大小。
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// 受控外观资产存储：把校验过的字节放进应用管理目录。
///
/// 目录本身**不在构造时创建**：没有任何导入时不应留下空目录，测试用内存库时也不会
/// 因为构造 AppState 就在临时目录里造出东西。
pub struct LocalAppearanceAssetStorage {
    root: PathBuf,
}

impl LocalAppearanceAssetStorage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 默认根目录：跟随数据库所在的数据目录（与 `ArtworkCache::default_root` 同一套
    /// 「数据目录」事实），即 `{data_dir}/AppearanceAssets`。数据库是内存库（测试）时
    /// 回落到临时目录。
    pub fn default_root(db: &crate::db::Db) -> PathBuf {
        db.path()
            .and_then(|path| path.parent().map(|parent| parent.join("AppearanceAssets")))
            .unwrap_or_else(|| std::env::temp_dir().join("haven-appearance-assets"))
    }

    /// 受控根目录（诊断/测试用；不出 IPC）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 删除同一 ID 名下除 `keep` 以外的全部文件（尽力而为）。
    ///
    /// 一个 ID 只能对应一个文件，否则「资产存在 ⟺ 字节存在」就不再是 1:1，删除与读路径
    /// 都要面对「哪个才是当前字节」。正常路径上 ID 每次导入都是新的，这里覆盖的是
    /// 「同一 ID 换了格式重新导入」与「上次导入崩溃留下的临时文件」两种残留；
    /// 清理失败不改变导入结论——新字节已经完整落盘，残留会在删除时按前缀一并清掉。
    fn remove_other_files(&self, id: AppearanceAssetId, keep: &Path) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let id_text = id.to_string();
        let prefix = format!("{id_text}.");
        for entry in entries.flatten() {
            let path = entry.path();
            if path == keep {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == id_text.as_str() || name.starts_with(prefix.as_str()) {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

impl AppearanceAssetStorage for LocalAppearanceAssetStorage {
    fn import_file(
        &self,
        source: &Path,
        id: AppearanceAssetId,
        kind: AppearanceAssetKind,
    ) -> Result<u64, AppError> {
        // 1) 大小：先用元数据挡掉空文件与明显超限的文件，避免把几百 MB 读一遍才发现。
        let metadata = fs::metadata(source).map_err(source_unavailable)?;
        if !metadata.is_file() {
            return Err(not_a_file());
        }
        validate_size(metadata.len(), kind)?;

        // 2) 签名：只读文件头，判定真实格式必须与资产种类匹配。
        let mut input = fs::File::open(source).map_err(source_unavailable)?;
        let mut header = vec![0u8; SNIFF_BYTES];
        let filled = read_up_to(&mut input, &mut header)?;
        let extension =
            detect_format(&header[..filled], kind).ok_or_else(|| signature_mismatch(kind))?;

        // 3) 落盘：目标名只由 ID 与嗅探出的扩展名组成，用户文件名不参与。
        fs::create_dir_all(&self.root).map_err(storage_failed)?;
        let target = self.root.join(format!("{id}.{extension}"));
        let temporary = self
            .root
            .join(format!("{id}.{extension}.{}.tmp", uuid::Uuid::new_v4()));

        let written = match copy_into(&temporary, &header[..filled], &mut input, kind) {
            Ok(written) => written,
            Err(error) => {
                // 半成品必须清掉：失败路径不允许留下任何以本 ID 命名的字节。
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::remove_file(&temporary);
            return Err(storage_failed(error));
        }
        self.remove_other_files(id, &target);
        Ok(written)
    }

    fn remove_file(&self, id: AppearanceAssetId) -> Result<(), AppError> {
        // 目标名由 `import_file` 唯一决定（`<id>.<规范化扩展名>`）。按 ID 前缀清理，
        // 既覆盖当前格式，也能清掉同一 ID 曾经以别的格式落盘、或上次导入崩溃留下的
        // 临时文件残留（它们同样以 `<id>.` 开头）。
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            // 根目录不存在 == 没有可删的字节；删除是幂等的。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage_failed(error)),
        };
        let id_text = id.to_string();
        let prefix = format!("{id_text}.");
        for entry in entries {
            let entry = entry.map_err(storage_failed)?;
            let name = entry.file_name();
            // 非 UTF-8 文件名不可能是本模块写出的（本模块只写 ASCII 名字）。
            let Some(name) = name.to_str() else {
                continue;
            };
            if name != id_text.as_str() && !name.starts_with(prefix.as_str()) {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(storage_failed(error)),
            }
        }
        Ok(())
    }

    fn open_file(&self, id: AppearanceAssetId) -> Result<AppearanceAssetFile, AppError> {
        let entries = fs::read_dir(&self.root).map_err(storage_failed)?;
        let id_text = id.to_string();
        let prefix = format!("{id_text}.");
        let mut candidate: Option<PathBuf> = None;
        for entry in entries {
            let entry = entry.map_err(storage_failed)?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(prefix.as_str()) || name.ends_with(".tmp") || !path.is_file() {
                continue;
            }
            // 一个 opaque ID 只能对应一个正式字节文件；多份残留不能凭文件名猜哪份生效。
            if candidate.is_some() {
                return Err(asset_unavailable());
            }
            candidate = Some(path);
        }
        let path = candidate.ok_or_else(asset_unavailable)?;
        let file = fs::File::open(&path).map_err(storage_failed)?;
        Ok(AppearanceAssetFile { file, path })
    }
}

/// 体积边界：必须为正且不超过该种类的上限（与 045 的 CHECK、领域的 `max_bytes` 同一组数字）。
fn validate_size(byte_size: u64, kind: AppearanceAssetKind) -> Result<(), AppError> {
    if byte_size == 0 || byte_size > kind.max_bytes() {
        return Err(too_large(kind));
    }
    Ok(())
}

/// 按资产种类嗅探字节格式，返回落盘使用的**规范扩展名**。
///
/// 扩展名来自签名而不是文件名：`photo.png` 里装 PNG 才算静态壁纸，改名成 `.png` 的
/// 可执行文件在这一步就被拒。返回 `None` 表示「不是该种类的受支持格式」。
fn detect_format(header: &[u8], kind: AppearanceAssetKind) -> Option<&'static str> {
    match kind {
        AppearanceAssetKind::Font => detect_font(header),
        AppearanceAssetKind::StaticWallpaper => detect_still_image(header),
        AppearanceAssetKind::DynamicWallpaper => detect_motion(header),
    }
}

/// 字体：TTF（含 Apple 的 `true` 变体）、OTF、WOFF、WOFF2。
///
/// 刻意**不**接受 `ttcf`（TrueType Collection）：一个集合文件里有多张字体，而资产模型
/// 没有承载「用第几张」的字段，收下它只会得到一个无法确定渲染哪一张的资产。
fn detect_font(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"wOFF") {
        return Some("woff");
    }
    if header.starts_with(b"wOF2") {
        return Some("woff2");
    }
    // OpenType with CFF outlines（必须在 `true`/`ttcf` 之前判定：首字节不同，顺序无关，
    // 但显式分段更清楚）。
    if header.starts_with(b"OTTO") {
        return Some("otf");
    }
    if header.starts_with(&[0x00, 0x01, 0x00, 0x00]) || header.starts_with(b"true") {
        return Some("ttf");
    }
    None
}

/// 静态壁纸：PNG、JPEG、WebP——三种都必须是**真正静态**的那一份。
///
/// 静态与动态两条路径的格式集合刻意不重叠：动态壁纸走 HTML `<video>`，因此这里只收
/// 视频容器 MP4 / WebM；GIF 虽然是动画位图，但 `<video>` 不会播放 GIF，不能登记为
/// 可用的动态壁纸。两个容器本身支持动画的格式也要在这里按容器头判掉：
///   - APNG：PNG 流里出现 `acTL` 块（规范要求它在第一个 `IDAT` 之前）；
///   - 动画 WebP：`VP8X` 扩展头里置了 ANIMATION 标志位。
///
/// 动画 WebP 也不在动态壁纸的接受集合里：那条路径由 `<video>` 渲染，而 WebView 的
/// `<video>` 放不了动画 WebP，收下它只会得到一个播不出来的背景层。
///
/// 这是**容器头验证**，不是完整解码验证：判定只用嗅探窗口（[`SNIFF_BYTES`]，64 KiB）
/// 里读到的块，不做像素级解析，也不承诺这份字节真的能解码播放。
fn detect_still_image(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(&PNG_SIGNATURE) {
        return if png_is_animated(header) {
            None
        } else {
            Some("png")
        };
    }
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    // RIFF 容器：`RIFF` + 4 字节长度 + `WEBP`。
    if header.len() >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WEBP" {
        return if webp_is_animated(header) {
            None
        } else {
            Some("webp")
        };
    }
    None
}

/// PNG 签名（8 字节）。
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// PNG 流里是否出现 `acTL`（APNG 的动画控制块）。
///
/// 规范要求 `acTL` 出现在第一个 `IDAT` 之前，因此这里顺着块头往后走，遇到 `acTL` 就是
/// 动图、遇到 `IDAT` 就是静态图。遍历只在嗅探窗口（[`SNIFF_BYTES`]，64 KiB）内进行：
/// 窗口耗尽时按静态处理——这是容器头验证的已知边界，不是「已经证明它没有动画」。
/// 窗口内足以覆盖任何可正常解码的 PNG 的元数据段，因此这个边界只对刻意构造的字节
/// 有影响（见 `rejects_an_apng_whose_actl_is_pushed_behind_a_large_chunk`）。
fn png_is_animated(header: &[u8]) -> bool {
    /// 块头：4 字节长度 + 4 字节类型；`acTL`/`IDAT` 判据都在这 8 字节里。
    const CHUNK_HEADER_BYTES: usize = 8;
    /// 块尾的 4 字节 CRC 也要计入下一个块的起点。
    const CHUNK_CRC_BYTES: usize = 4;

    let mut offset = PNG_SIGNATURE.len();
    while offset + CHUNK_HEADER_BYTES <= header.len() {
        let length = u32::from_be_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ]) as usize;
        let chunk_type = &header[offset + 4..offset + 8];
        if chunk_type == b"acTL" {
            return true;
        }
        if chunk_type == b"IDAT" {
            return false;
        }
        // 长度字段来自外部字节，不能当成可信前提：至少前进一个完整的空块，
        // 畸形长度最多让循环提前结束，不会原地打转。
        offset = offset
            .saturating_add(CHUNK_HEADER_BYTES + CHUNK_CRC_BYTES)
            .saturating_add(length);
    }
    false
}

/// WebP 是否是动画：`VP8X` 扩展头里的 ANIMATION 标志位。
///
/// 规范要求动画 WebP 必须使用扩展头并携带 `ANIM`/`ANMF` 块，因此这一个位就足以判定；
/// 没有 `VP8X` 的 WebP（简单有损/无损格式）按静态处理。
fn webp_is_animated(header: &[u8]) -> bool {
    /// ANIMATION 标志位（`VP8X` 标志字节的 bit 1）。
    const ANIMATION_FLAG: u8 = 0x02;
    // RIFF(4) + 长度(4) + WEBP(4) + "VP8X"(4) + 块长度(4) + 标志字节(1)
    const VP8X_FLAGS_OFFSET: usize = 20;
    header.len() > VP8X_FLAGS_OFFSET
        && &header[12..16] == b"VP8X"
        && header[VP8X_FLAGS_OFFSET] & ANIMATION_FLAG != 0
}

/// 动态壁纸：MP4、WebM。
///
/// 生产渲染路径是 HTML `<video>`，因此不能把 GIF 当作已实现的动态壁纸接受；如果未来
/// 要支持 GIF，必须先增加按格式选择 `<img>` / `<video>` 的真实运行时契约。
fn detect_motion(header: &[u8]) -> Option<&'static str> {
    // ISO BMFF：第一个 box 的 type 是 `ftyp`（紧跟在 4 字节长度之后）。
    if header.len() >= 12 && &header[4..8] == b"ftyp" {
        return Some("mp4");
    }
    // EBML 是 Matroska 与 WebM 共用的容器头，光看魔数会把 `.mkv` 也放进来。
    // 必须在 EBML Header 元素边界内解析出 DocType=webm；普通元数据里出现 `webm`
    // 子串并不能证明它是 WebM。
    if ebml_doc_type(header) == Some(&b"webm"[..]) {
        return Some("webm");
    }
    None
}

/// 从有界嗅探窗口解析 EBML Header 中的 DocType。任何截断、越界、未知长度或重复
/// DocType 都按不支持的容器拒绝。
fn ebml_doc_type(header: &[u8]) -> Option<&[u8]> {
    const EBML_HEADER_ID: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
    const EBML_DOC_TYPE_ID: u64 = 0x4282;

    if !header.starts_with(EBML_HEADER_ID) {
        return None;
    }
    let (header_size, size_length) = parse_ebml_vint(header, EBML_HEADER_ID.len(), true, 8)?;
    let mut cursor = EBML_HEADER_ID.len().checked_add(size_length)?;
    let header_end = cursor.checked_add(usize::try_from(header_size).ok()?)?;
    if header_end > header.len() {
        return None;
    }

    let mut doc_type = None;
    while cursor < header_end {
        let (element_id, id_length) = parse_ebml_vint(header, cursor, false, 4)?;
        cursor = cursor.checked_add(id_length)?;
        let (element_size, element_size_length) = parse_ebml_vint(header, cursor, true, 8)?;
        cursor = cursor.checked_add(element_size_length)?;
        let value_end = cursor.checked_add(usize::try_from(element_size).ok()?)?;
        if value_end > header_end {
            return None;
        }
        if element_id == EBML_DOC_TYPE_ID {
            if doc_type.is_some() {
                return None;
            }
            doc_type = Some(&header[cursor..value_end]);
        }
        cursor = value_end;
    }
    doc_type
}

/// 读一个 EBML variable-size integer。`strip_marker` 用于 size VINT；element ID 保留标记位。
fn parse_ebml_vint(
    bytes: &[u8],
    offset: usize,
    strip_marker: bool,
    max_width: usize,
) -> Option<(u64, usize)> {
    let first = *bytes.get(offset)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > max_width {
        return None;
    }
    let end = offset.checked_add(width)?;
    let encoded = bytes.get(offset..end)?;
    let marker = 1u8.checked_shl((8 - width) as u32)?;
    let first_value = if strip_marker {
        first & (marker - 1)
    } else {
        first
    };
    let mut value = u64::from(first_value);
    for byte in &encoded[1..] {
        value = value.checked_mul(256)?.checked_add(u64::from(*byte))?;
    }
    if strip_marker {
        let data_bits = 7 * width;
        if value == (1u64 << data_bits) - 1 {
            // 全 1 的 size 表示未知长度；EBML Header 必须是有界的。
            return None;
        }
    }
    Some((value, width))
}

/// 把文件头与剩余字节写进临时文件，并返回**实际写入**的字节数。
///
/// 上限在复制过程中会再查一次：`fs::metadata` 与真正读完之间存在窗口，文件可能在
/// 这之间变大。声明的大小不是事实，写下去多少字节才是。
fn copy_into(
    temporary: &Path,
    header: &[u8],
    input: &mut fs::File,
    kind: AppearanceAssetKind,
) -> Result<u64, AppError> {
    let mut output = fs::File::create(temporary).map_err(storage_failed)?;
    output.write_all(header).map_err(storage_failed)?;
    let mut written = header.len() as u64;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        let read = input.read(&mut buffer).map_err(source_unavailable)?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > kind.max_bytes() {
            return Err(too_large(kind));
        }
        output.write_all(&buffer[..read]).map_err(storage_failed)?;
    }
    // 先落盘再 rename：崩溃时要么看到旧文件，要么看到完整的新文件。
    output.sync_all().map_err(storage_failed)?;
    Ok(written)
}

/// 读取至多 `buffer.len()` 字节；短读是正常情况（小文件）。
fn read_up_to(input: &mut fs::File, buffer: &mut [u8]) -> Result<usize, AppError> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = input
            .read(&mut buffer[filled..])
            .map_err(source_unavailable)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

fn signature_mismatch(kind: AppearanceAssetKind) -> AppError {
    AppError::new(
        APPEARANCE_ASSET_SIGNATURE_MISMATCH,
        ErrorKind::Validation,
        format!("文件内容不是受支持的{}格式", kind.label()),
        false,
    )
}

/// 目录、设备、命名管道等都不是资产字节：Native 选择器理论上只给文件，这里仍然显式挡住。
fn not_a_file() -> AppError {
    AppError::new(
        APPEARANCE_ASSET_SIGNATURE_MISMATCH,
        ErrorKind::Validation,
        "只能导入普通文件",
        false,
    )
}

fn too_large(kind: AppearanceAssetKind) -> AppError {
    AppError::new(
        APPEARANCE_ASSET_TOO_LARGE,
        ErrorKind::Validation,
        format!(
            "资产体积必须在 1 到 {} 字节之间（{}）",
            kind.max_bytes(),
            kind.label()
        ),
        false,
    )
}

fn storage_failed(error: std::io::Error) -> AppError {
    AppError::new(
        APPEARANCE_ASSET_STORAGE_FAILED,
        ErrorKind::Storage,
        "外观资产存储读写失败",
        true,
    )
    .with_source(error)
}

/// 源文件读不出来（已被移动 / 没有权限）对用户就是「这个文件不能用」，不是可重试的服务故障。
fn source_unavailable(error: std::io::Error) -> AppError {
    AppError::new(
        APPEARANCE_ASSET_SIGNATURE_MISMATCH,
        ErrorKind::Validation,
        "无法读取所选文件（可能已被移动或没有读取权限）",
        false,
    )
    .with_source(error)
}

fn asset_unavailable() -> AppError {
    AppError::new(
        "APPEARANCE_ASSET_UNAVAILABLE",
        ErrorKind::NotFound,
        "外观资产不存在或当前不可用",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小可识别的字体头（只喂签名嗅探，不要求是能真正解析的字体）。
    const TTF_HEADER: [u8; 8] = [0x00, 0x01, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x80];
    /// 最小可识别的 PNG 头。
    const PNG_HEADER: [u8; 12] = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
    ];

    /// 每个用例独立的受控存储；`_dir` 保证临时目录活到用例结束。
    struct Fixture {
        _dir: tempfile::TempDir,
        storage: LocalAppearanceAssetStorage,
    }

    fn setup() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalAppearanceAssetStorage::new(dir.path().join("assets"));
        Fixture { _dir: dir, storage }
    }

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn stored_names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// 落盘名只由不透明 ID + **嗅探出的**扩展名组成：源文件名（含错扩展名、空格、中文）
    /// 不参与任何一段，因此前端也没有办法通过文件名影响存储路径。
    #[test]
    fn stores_bytes_under_the_opaque_id_and_ignores_the_source_name() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let id = AppearanceAssetId::new();
        let source = write_source(sources.path(), "伪装 图片.png", &TTF_HEADER);

        let written = fixture
            .storage
            .import_file(&source, id, AppearanceAssetKind::Font)
            .unwrap();
        assert_eq!(written, TTF_HEADER.len() as u64);

        assert_eq!(
            stored_names(fixture.storage.root()),
            vec![format!("{id}.ttf")]
        );
        let stored = fixture.storage.root().join(format!("{id}.ttf"));
        assert_eq!(fs::read(&stored).unwrap(), TTF_HEADER);
    }

    /// 签名必须与资产种类匹配：扩展名可改，签名不行。
    #[test]
    fn rejects_a_signature_that_does_not_match_the_kind() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let png = write_source(sources.path(), "a.png", &PNG_HEADER);

        let error = fixture
            .storage
            .import_file(&png, AppearanceAssetId::new(), AppearanceAssetKind::Font)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);
        // 签名不过的文件一个字节都不落盘（连根目录都不该被建出来）。
        assert!(!fixture.storage.root().exists());

        // 同一份 PNG 作为静态壁纸是合法输入。
        let wallpaper = AppearanceAssetKind::StaticWallpaper;
        let written = fixture
            .storage
            .import_file(&png, AppearanceAssetId::new(), wallpaper)
            .unwrap();
        assert_eq!(written, PNG_HEADER.len() as u64);
    }

    /// PNG 签名 + IHDR + `acTL`：APNG 的最小可识别容器头。
    fn apng_bytes() -> Vec<u8> {
        // 12 字节 = 签名(8) + IHDR 的长度字段(4)。
        let mut bytes = PNG_HEADER.to_vec();
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        // acTL：8 字节负载（帧数 + 播放次数），规范要求它在第一个 IDAT 之前。
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(b"acTL");
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes
    }

    /// 带 `IDAT` 的普通 PNG：块遍历必须在第一个 `IDAT` 处收口。
    fn png_with_idat_bytes() -> Vec<u8> {
        let mut bytes = PNG_HEADER.to_vec();
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(b"IDAT");
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes
    }

    /// 最小 WebP 头：`RIFF` + 长度 + `WEBP` + `VP8X` 扩展头 + 一个标志字节。
    fn webp_bytes(flags: u8) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&18u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8X");
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.push(flags);
        bytes
    }

    /// 静态与动态壁纸的格式集合必须**不重叠**：动态只接受 HTML `<video>` 能播放的
    /// MP4 / WebM；GIF、APNG 与动画 WebP 都必须拒绝，不能把不可播放的格式伪装成已实现
    /// 的动态壁纸。
    #[test]
    fn keeps_the_still_and_motion_format_sets_disjoint() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let still = AppearanceAssetKind::StaticWallpaper;
        let motion = AppearanceAssetKind::DynamicWallpaper;

        // GIF：静态一侧明确拒绝，动态一侧也拒绝（当前渲染器只有 `<video>`）。
        let gif = write_source(
            sources.path(),
            "a.gif",
            b"GIF89a\x01\x00\x01\x00\x00\x00\x00;",
        );
        for kind in [still, motion] {
            let error = fixture
                .storage
                .import_file(&gif, AppearanceAssetId::new(), kind)
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                APPEARANCE_ASSET_SIGNATURE_MISMATCH,
                "{kind:?} 不得接受 GIF"
            );
        }

        // APNG：静态一侧判掉（`acTL` 早于 `IDAT`）；动态一侧本来就不收 PNG 容器。
        let apng = write_source(sources.path(), "animated.png", &apng_bytes());
        for kind in [still, motion] {
            let error = fixture
                .storage
                .import_file(&apng, AppearanceAssetId::new(), kind)
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                APPEARANCE_ASSET_SIGNATURE_MISMATCH,
                "{kind:?} 不得接受 APNG"
            );
        }

        // 动画 WebP：静态一侧按 `VP8X` 的 ANIMATION 标志位判掉；动态一侧由 `<video>`
        // 渲染，而 `<video>` 放不了动画 WebP，同样不收。
        let animated_webp = write_source(sources.path(), "animated.webp", &webp_bytes(0x02));
        for kind in [still, motion] {
            let error = fixture
                .storage
                .import_file(&animated_webp, AppearanceAssetId::new(), kind)
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                APPEARANCE_ASSET_SIGNATURE_MISMATCH,
                "{kind:?} 不得接受动画 WebP"
            );
        }

        // 真正静态的 WebP 与带 `IDAT` 的普通 PNG 照常通过——边界收紧的是动图，不是
        // 整个格式。
        let still_webp = write_source(sources.path(), "plain.webp", &webp_bytes(0x00));
        let webp_id = AppearanceAssetId::new();
        fixture
            .storage
            .import_file(&still_webp, webp_id, still)
            .unwrap();

        let still_png = write_source(sources.path(), "plain.png", &png_with_idat_bytes());
        let png_id = AppearanceAssetId::new();
        fixture
            .storage
            .import_file(&still_png, png_id, still)
            .unwrap();

        // 被拒的文件一个字节都没落盘：存储里只有两份通过的静态资产。
        let mut expected = vec![format!("{webp_id}.webp"), format!("{png_id}.png")];
        expected.sort();
        assert_eq!(stored_names(fixture.storage.root()), expected);
    }

    /// APNG，但 `acTL` 前面塞了一个 `before_actl` 字节的辅助块（`tEXt`）。
    ///
    /// 用来把动画控制块推到嗅探窗口里的任意位置：判定必须是「窗口内读到的块」，
    /// 而不是只看文件最前面那几十个字节。
    fn apng_with_delayed_actl(before_actl: usize) -> Vec<u8> {
        let mut bytes = PNG_HEADER.to_vec();
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&(before_actl as u32).to_be_bytes());
        bytes.extend_from_slice(b"tEXt");
        bytes.resize(bytes.len() + before_actl, 0u8);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(b"acTL");
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes
    }

    /// 延迟的 `acTL`：动画控制块被推到接近嗅探窗口末端，静态一侧仍然必须拒绝。
    ///
    /// 规范要求 `acTL` 在第一个 `IDAT` 之前，而 IDAT 之前塞不下这么多元数据还不影响
    /// 可解码性——因此「窗口内」这一档就是真实文件可能出现的最坏情况。
    #[test]
    fn rejects_an_apng_whose_actl_is_pushed_behind_a_large_chunk() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let bytes = apng_with_delayed_actl(60 * 1024);
        let actl_at = bytes
            .windows(4)
            .position(|window| window == b"acTL")
            .expect("用例字节里必须有 acTL");
        assert!(
            actl_at < SNIFF_BYTES,
            "本用例覆盖的是「窗口内但很靠后」的 acTL（actl_at={actl_at}）"
        );
        let source = write_source(sources.path(), "delayed.png", &bytes);

        let error = fixture
            .storage
            .import_file(
                &source,
                AppearanceAssetId::new(),
                AppearanceAssetKind::StaticWallpaper,
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);
        assert!(
            !fixture.storage.root().exists(),
            "被拒的文件一个字节都不落盘"
        );
    }

    /// 已知边界：嗅探窗口**之外**的 `acTL` 读不到，这类刻意构造的字节会被当成静态
    /// PNG 收下（同一份字节作为动态壁纸本来也不收 PNG 容器）。
    ///
    /// 这条测试钉住的是当前的取值，不是期望的行为：校验窗口是 64 KiB 而不是整个文件，
    /// 所以「动图一律被拒」是**不成立**的说法——面向用户的文案不得这么承诺。谁把判定
    /// 换成全文件扫描，就该把这条测试一起改掉。
    #[test]
    fn documents_that_actl_beyond_the_sniff_window_is_not_seen() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let bytes = apng_with_delayed_actl(70 * 1024);
        let actl_at = bytes
            .windows(4)
            .position(|window| window == b"acTL")
            .expect("用例字节里必须有 acTL");
        assert!(
            actl_at >= SNIFF_BYTES,
            "本用例覆盖的是窗口之外的 acTL（actl_at={actl_at}）"
        );
        let source = write_source(sources.path(), "far.png", &bytes);

        let id = AppearanceAssetId::new();
        fixture
            .storage
            .import_file(&source, id, AppearanceAssetKind::StaticWallpaper)
            .unwrap();
        assert_eq!(
            stored_names(fixture.storage.root()),
            vec![format!("{id}.png")]
        );
    }

    /// 空文件、超限文件与目录都必须被挡在复制之前。
    #[test]
    fn rejects_empty_oversized_and_non_file_sources() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let font = AppearanceAssetKind::Font;

        let empty = write_source(sources.path(), "empty.ttf", &[]);
        let error = fixture
            .storage
            .import_file(&empty, AppearanceAssetId::new(), font)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_TOO_LARGE);

        // 超限文件用 `set_len` 造出长度，避免测试真的写满 32 MiB。
        let oversize = sources.path().join("oversize.ttf");
        fs::File::create(&oversize)
            .unwrap()
            .set_len(font.max_bytes() + 1)
            .unwrap();
        let error = fixture
            .storage
            .import_file(&oversize, AppearanceAssetId::new(), font)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_TOO_LARGE);

        // 目录不是资产字节。
        let error = fixture
            .storage
            .import_file(sources.path(), AppearanceAssetId::new(), font)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);
    }

    /// EBML 是 Matroska 与 WebM 共用的容器头：只认 `webm` DocType，`.mkv` 不得混入。
    /// 字体集合（`ttcf`）同样不收——资产模型没有承载「用第几张」的字段。
    #[test]
    fn rejects_webm_lookalikes_and_font_collections() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let motion = AppearanceAssetKind::DynamicWallpaper;

        let mut mkv_bytes = vec![0x1A, 0x45, 0xDF, 0xA3];
        mkv_bytes.extend_from_slice(b"\x8F\x42\x86\x81\x01\x42\x82\x88matroska");
        let mkv = write_source(sources.path(), "a.mkv", &mkv_bytes);
        let error = fixture
            .storage
            .import_file(&mkv, AppearanceAssetId::new(), motion)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);

        // EBML Header size is exactly 11 bytes: four-byte EBMLVersion and seven-byte DocType.
        let mut webm_bytes = vec![0x1A, 0x45, 0xDF, 0xA3];
        webm_bytes.extend_from_slice(b"\x8B\x42\x86\x81\x01\x42\x82\x84webm");
        let expected = webm_bytes.len() as u64;
        let webm = write_source(sources.path(), "a.webm", &webm_bytes);
        let written = fixture
            .storage
            .import_file(&webm, AppearanceAssetId::new(), motion)
            .unwrap();
        assert_eq!(written, expected);

        // `webm` 出现在 Matroska 头内的 Void 元素里，不能冒充 DocType。
        let mut lookalike_bytes = vec![0x1A, 0x45, 0xDF, 0xA3];
        lookalike_bytes.extend_from_slice(b"\x95\x42\x86\x81\x01\x42\x82\x88matroska\xEC\x84webm");
        let lookalike = write_source(sources.path(), "lookalike.mkv", &lookalike_bytes);
        let error = fixture
            .storage
            .import_file(&lookalike, AppearanceAssetId::new(), motion)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);

        let ttc = write_source(sources.path(), "a.ttc", b"ttcf\x00\x01\x00\x00");
        let error = fixture
            .storage
            .import_file(&ttc, AppearanceAssetId::new(), AppearanceAssetKind::Font)
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_SIGNATURE_MISMATCH);
    }

    /// 同一个 ID 在存储里只能有一个文件：重新导入必须整体替换目标，且不留临时文件。
    #[test]
    fn replaces_the_target_atomically_and_leaves_no_temporary_files() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let id = AppearanceAssetId::new();
        let font = AppearanceAssetKind::Font;

        let first = write_source(sources.path(), "first.ttf", &TTF_HEADER);
        fixture.storage.import_file(&first, id, font).unwrap();
        assert_eq!(
            stored_names(fixture.storage.root()),
            vec![format!("{id}.ttf")]
        );

        // 同一 ID 换成 OTF 内容重新导入：只应留下替换后的那一个文件。
        let mut otf_bytes = vec![0x4F, 0x54, 0x54, 0x4F];
        otf_bytes.extend_from_slice(&[0xAA; 64]);
        let second = write_source(sources.path(), "second.otf", &otf_bytes);
        let written = fixture.storage.import_file(&second, id, font).unwrap();
        assert_eq!(written, otf_bytes.len() as u64);

        assert_eq!(
            stored_names(fixture.storage.root()),
            vec![format!("{id}.otf")]
        );
        let stored = fixture.storage.root().join(format!("{id}.otf"));
        assert_eq!(fs::read(&stored).unwrap(), otf_bytes);
    }

    /// 删除只影响目标 ID，且重复删除是幂等的（根目录不存在也算成功）。
    #[test]
    fn removes_only_the_matching_id_and_is_idempotent() {
        let fixture = setup();
        let sources = tempfile::tempdir().unwrap();
        let font = AppearanceAssetKind::Font;
        let first_id = AppearanceAssetId::new();
        let second_id = AppearanceAssetId::new();

        let first = write_source(sources.path(), "first.ttf", &TTF_HEADER);
        let second = write_source(sources.path(), "second.ttf", &TTF_HEADER);
        fixture.storage.import_file(&first, first_id, font).unwrap();
        fixture
            .storage
            .import_file(&second, second_id, font)
            .unwrap();

        fixture.storage.remove_file(first_id).unwrap();
        let remaining = vec![format!("{second_id}.ttf")];
        assert_eq!(stored_names(fixture.storage.root()), remaining);

        // 再删一次：已经不存在，仍然是成功（幂等）。
        fixture.storage.remove_file(first_id).unwrap();
        assert_eq!(stored_names(fixture.storage.root()), remaining);

        // 从未导入过（根目录都不存在）时同样幂等成功。
        let fresh = setup();
        fresh.storage.remove_file(AppearanceAssetId::new()).unwrap();
    }
}
