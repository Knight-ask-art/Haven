//! 自定义源凭据的生命周期：来源作用域版本槽 + 持久化清理队列 + 外部 I/O 独占租约。
//!
//! 本模块替换旧的「共享 target + 先删系统凭据再清记录」删除顺序。旧顺序的三处缺陷
//! 在这里被结构性消除，而不是靠更小心的时序：
//!
//! 1. **共享 target**：旧实现把每个来源的凭据写在 `haven:<provider>:<sourceId>` 上，
//!    两个并发写入者互相覆盖；失败方还会把「IO 之前读到的旧秘密」还原回去，把赢家的
//!    秘密盖掉（还原本身也可能失败，于是 keystore 与 DB 引用错位）。现在**每一次写入
//!    都分配一个独立的、不可猜测的版本槽**，且槽位**同时绑定来源身份与来源种类**：
//!    provider 段是 `<opds|komga|kavita>_<sourceId 的 sha256 前 24 字节>`（48 个十六进制
//!    字符），profile 段是随机 UUID（32 个十六进制字符）。写入者只能写自己的槽，失败方
//!    只能删自己的槽；槽位名不与 sourceId 拼接，因此来源 ID 的长度不再是 target 合法性的
//!    隐变量（provider 段 53 字符、总长 92 字符，都远低于 60/128 的上限）。
//! 2. **没有补偿所有权**：系统凭据库与设置行不可能在同一个事务里。现在外部写入**之前**
//!    先把槽位登记进持久化的暂存队列，于是任何一次失败或中断都留下一条可重放的清理
//!    义务；[`SourceRegistryService::retry_pending_cleanup`] 跨重启把它清干净。采纳只在
//!    同一个设置事务里发生，并且同时要求：来源仍在、种类未变、凭据引用与配置版本仍是
//!    IO 之前那一份、**自己那条暂存记录仍在队列里**（= 没有被清理扫描回收）。
//! 3. **删除顺序**：移除来源时先在**同一个事务**里撤销引用并登记清理义务（凭据槽位 +
//!    last-known-good 配置），再执行外部删除。外部删除失败不丢义务、不谎报成功：错误
//!    如实上抛，条目留在队列里等待重试。迟到的写入者既不能复活已删除的来源，也不能
//!    留下不被追踪的槽位——采纳栅栏会失败，随后它只删除自己刚写的那个槽。
//!
//! ## 外部 I/O 独占租约（本模块的关键前提）
//!
//! 「暂存 + 清理队列」单独**不足以**证明一次被中断的写入不会再有下文：暂存记录可能被
//! 扫描按有效期或容量压力回收并删除，而那个写入者仍然活着、仍然会完成它那次不参与任何
//! 事务的外部写入，然后在补偿删除之前崩溃——留下一个没有任何清理所有者的槽位，而且
//! 没有任何时间量能证明它不会再被写一次。
//!
//! 现在，**所有**会触碰自定义源凭据外部条目的公开入口（写入、清除、来源移除、清理扫描）
//! 都在整个区间内持有一份 [`SourceCredentialLease`]：
//!
//! ```text
//! 写入：  持租约 { 暂存(DB) → 外部 set → 采纳(DB) / 补偿删除(keystore+DB) }
//! 清除：  持租约 { 撤销+登记义务(DB) → 外部 delete → 出队(DB) }
//! 移除：  持租约 { 撤销记录+登记义务(DB) → 外部 delete → 出队(DB) }
//! 扫描：  持租约 { 过期暂存槽转义务(DB) → 外部 delete → 出队(DB) }
//! ```
//!
//! 持租约期间不存在第二个合规写者，因此扫描（以及暂存队列的容量回收）看到的每一条暂存
//! 记录都只可能属于**已经不会再继续**的写入。租约是**非重入**的：公开入口各获取一次，
//! 私有 helper 只接收已经持有的守卫，绝不递归获取。
//!
//! TVBox 配置的保存路径同样借用注册表**这一份**租约：它在「注册来源 → 写 last-known-good
//! 缓存」区间内持租约（`TvboxConfigSaveService::save`），因此一次合规的移除无法插进这两步
//! 之间、把一个刚注册、缓存还没写的来源删掉而留下没有身份指向的原始配置。远端取回**不**在
//! 租约内。
//!
//! 边界（不夸大）：租约只仲裁**遵守本契约的**路径，不让 keystore 与数据库变成原子，也
//! 不覆盖本进程之外的程序直接改写系统凭据库。受信任的 `CredentialStore` 实现必须在自己
//! 的 future 内完成系统调用——把写入甩给后台任务后立刻返回，会让副作用跑到租约之外。
//!
//! ## 清理队列的推进：有界、容量感知、公平
//!
//! 队列本身是**义务**的载体，因此推进它时必须同时满足三件事，任何一件做不到都会让
//! 「清干净」变成一句空话：
//!
//! - **有界**：一次扫描最多尝试 [`SOURCE_CREDENTIAL_CLEANUP_BATCH`] 条凭据与同样多的缓存
//!   条目；条目**按身份**出队，所以扫描期间并发新增的义务不会被读取时的陈旧快照覆盖。
//! - **容量感知的退休**：把过期暂存槽转入清理队列时只看**当前还剩多少空位**，只转得下
//!   的那几条；转不下的**原样留在暂存队列**里——暂存记录本身就是那条槽位的持久化清理
//!   所有者，因此「队列满」不是遗忘，只是排队，等下一次扫描腾出空位再转。退休**绝不**
//!   因为容量不足而让整次事务失败：否则扫描会在同一个点上重复同一次失败，连系统凭据库
//!   恢复之后队列也永远排不空，而写入、清除与移除会一起被永久卡住。
//! - **公平轮转**：一轮里被守卫拦下或外部删除失败的条目，在写事务里被**重排**到队尾
//!   （条目连同引用与归属原样留着）。没有这一步，队首那一批长期不可结项的条目会把排在
//!   后面的合法义务永远挡在批量窗口之外——队列看起来在跑，实际停滞。
//!
//! 缓存清理用的是同一套「来源所有权」纪律：一条待清理条目在来源记录**仍然存在**时不会
//! 删掉该来源的 last-known-good 配置（伪造或陈旧的条目不得让一个活着的来源静默失去
//! 配置），义务留在队列里，等来源真的被移除之后再履行。
//!
//! ## 引用归属
//!
//! 读取、投影、暂存、采纳、撤销、移除与清理队列（**包括来源已被移除之后**）一律用
//! [`ref_belongs_to_source`] 判定归属：provider 段必须精确等于该来源作用域的 provider，
//! 或者条目恰好是历史稳定引用 `haven:<provider>:<sourceId>`。格式非法、串源、跨 provider
//! 的引用一律 fail closed——绝不被读出、绝不进入出站请求、绝不被拿去删系统凭据库里的
//! 任意条目，也绝不被静默丢弃（它们在清理队列里保持 `guarded`，等待人工判定）。
//! 清理队列条目在来源被移除之后仍然要判定归属，而那时已经没有来源记录可读，因此归属由
//! **sourceId 前缀**恢复出的种类决定：前缀本来就是由种类生成的，两者在写入路径上永不
//! 分叉（`add_custom_source_with_kind` 是唯一生成 sourceId 的地方）。
//!
//! 清理队列的**登记与出队都按精确身份**（引用 + 归属来源）匹配：同一引用若在队列里出现
//! 两条归属来源不同的条目（被伪造或手工改写），它们是两条彼此独立的义务——登记不会被
//! 「引用已存在」吞掉，出队也只结清本次那一条，绝不连带删掉另一条。

use std::error::Error as _;

use sha2::{Digest, Sha256};

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

use super::lease::{SourceCredentialLeaseGuard, source_credential_lease_unavailable};
use super::{
    CUSTOM_FEED_SOURCE_PREFIX, CUSTOM_TVBOX_SOURCE_PREFIX, CustomSourceKind,
    KAVITA_CREDENTIAL_PROVIDER, KOMGA_CREDENTIAL_PROVIDER, OPDS_CREDENTIAL_PROVIDER,
    SourceRegistryService, SourcesPayload, bump_source_revision, invalid_argument,
    mutations::SourcesMutation, not_found, source_config_stale, strip_custom_source,
};
use crate::services::source_config_cache::SourceConfigCache;
use crate::wire::{SourceRemoveResult, SourceSetCredentialRequest};

/// 单次清理扫描最多处理的条目数：一次调用绝不无界读取或删除。
pub const SOURCE_CREDENTIAL_CLEANUP_BATCH: usize = 64;
/// 同时「在途」的暂存槽上限；超出时最旧的槽位转入清理队列（绝不遗忘）。
///
/// 容量回收只发生在**已经持有租约**的暂存或扫描事务里，因此它看到的每一条暂存记录都
/// 只可能属于已经不会再继续的写入：容量压力不会把一次在途写入的清理所有者弄丢。
pub const MAX_STAGED_CREDENTIALS: usize = 32;
/// 待清理凭据条目上限；写满时新的变更 fail closed，而不是丢掉旧的清理义务。
///
/// 它同时是「过期暂存槽能不能被退休」的容量来源：退休只转入装得下的条数，装不下的继续
/// 留在暂存队列里当自己的清理所有者（见私有 helper `retire_expired_stages`）。对外暴露只
/// 为了让集成用例能构造一个**刚好写满**的队列，不构成任何新的写入口。
pub const MAX_CREDENTIAL_CLEANUP: usize = 256;
/// 待清理来源配置缓存条目上限。
const MAX_CACHE_CLEANUP: usize = 64;
/// 暂存槽有效期：超过它的暂存槽视为被中断的写入，转入清理队列。
///
/// 这不是「清静性证明」——持租约才是。它只是一道额外的保守延迟：即使写入者已经不可能
/// 再继续，也先把槽位留在队列里一段时间，避免用户刚看到一次失败、槽位就被下一次扫描
/// 顺手删掉，让排障现场消失。
pub const SOURCE_CREDENTIAL_STAGE_TTL_MS: i64 = 600_000;
/// 来源作用域段的十六进制长度（sha256 前 24 字节 = 192 位）。
const SOURCE_SCOPE_HEX_LEN: usize = 48;
/// 版本槽 profile 段的规范形状长度（UUID simple 形式）。
const VERSION_PROFILE_HEX_LEN: usize = 32;

/// 一个已落库、尚未被采纳的版本槽：外部写入**之前**登记，用于让中断的写入仍有清理
/// 所有者。只保存不透明引用，永远不含 secret。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StagedSourceCredential {
    /// `haven:<provider>_<source-scope>:<uuid>`。
    pub(super) reference: String,
    /// 这次写入针对的来源；清理不需要来源仍然存在。
    pub(super) source_id: String,
    pub(super) staged_at_ms: i64,
}

/// 一条必须从系统凭据库删除的引用（外部删除失败时留在队列里重试）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PendingCredentialCleanup {
    pub(super) reference: String,
    pub(super) source_id: String,
}

/// 一条待清理条目（凭据引用或来源配置缓存）在一次扫描里的处理结果。
enum DrainOutcome {
    /// 外部存储里确实存在并被删除，条目已出队。
    Deleted,
    /// 外部存储里本就不存在（幂等），条目已出队。
    Absent,
    /// **不删除也不出队**：引用不属于这个来源、仍被某个来源记录引用、或仍属于一条在途
    /// 暂存槽；对缓存而言是来源记录仍然存在，因此那份配置仍然属于一个有身份的来源。
    Guarded,
}

/// 一次凭据清理扫描的真实结果。字段全部是**观测到的事实**，不是承诺：
/// 失败条目一律留在队列里，由下一次扫描重试。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceCredentialCleanupReport {
    /// 本次扫描把多少条超过有效期的暂存槽转入了清理队列。
    pub stages_retired: u32,
    /// 本次扫描从系统凭据库实际删除了多少条引用。
    pub credentials_deleted: u32,
    /// 本次扫描处理完毕（删除成功或目标本就不存在）的引用条目数。
    pub credentials_settled: u32,
    /// 因为不属于该来源、仍被引用或仍在途而被守卫拦下、留在队列里的条目数。
    pub guarded: u32,
    /// 实际删除的来源配置缓存条目数。
    pub caches_deleted: u32,
    /// 因为来源记录仍然存在（那份配置仍然有身份指向）而被守卫拦下、留在队列里的缓存
    /// 条目数。
    pub caches_guarded: u32,
    /// 外部操作失败、留在队列里等待重试的条目数（凭据 + 缓存）。
    pub failed: u32,
    /// 扫描结束后凭据清理队列的剩余长度。
    pub credentials_remaining: u32,
    /// 扫描结束后缓存清理队列的剩余长度。
    pub caches_remaining: u32,
    /// 扫描结束后仍处于「在途」状态的暂存槽数量。
    pub staged_remaining: u32,
}

/// 来源身份 → 槽位 provider 里的作用域段：`sourceId` 的 sha256 前 24 字节（48 个小写
/// 十六进制字符）。
///
/// 这是「同一个来源」的 192 位绑定，比复用 sourceId 更强：它不泄露来源身份到 target 里
/// 的可读形式，同时把长度固定下来，来源 ID 的长短不再影响 target 的合法性。
fn source_scope(source_id: &str) -> String {
    let digest = Sha256::digest(source_id.as_bytes());
    let mut scope = String::with_capacity(SOURCE_SCOPE_HEX_LEN);
    for byte in digest.iter().take(SOURCE_SCOPE_HEX_LEN / 2) {
        let byte = *byte;
        scope.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        scope.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    scope
}

/// 某个来源种类 + 来源身份对应的新槽 provider 段；不使用凭据的种类返回 `None`。
fn scoped_provider(kind: CustomSourceKind, source_id: &str) -> Option<String> {
    let provider = SourceRegistryService::credential_provider(kind)?;
    Some(format!("{provider}_{}", source_scope(source_id)))
}

/// 版本槽 profile 段的规范形状：32 个小写十六进制字符（`Uuid::new_v4().simple()`）。
fn is_canonical_version_profile(profile: &str) -> bool {
    profile.len() == VERSION_PROFILE_HEX_LEN
        && profile
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
}

/// 从 `sourceId` 前缀恢复来源种类。
///
/// sourceId 的生成是**唯一**一处决定前缀的地方（`add_custom_source_with_kind`），而前缀
/// 与种类一一对应，因此这里恢复出的种类与记录里的 `kind` 在合法数据上永远一致。来源被
/// 删除之后，清理队列里剩下的条目只能靠它判定归属。
pub(super) fn kind_from_source_id(source_id: &str) -> Option<CustomSourceKind> {
    // 最长前缀优先：`custom_feed_` / `custom_komga_` / `custom_kavita_` / `custom_tvbox_`
    // 都是 `custom_` 的子前缀，判定顺序不能颠倒。
    if source_id.starts_with(CUSTOM_FEED_SOURCE_PREFIX) {
        return Some(CustomSourceKind::Feed);
    }
    if let Some(kind) = SourceRegistryService::comic_library_kind(source_id) {
        return Some(kind);
    }
    if source_id.starts_with(CUSTOM_TVBOX_SOURCE_PREFIX) {
        return Some(CustomSourceKind::Tvbox);
    }
    if source_id.starts_with(super::CUSTOM_SOURCE_PREFIX) {
        return Some(CustomSourceKind::Opds);
    }
    None
}

/// 引用是否**归属**该来源（种类 + 身份）。
///
/// 接受两种形状，其余一律拒绝：
/// - 新槽位：`haven:<kind-provider>_<source-scope>:<uuid32>`——provider 段必须精确等于
///   本来源的 scoped provider，profile 段必须是规范版本形状；
/// - 历史稳定引用：恰好 `haven:<kind-provider>:<sourceId>`（升级前写入的引用照常可读、
///   可轮换）。
///
/// 这条判定是读取、投影、暂存、采纳、撤销、移除与清理的**统一**准入条件：格式非法、
/// provider 串台（跨 provider/跨来源）或落到不使用凭据的种类上的引用一律不当作「本来源
/// 的凭据」处理，更不会拿去删系统凭据库里的任意条目。
pub(super) fn ref_belongs_to_source(
    kind: CustomSourceKind,
    source_id: &str,
    reference: &str,
) -> bool {
    let Ok(parsed) = reference.parse::<CredentialRef>() else {
        return false;
    };
    let Some(expected_provider) = SourceRegistryService::credential_provider(kind) else {
        return false;
    };
    // `parse_scoped` 保证恰好三段，因此第二段就是 provider 段、第三段是 profile 段。
    let mut parts = parsed.as_str().split(':');
    let _namespace = parts.next();
    let provider = parts.next().unwrap_or_default();
    let profile = parts.next().unwrap_or_default();
    if scoped_provider(kind, source_id).is_some_and(|scoped| scoped == provider) {
        return is_canonical_version_profile(profile);
    }
    provider == expected_provider && profile == source_id
}

/// 凭据清理队列写满：本次变更 fail closed，绝不丢掉已有的清理义务。
fn cleanup_queue_full() -> AppError {
    AppError::new(
        "SOURCE_CREDENTIAL_CLEANUP_FULL",
        ErrorKind::Conflict,
        "来源凭据清理队列已满，请稍后重试",
        true,
    )
}

/// 清理队列当前还剩多少空位（被伪造数据塞到超限时饱和为 0，绝不回绕）。
fn credential_cleanup_capacity(payload: &SourcesPayload) -> usize {
    MAX_CREDENTIAL_CLEANUP.saturating_sub(payload.credential_cleanup.len())
}

/// 一条待清理条目是否**恰好**是本次要履行的那条义务：引用与归属来源都必须对得上。
///
/// 清理队列是落库数据，可能被伪造或手工改写：同一个引用可以同时出现在两条不同归属来源
/// 的条目里（例如把一条合法义务的来源段改成别的来源）。只按引用判定会把那条不属于本次
/// 义务的行也算进来——登记时它会让合法义务被「已存在」吞掉，出队时它会把一条本不属于
/// 本次操作的义务一起删掉。两种后果都是静默丢掉一条义务（甚至丢掉一个仍然活着的来源的
/// 槽位引用），所以引用必须与归属来源一起判定。
fn is_exact_cleanup_obligation(
    entry: &PendingCredentialCleanup,
    source_id: &str,
    reference: &str,
) -> bool {
    entry.source_id == source_id && entry.reference == reference
}

/// 登记一条待清理引用（幂等：同一条**精确**义务只登记一次）。
fn enqueue_credential_cleanup(
    payload: &mut SourcesPayload,
    reference: &str,
    source_id: &str,
) -> Result<(), AppError> {
    if payload
        .credential_cleanup
        .iter()
        .any(|entry| is_exact_cleanup_obligation(entry, source_id, reference))
    {
        return Ok(());
    }
    if credential_cleanup_capacity(payload) == 0 {
        return Err(cleanup_queue_full());
    }
    payload.credential_cleanup.push(PendingCredentialCleanup {
        reference: reference.to_owned(),
        source_id: source_id.to_owned(),
    });
    Ok(())
}

/// 登记一个待清理的来源配置缓存（幂等）。
fn enqueue_cache_cleanup(payload: &mut SourcesPayload, source_id: &str) -> Result<(), AppError> {
    if payload.cache_cleanup.iter().any(|id| id == source_id) {
        return Ok(());
    }
    if payload.cache_cleanup.len() >= MAX_CACHE_CLEANUP {
        return Err(cleanup_queue_full());
    }
    payload.cache_cleanup.push(source_id.to_owned());
    Ok(())
}

/// 把一条暂存槽转为待清理条目，返回这条暂存槽是否存在并被转走。
///
/// 顺序是**先**在清理队列里登记（容量不足即整体失败，义务不丢），**再**从暂存队列移除。
/// 清理扫描只删除已经登记进清理队列的槽位，因此被中断的写入在任何时刻都有一名清理所有者。
///
/// 转走的是**这一条**暂存记录（按位置精确移除），而不是「所有引用相同的记录」：暂存队列
/// 同样是落库数据，按引用 `retain` 会把一条引用相同、归属来源不同的伪造记录一并抹掉，等于
/// 替它丢掉义务。登记用的归属来源取自这一条记录本身，绝不从别处猜。
fn retire_staged_slot(payload: &mut SourcesPayload, reference: &str) -> Result<bool, AppError> {
    let Some(index) = payload
        .staged_credentials
        .iter()
        .position(|staged| staged.reference == reference)
    else {
        return Ok(false);
    };
    let source_id = payload.staged_credentials[index].source_id.clone();
    enqueue_credential_cleanup(payload, reference, &source_id)?;
    payload.staged_credentials.remove(index);
    Ok(true)
}

/// 把超过有效期的暂存槽转入清理队列，返回转入的条数。
///
/// **容量感知**：只转入清理队列当前装得下的那几条。装不下的过期暂存槽**原样留在暂存
/// 队列**里——暂存记录本身就是那条槽位的持久化清理所有者，所以「队列满」不丢任何义务，
/// 只是把退休推迟到有空位的那一次扫描。
///
/// 这里**绝不**因为容量不足而返回错误：一次扫描要是整体回滚，同一个点上就会永远重复
/// 同一次失败——清理队列即使等到系统凭据库恢复也排不空，写入、清除与移除会一起被永久
/// 卡住。超出容量的过期槽位也不会让它「填一半就中止」：能转的照转，转不下的留队。
fn retire_expired_stages(payload: &mut SourcesPayload, now: UtcMillis) -> u32 {
    let expired: Vec<String> = payload
        .staged_credentials
        .iter()
        .filter(|staged| {
            now.0.saturating_sub(staged.staged_at_ms) >= SOURCE_CREDENTIAL_STAGE_TTL_MS
        })
        .map(|staged| staged.reference.clone())
        .collect();
    let mut retired = 0;
    for reference in expired {
        if credential_cleanup_capacity(payload) == 0 {
            // 没有空位：剩下的过期槽位保持「在途」身份，继续由自己的暂存记录兜底。
            break;
        }
        match retire_staged_slot(payload, &reference) {
            Ok(true) => retired += 1,
            // 空位刚刚预检过，因此这里只可能是「登记时发现引用已在队列里」这种幂等情况：
            // 那条槽位已经不再需要处理，继续看下一条。
            Ok(false) => {}
            // 容量在预检与登记之间被其它条目吃掉：停下即可，槽位仍然留在暂存队列里（有主）。
            Err(_) => break,
        }
    }
    retired
}

/// 暂存队列超限时把最旧的槽位转入清理队列（同样是「先登记后移除」）。
///
/// 只可能在**已经持有租约**的事务里发生（暂存与扫描都持租约），因此这里回收的槽位只
/// 可能属于已经不会再继续的写入——容量压力不会把一次在途写入的清理所有者弄丢。
///
/// 与 [`retire_expired_stages`] 的区别是这里**必须**做成功：腾不出位置就无法为本次写入
/// 预留清理义务，因此清理队列真的满时如实返回 `SOURCE_CREDENTIAL_CLEANUP_FULL`，让这次
/// 变更 fail closed，而不是在登记不下义务的情况下继续去写外部存储。
fn make_room_for_staging(payload: &mut SourcesPayload) -> Result<(), AppError> {
    while payload.staged_credentials.len() >= MAX_STAGED_CREDENTIALS {
        let Some(oldest) = payload
            .staged_credentials
            .iter()
            .min_by_key(|staged| staged.staged_at_ms)
            .map(|staged| staged.reference.clone())
        else {
            break;
        };
        // `oldest` 就取自这份列表，因此 `false`（引用已不在）在这里不可能出现；真出现了
        // 也宁可 fail closed，绝不在原地空转。
        if !retire_staged_slot(payload, &oldest)? {
            return Err(cleanup_queue_full());
        }
    }
    Ok(())
}

/// 暂存之前**先**为本次写入预留一条清理义务的位置。
///
/// 采纳/补偿阶段的 `enqueue_credential_cleanup` 只要因为容量不足而失败，一次**已经发生**
/// 的外部副作用就会失去可重试的所有者（补偿删除失败时尤其如此）。持租约期间没有第二个
/// 合规写者，因此在这里预留之后，本次操作后续的登记不会再因容量而失败。
fn reserve_cleanup_capacity(payload: &SourcesPayload) -> Result<(), AppError> {
    if credential_cleanup_capacity(payload) == 0 {
        return Err(cleanup_queue_full());
    }
    Ok(())
}

/// 复核栅栏：来源仍存在、种类未变、凭据引用与配置版本都还是进入 IO 之前那一份。
///
/// 任一条不成立即 [`source_config_stale`]：本次写入描述的不是当前来源。
fn check_fence(
    payload: &SourcesPayload,
    source_id: &str,
    fence: &CredentialFence,
) -> Result<(), AppError> {
    let Some(record) = payload
        .custom_sources
        .iter()
        .find(|c| c.source_id == source_id)
    else {
        // 来源在 IO 期间被删除：这次写入描述的不是任何存在的来源。
        return Err(source_config_stale());
    };
    if record.kind != fence.kind
        || record.credential_ref != fence.previous_ref
        || payload.source_revisions.get(source_id) != fence.previous_revision.as_ref()
    {
        return Err(source_config_stale());
    }
    Ok(())
}

/// 一次凭据写入的栅栏快照：外部写入前后各复核一次的那几个事实。
struct CredentialFence {
    kind: CustomSourceKind,
    previous_ref: Option<String>,
    previous_revision: Option<String>,
}

/// 补偿失败附在主错误上：主错误（与主文案）永不被替换，补偿失败只作为 source 附上；
/// 已有 source 时不再覆盖它（两层补偿同时失败时保留最先记录的那条）。
fn attach_compensation(primary: AppError, compensation: AppError) -> AppError {
    if primary.source().is_some() {
        return primary;
    }
    primary.with_source(compensation)
}

impl SourceRegistryService {
    /// 来源种类 → 凭据 provider 段（`haven:<provider>:<profile>` 的第二段前缀）。
    ///
    /// 不使用访问凭据的种类（订阅源、TVBox 配置）返回 `None`：调用方必须先拒绝，
    /// 而不是拿到一个永远不会被任何请求消费的凭据命名空间。
    pub fn credential_provider(kind: CustomSourceKind) -> Option<&'static str> {
        match kind {
            CustomSourceKind::Opds => Some(OPDS_CREDENTIAL_PROVIDER),
            CustomSourceKind::Komga => Some(KOMGA_CREDENTIAL_PROVIDER),
            CustomSourceKind::Kavita => Some(KAVITA_CREDENTIAL_PROVIDER),
            CustomSourceKind::Feed | CustomSourceKind::Tvbox => None,
        }
    }

    /// 分配一个**新的、不可猜测的、来源作用域**的版本槽。
    ///
    /// target = `haven:<provider>_<sha256(sourceId) 前 24 字节>:<uuid32>`：provider 段把
    /// 槽位绑定到**种类 + 来源身份**，profile 段每次写入重新随机。它不与 sourceId 拼接，
    /// 因此不存在「来源 ID 太长导致 target 非法」的路径。
    fn allocate_credential_slot(
        kind: CustomSourceKind,
        source_id: &str,
    ) -> Result<CredentialRef, AppError> {
        let provider = scoped_provider(kind, source_id)
            .ok_or_else(|| invalid_argument("该来源不使用单独的访问凭据"))?;
        let profile = uuid::Uuid::new_v4().simple().to_string();
        CredentialRef::new_scoped(&provider, &profile)
            .map_err(|error| invalid_argument(error.user_message()))
    }

    /// 取得外部凭据 I/O 租约。未注入租约的装配 face **fail closed**：只读注册表照常可用，
    /// 但没有任何入口会去写外部凭据库。
    ///
    /// `pub(crate)` 而不是私有：TVBox 配置保存路径（`TvboxConfigSaveService::save`）要在
    /// 「注册来源 → 写 last-known-good 缓存」区间内借用**同一份**租约，好让一次合规的移除
    /// 无法插进这两步之间。这是**借用**，不是新的仲裁者：租约的所有权与解释仍在注册表，
    /// 这里不新增任何通用管理器、公开端口或 IPC 入口。
    pub(crate) async fn acquire_credential_lease(
        &self,
    ) -> Result<Box<dyn SourceCredentialLeaseGuard>, AppError> {
        let lease = self
            .credential_lease
            .clone()
            .ok_or_else(source_credential_lease_unavailable)?;
        lease.acquire().await
    }

    /// `source_set_credential`：写/清系统凭据库条目并同步持久化引用。
    ///
    /// secret 只在本调用栈内以可清零类型存在，绝不出 IPC、日志或数据库。写入走
    /// 「分配新槽 → 落库暂存 → 外部写入 → 同一事务内采纳」四步；清除走「同一事务内
    /// 撤销引用并登记清理义务 → 外部删除」。两条路径都**没有**任何旧秘密还原分支，
    /// 并且都在**同一份**外部 I/O 租约内完成（租约在这里获取一次，私有 helper 只接收
    /// 已经持有的守卫，绝不递归获取）。
    pub async fn set_custom_source_credential(
        &self,
        request: &SourceSetCredentialRequest,
        store: &dyn CredentialStore,
    ) -> Result<(), AppError> {
        if !Self::is_custom_source_id(&request.source_id) {
            return Err(invalid_argument("仅自定义来源可配置凭据"));
        }
        if request.secret.as_deref() == Some("") {
            return Err(invalid_argument("凭据内容不能为空"));
        }
        let lease = self.acquire_credential_lease().await?;
        match request.secret.as_deref() {
            None => {
                self.clear_custom_source_credential(&request.source_id, store, lease.as_ref())
                    .await
            }
            Some(secret) => {
                self.write_custom_source_credential(
                    &request.source_id,
                    secret,
                    store,
                    lease.as_ref(),
                )
                .await
            }
        }
    }

    /// `source_remove`：在同一事务里撤销记录并登记清理义务（凭据槽位 + last-known-good
    /// 配置），随后在**同一份租约**内执行外部删除。
    ///
    /// - 记录不存在且没有遗留清理义务 → `RESOURCE_NOT_FOUND`（不为不存在的来源造行）。
    /// - 记录不存在但仍有清理义务 → 继续把义务清完（上一次删除的外部步骤失败了）。
    ///   这正是「删除失败后重试」的入口：重试不会因为记录已经消失而丢掉清理。
    /// - 外部删除失败 → 如实上抛错误；义务留在持久化队列里，可重试、不会丢失。
    /// - `credential_deleted` = 本次调用**实际**从系统凭据库删除了至少一条属于该来源的
    ///   引用；没有凭据、凭据本就不存在、或条目被守卫拦下时为 `false`。
    pub async fn remove_custom_source(
        &self,
        source_id: &str,
        store: &dyn CredentialStore,
        cache: &dyn SourceConfigCache,
    ) -> Result<SourceRemoveResult, AppError> {
        let lease = self.acquire_credential_lease().await?;
        let source_owned = source_id.to_owned();
        let plan = self
            .mutate_sources(move |payload, _persisted| {
                let Some(record) = payload
                    .custom_sources
                    .iter()
                    .find(|c| c.source_id == source_owned)
                else {
                    // 记录已不存在：不为它造行，也不在这里报错——调用方需要先看到
                    // 「还有没有清理义务没做完」。
                    let pending = has_pending_cleanup(payload, &source_owned);
                    return Ok(SourcesMutation::Unchanged(SourceRemovalPlan {
                        removed: false,
                        pending,
                    }));
                };
                // 只有确实属于该来源（种类 + 身份）的引用才进入删除队列：伪造、串源或
                // 跨 provider 的引用只随记录一起消失，绝不拿它去删系统凭据库里的任意条目。
                let credential = record.credential_ref.clone().filter(|reference| {
                    ref_belongs_to_source(record.kind, &source_owned, reference)
                });
                strip_custom_source(payload, &source_owned);
                if let Some(credential) = &credential {
                    enqueue_credential_cleanup(payload, credential, &source_owned)?;
                }
                enqueue_cache_cleanup(payload, &source_owned)?;
                Ok(SourcesMutation::Changed(SourceRemovalPlan {
                    removed: true,
                    pending: true,
                }))
            })
            .await?;

        if !plan.removed && !plan.pending {
            return Err(not_found("自定义来源不存在"));
        }

        // 记录已经按用户意图移除；外部清理失败时义务留在持久化队列里等待重试。
        let deleted = self
            .drain_cleanup_for_source(source_id, store, lease.as_ref())
            .await?;
        // 缓存清理失败不由本调用吞掉：`?` 让它如实向上返回（`DrainOutcome` 只在成功时
        // 用于分类，因此在这里被丢弃）。义务不丢：队列条目仍留在持久化队列里，由后续扫描
        // 重试。记录已经在同一个事务里被移除，因此这条路径不会被「来源仍然存在」的在用
        // 判定拦下（那道判定针对的是伪造或陈旧的队列条目）。
        let _ = self
            .drain_cache_cleanup_for_source(source_id, cache)
            .await?;
        Ok(SourceRemoveResult {
            schema_version: 1,
            source_id: source_id.to_owned(),
            credential_deleted: deleted > 0,
        })
    }

    /// 写入一条新凭据：分配新版本槽 → 暂存 → 外部写入 → 采纳。调用方必须已经持有租约。
    async fn write_custom_source_credential(
        &self,
        source_id: &str,
        secret: &str,
        store: &dyn CredentialStore,
        lease: &dyn SourceCredentialLeaseGuard,
    ) -> Result<(), AppError> {
        // 1. 读取来源身份：只用于分配槽位与提前拒绝（Feed/TVBox 没有凭据消费者）。
        //    真正的栅栏在事务内复核，这里读到的任何东西都不被信任。
        let payload = self.payload().await?;
        let Some(record) = payload
            .custom_sources
            .iter()
            .find(|c| c.source_id == source_id)
        else {
            return Err(not_found("自定义来源不存在"));
        };
        let fence = CredentialFence {
            kind: record.kind,
            previous_ref: record.credential_ref.clone(),
            previous_revision: payload.source_revisions.get(source_id).cloned(),
        };
        Self::credential_provider(fence.kind)
            .ok_or_else(|| invalid_argument("该来源不使用单独的访问凭据"))?;
        let slot = Self::allocate_credential_slot(fence.kind, source_id)?;
        let slot_reference = slot.as_str().to_owned();

        // 2. 暂存：**在外部写入之前**把槽位登记进持久化队列。中断（进程退出、外部写入
        //    失败、采纳冲突）之后，这条记录就是清理义务的所有者。容量先预留，避免
        //    「外部副作用已经发生、却发现登记不下义务」这种不可逆失败。
        let staged_at = UtcMillis::now();
        {
            let staged_slot = slot_reference.clone();
            let staged_source = source_id.to_owned();
            let staged_fence = &fence;
            self.mutate_sources(move |payload, _persisted| {
                check_fence(payload, &staged_source, staged_fence)?;
                // 过期槽位的退休是容量感知的：清理队列没有空位时它不会失败，而是把那些
                // 槽位继续留在暂存队列里（它们仍然是自己的清理所有者）。队列真的满时
                // 这次写入仍然 fail closed——由下面的预留检查给出稳定的
                // `SOURCE_CREDENTIAL_CLEANUP_FULL`，绝不「登记不下义务还去写外部存储」。
                retire_expired_stages(payload, staged_at);
                make_room_for_staging(payload)?;
                reserve_cleanup_capacity(payload)?;
                payload.staged_credentials.push(StagedSourceCredential {
                    reference: staged_slot,
                    source_id: staged_source,
                    staged_at_ms: staged_at.0,
                });
                Ok(SourcesMutation::Changed(()))
            })
            .await?;
        }

        // 3. 外部写入只针对这次分配的新槽位。
        let wrapped = SecretString::new(secret);
        if let Err(error) = store.set(&slot, &wrapped).await {
            return Err(self
                .abandon_staged_slot(source_id, &slot_reference, error, store, lease)
                .await);
        }

        // 4. 采纳：与暂存记录、来源身份和配置版本同一个事务。
        let adoption = self
            .mutate_sources(|payload, _persisted| {
                check_fence(payload, source_id, &fence)?;
                // 暂存记录必须仍是**我们这一条**：它被清理扫描回收过，就说明这次写入
                // 已经不再被追踪，绝不能采纳。
                let index = payload
                    .staged_credentials
                    .iter()
                    .position(|staged| {
                        staged.reference == slot_reference && staged.source_id == source_id
                    })
                    .ok_or_else(source_config_stale)?;
                payload.staged_credentials.remove(index);
                let retired = {
                    let record = payload
                        .custom_sources
                        .iter_mut()
                        .find(|c| c.source_id == source_id)
                        .ok_or_else(source_config_stale)?;
                    record.credential_ref.replace(slot_reference.clone())
                };
                // 退休的旧引用同样要过归属判定：畸形记录里可能写着别的来源、别的 provider
                // （AI / 云端凭据）的 target，绝不能借「轮换」把它删掉。它只随记录一起
                // 被替换掉，系统凭据库里的那一份保持不动。
                if let Some(retired) = retired
                    .as_deref()
                    .filter(|reference| ref_belongs_to_source(fence.kind, source_id, reference))
                {
                    enqueue_credential_cleanup(payload, retired, source_id)?;
                }
                // 凭据（有 / 无 / 换了值）决定请求是否会带认证：变更后旧观测描述的不再是
                // 当前来源。
                bump_source_revision(payload, source_id);
                Ok(SourcesMutation::Changed(retired))
            })
            .await;

        match adoption {
            Ok(retired) => {
                // 旧槽位已经进了持久化清理队列：这里尽力删除，失败留在队列里重试。
                // 新凭据已经是权威事实，本次写入不因为旧槽位的清理失败而失败——也绝不
                // 声称那个旧秘密已经被物理删除。
                if let Some(retired) = retired
                    .as_deref()
                    .filter(|reference| ref_belongs_to_source(fence.kind, source_id, reference))
                {
                    let _ = self
                        .drain_credential_entry(source_id, retired, store, lease)
                        .await;
                }
                Ok(())
            }
            Err(error) => Err(self
                .abandon_staged_slot(source_id, &slot_reference, error, store, lease)
                .await),
        }
    }

    /// 清除当前凭据：撤销引用与登记清理义务在**同一个事务**里完成，之后才做外部删除。
    /// 调用方必须已经持有租约。
    async fn clear_custom_source_credential(
        &self,
        source_id: &str,
        store: &dyn CredentialStore,
        lease: &dyn SourceCredentialLeaseGuard,
    ) -> Result<(), AppError> {
        let source_owned = source_id.to_owned();
        self.mutate_sources(move |payload, _persisted| {
            let index = payload
                .custom_sources
                .iter()
                .position(|c| c.source_id == source_owned)
                .ok_or_else(|| not_found("自定义来源不存在"))?;
            let kind = payload.custom_sources[index].kind;
            // 订阅源与 TVBox 配置来源没有凭据消费者：写进去只会变成一条不被消费的假能力。
            if Self::credential_provider(kind).is_none() {
                return Err(invalid_argument("该来源不使用单独的访问凭据"));
            }
            let Some(current) = payload.custom_sources[index].credential_ref.clone() else {
                // 本来就没有引用可撤销：不写库，也不产生新版本。
                return Ok(SourcesMutation::Unchanged(()));
            };
            payload.custom_sources[index].credential_ref = None;
            // 撤销同样要过归属判定：畸形记录里的外来 target 只被撤销引用，绝不被删除。
            if ref_belongs_to_source(kind, &source_owned, &current) {
                enqueue_credential_cleanup(payload, &current, &source_owned)?;
            }
            // 撤销本身（无论外部删除是否已经完成）都改变请求是否带认证：旧观测作废。
            bump_source_revision(payload, &source_owned);
            Ok(SourcesMutation::Changed(()))
        })
        .await?;
        // 撤销是权威事实；外部删除失败时如实上抛，义务留在持久化队列里可重试。
        self.drain_cleanup_for_source(source_id, store, lease)
            .await?;
        Ok(())
    }

    /// 放弃一次**尚未采纳**的版本槽：先把清理义务落库（暂存记录可能已被扫描回收，
    /// 那就重建一条待清理记录），再只删除自己的槽位。调用方必须已经持有租约。
    ///
    /// 绝不触碰上一个凭据，也没有任何「还原旧秘密」的分支：旧凭据的归属没有因为这次
    /// 失败的写入而改变。
    async fn abandon_staged_slot(
        &self,
        source_id: &str,
        slot_reference: &str,
        primary: AppError,
        store: &dyn CredentialStore,
        lease: &dyn SourceCredentialLeaseGuard,
    ) -> AppError {
        let slot_owned = slot_reference.to_owned();
        let source_owned = source_id.to_owned();
        let recorded = self
            .mutate_sources(move |payload, _persisted| {
                // 只移除**我们这一条**暂存记录（引用 + 归属来源都对得上）：按引用 `retain`
                // 会把一条引用相同、归属来源不同的伪造记录一并抹掉。
                payload.staged_credentials.retain(|staged| {
                    !(staged.reference == slot_owned && staged.source_id == source_owned)
                });
                // 重建而不是假定：暂存记录可能已经被清理扫描转走，迟到的外部写入
                // 仍然必须有一名清理所有者。
                enqueue_credential_cleanup(payload, &slot_owned, &source_owned)?;
                Ok(SourcesMutation::Changed(()))
            })
            .await;
        let cleanup = self
            .drain_credential_entry(source_id, slot_reference, store, lease)
            .await;
        let failure = match recorded {
            Ok(()) => primary,
            Err(record_error) => attach_compensation(primary, record_error),
        };
        match cleanup {
            Ok(_) => failure,
            Err(cleanup_error) => attach_compensation(failure, cleanup_error),
        }
    }

    /// 删除一条待清理引用；成功（或目标本就不存在）后把它移出队列。
    ///
    /// 返回 [`DrainOutcome`]：被守卫拦下的条目既不删除也不出队。守卫包含两层——
    /// **归属判定**（这条引用确实属于这个来源吗？队列条目是落库数据，可能被伪造或
    /// 手工改写，绝不能「解析出任何合法 CredentialRef 就删」）与**在用判定**（仍被来源
    /// 记录引用、或仍属于在途暂存槽）。调用方必须已经持有租约。
    async fn drain_credential_entry(
        &self,
        source_id: &str,
        reference: &str,
        store: &dyn CredentialStore,
        lease: &dyn SourceCredentialLeaseGuard,
    ) -> Result<DrainOutcome, AppError> {
        // 租约在调用方的栈帧里；这里把它显式收进来，避免出现「忘了持租约的外部删除」。
        let _lease = lease;
        let Some(kind) = kind_from_source_id(source_id) else {
            return Ok(DrainOutcome::Guarded);
        };
        if !ref_belongs_to_source(kind, source_id, reference) {
            return Ok(DrainOutcome::Guarded);
        }
        let target: CredentialRef = reference
            .parse()
            .map_err(|_| invalid_argument("凭据引用非法"))?;
        let payload = self.payload().await?;
        let live = payload
            .custom_sources
            .iter()
            .any(|record| record.credential_ref.as_deref() == Some(reference))
            || payload.staged_credentials.iter().any(|staged| {
                // 在途判定同样按**精确**身份：一条引用相同、归属来源不同的伪造暂存记录
                // 不得把本次这条合法义务永远挡在删除之外（反之亦然）。
                staged.reference == reference && staged.source_id == source_id
            });
        if live {
            return Ok(DrainOutcome::Guarded);
        }
        let deleted = store.delete(&target).await?;
        let reference_owned = reference.to_owned();
        let source_owned = source_id.to_owned();
        self.mutate_sources(move |payload, _persisted| {
            let before = payload.credential_cleanup.len();
            // 只出队**这一条**精确义务：伪造的「同引用、别的来源」条目必须原样留在队列里，
            // 绝不能跟着本次出队一起被抹掉。
            payload.credential_cleanup.retain(|entry| {
                !is_exact_cleanup_obligation(entry, &source_owned, &reference_owned)
            });
            Ok(if payload.credential_cleanup.len() == before {
                SourcesMutation::Unchanged(())
            } else {
                SourcesMutation::Changed(())
            })
        })
        .await?;
        Ok(if deleted {
            DrainOutcome::Deleted
        } else {
            DrainOutcome::Absent
        })
    }

    /// 处理某个来源的全部待清理凭据条目，返回**实际删除**的引用条数。
    /// 任一条外部删除失败即如实返回错误：已出队的条目已经处理完，其余留在队列里。
    async fn drain_cleanup_for_source(
        &self,
        source_id: &str,
        store: &dyn CredentialStore,
        lease: &dyn SourceCredentialLeaseGuard,
    ) -> Result<u32, AppError> {
        let references = self.cleanup_references_for_source(source_id).await?;
        let mut deleted = 0;
        for reference in references {
            if matches!(
                self.drain_credential_entry(source_id, &reference, store, lease)
                    .await?,
                DrainOutcome::Deleted
            ) {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// 处理某个来源的待清理配置缓存。
    ///
    /// 与凭据条目同一套**来源所有权**纪律：待清理队列是落库数据，可能被伪造或手工改写。
    /// 只要来源记录**仍然存在**，那份 last-known-good 配置就仍然属于一个有身份的来源，
    /// 绝不是「库里已经没有身份指向的原始配置」。这种情况下只返回
    /// [`DrainOutcome::Guarded`]：不删除、不出队，义务留在队列里，等来源真的被移除之后
    /// （那时它已经不再属于任何来源）再履行。
    async fn drain_cache_cleanup_for_source(
        &self,
        source_id: &str,
        cache: &dyn SourceConfigCache,
    ) -> Result<DrainOutcome, AppError> {
        let payload = self.payload().await?;
        if !payload.cache_cleanup.iter().any(|id| id == source_id) {
            return Ok(DrainOutcome::Absent);
        }
        if payload
            .custom_sources
            .iter()
            .any(|record| record.source_id == source_id)
        {
            return Ok(DrainOutcome::Guarded);
        }
        let deleted = cache.delete(source_id).await?;
        let source_owned = source_id.to_owned();
        self.mutate_sources(move |payload, _persisted| {
            let before = payload.cache_cleanup.len();
            payload.cache_cleanup.retain(|id| id != &source_owned);
            Ok(if payload.cache_cleanup.len() == before {
                SourcesMutation::Unchanged(())
            } else {
                SourcesMutation::Changed(())
            })
        })
        .await?;
        Ok(if deleted {
            DrainOutcome::Deleted
        } else {
            // 缓存本来就不存在：义务同样已经履行完毕（幂等），不是「没做」。
            DrainOutcome::Absent
        })
    }

    async fn cleanup_references_for_source(
        &self,
        source_id: &str,
    ) -> Result<Vec<String>, AppError> {
        Ok(self
            .payload()
            .await?
            .credential_cleanup
            .iter()
            .filter(|entry| entry.source_id == source_id)
            .map(|entry| entry.reference.clone())
            .collect())
    }

    /// 公平轮转：把这一轮**已经尝试过、但既没有删除也没有出队**的条目（守卫拦下 / 外部
    /// 删除失败）移到队列尾部，并保持它们彼此之间以及其余条目的相对顺序。
    ///
    /// 这是**重排**，不是遗忘也不是丢弃：条目连同引用与归属原样留在持久化队列里，只是
    /// 下一次扫描从别的条目开始。没有它，队首那一批长期不可结项的条目会把排在后面的合法
    /// 义务永远挡在 `take(BATCH)` 之外——队列看起来在跑，实际停滞。
    ///
    /// 重排在权威事务里完成（读 → 改 → 条件写），因此扫描期间并发新增或删除的条目不受
    /// 影响：不在这份名单里的条目原样保留；顺序没有实际变化时不写库。
    async fn rotate_attempted_cleanup_entries_to_tail(
        &self,
        attempted: Vec<(String, String)>,
    ) -> Result<(), AppError> {
        if attempted.is_empty() {
            return Ok(());
        }
        self.mutate_sources(move |payload, _persisted| {
            let before = std::mem::take(&mut payload.credential_cleanup);
            let mut untouched = Vec::with_capacity(before.len());
            let mut rotated = Vec::new();
            for entry in before.iter() {
                // 按**精确身份**（引用 + 归属来源）匹配：同名引用属于另一个来源时不轮转。
                let was_attempted = attempted.iter().any(|(source_id, reference)| {
                    entry.source_id == *source_id && entry.reference == *reference
                });
                if was_attempted {
                    rotated.push(entry.clone());
                } else {
                    untouched.push(entry.clone());
                }
            }
            let mut next = untouched;
            next.extend(rotated);
            if next == before {
                // 全队列都在这份名单里（或顺序本来就一致）：不写库。
                payload.credential_cleanup = before;
                return Ok(SourcesMutation::Unchanged(()));
            }
            payload.credential_cleanup = next;
            Ok(SourcesMutation::Changed(()))
        })
        .await
    }

    /// 组合根启动/命令接线用的重试入口：把超过有效期的暂存槽转入清理队列，然后按有界
    /// 批量处理凭据与配置缓存的清理义务。整个过程持有外部 I/O 租约。
    ///
    /// - 独占：持租约期间不存在第二个合规写者，因此这里回收的暂存槽只可能属于已经不会
    ///   再继续的写入；另一个写者会让本次调用直接拿到可重试的 `SOURCE_CREDENTIAL_BUSY`，
    ///   而不是去猜它是否还活着。
    /// - 幂等：重复调用不会多删任何东西（目标不存在也算处理完毕）。
    /// - 有界：一次调用最多处理 [`SOURCE_CREDENTIAL_CLEANUP_BATCH`] 条凭据与同样多的
    ///   缓存条目；按**身份**出队，因此并发的新的登记不会被这次扫描丢掉。
    /// - **容量感知的退休**：清理队列写满时，过期暂存槽不会被转走（它们继续留在暂存队列
    ///   里当自己的清理所有者），更不会让整次扫描失败——否则队列会在同一个点上重复同一次
    ///   失败，即使系统凭据库恢复也永远排不空。装不下的义务在下一次扫描空出位置之后退休。
    /// - **公平**：这一轮尝试过却没结项的条目（守卫拦下 / 外部失败）被轮转到队尾，好让
    ///   排在它们后面的合法义务也有机会被处理。
    /// - 保留失败：外部操作失败的条目留在队列里，`failed` 如实计数（不谎报成功）。
    /// - `now` 由调用方给出，让有效期判定可复现（也不需要 sleep）。
    pub async fn retry_pending_cleanup(
        &self,
        store: &dyn CredentialStore,
        cache: &dyn SourceConfigCache,
        now: UtcMillis,
    ) -> Result<SourceCredentialCleanupReport, AppError> {
        let lease = self.acquire_credential_lease().await?;

        // 1) 先把过期暂存槽转入清理队列。容量不足时这一步**不是**失败：转不下的过期槽位
        //    继续留在暂存队列里（它们本身就是自己的清理所有者），等下一次扫描腾出空位。
        let stages_retired = self
            .mutate_sources(move |payload, _persisted| {
                let retired = retire_expired_stages(payload, now);
                Ok(if retired > 0 {
                    SourcesMutation::Changed(retired)
                } else {
                    SourcesMutation::Unchanged(0)
                })
            })
            .await?;

        let mut report = SourceCredentialCleanupReport {
            stages_retired,
            ..SourceCredentialCleanupReport::default()
        };

        // 2) 有界地履行凭据清理义务。没能结项的条目在最后被轮转到队尾：队列里的每一条义务
        //    都留在原地，但下一条不会再被同一批长期不可结项的条目挡住。
        let payload = self.payload().await?;
        let entries: Vec<(String, String)> = payload
            .credential_cleanup
            .iter()
            .take(SOURCE_CREDENTIAL_CLEANUP_BATCH)
            .map(|entry| (entry.source_id.clone(), entry.reference.clone()))
            .collect();
        let mut unclaimed = Vec::new();
        for (source_id, reference) in entries {
            match self
                .drain_credential_entry(&source_id, &reference, store, lease.as_ref())
                .await
            {
                Ok(DrainOutcome::Deleted) => {
                    report.credentials_deleted += 1;
                    report.credentials_settled += 1;
                }
                Ok(DrainOutcome::Absent) => report.credentials_settled += 1,
                Ok(DrainOutcome::Guarded) => {
                    report.guarded += 1;
                    unclaimed.push((source_id, reference));
                }
                Err(_) => {
                    report.failed += 1;
                    unclaimed.push((source_id, reference));
                }
            }
        }
        self.rotate_attempted_cleanup_entries_to_tail(unclaimed)
            .await?;

        // 3) 有界地履行缓存清理义务。缓存队列的上限不超过一个批量，因此一轮就能把队列里
        //    每一条都试一遍，不需要轮转：来源仍然存在（在用判定）的条目留在队列里，等来源
        //    真的被移除之后再履行。
        let payload = self.payload().await?;
        let caches: Vec<String> = payload
            .cache_cleanup
            .iter()
            .take(SOURCE_CREDENTIAL_CLEANUP_BATCH)
            .cloned()
            .collect();
        for source_id in caches {
            match self.drain_cache_cleanup_for_source(&source_id, cache).await {
                Ok(DrainOutcome::Deleted) => report.caches_deleted += 1,
                Ok(DrainOutcome::Guarded) => report.caches_guarded += 1,
                Ok(DrainOutcome::Absent) => {}
                Err(_) => report.failed += 1,
            }
        }

        let payload = self.payload().await?;
        report.credentials_remaining = payload.credential_cleanup.len() as u32;
        report.caches_remaining = payload.cache_cleanup.len() as u32;
        report.staged_remaining = payload.staged_credentials.len() as u32;
        Ok(report)
    }
}

/// 一次来源移除的事实：记录是否真的被移除，以及该来源是否还有未完成的清理义务。
struct SourceRemovalPlan {
    removed: bool,
    pending: bool,
}

/// 某个来源是否还有未完成的清理义务（凭据或配置缓存）。
fn has_pending_cleanup(payload: &SourcesPayload, source_id: &str) -> bool {
    payload
        .credential_cleanup
        .iter()
        .any(|entry| entry.source_id == source_id)
        || payload.cache_cleanup.iter().any(|id| id == source_id)
}
