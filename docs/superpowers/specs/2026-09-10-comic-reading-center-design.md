# 漫画追更中心设计规格

- 日期：2026-09-10
- 状态：设计规格已获用户批准，等待实现计划
- 功能范围：漫画 Work、Edition、章节目录、Reader 章节切换与跨来源进度连续性
- 第一实现切片：Comic Reader 真实章节抽屉与章节切换
- 第一版来源：MangaDex
- 设计原则：后端生成事实，前端消费事实；远端 ID 保留，但不把远端 ID 当作永久进度身份

## 1. 设计结论

栖阅新增“漫画追更中心”，以 Work 级章节目录作为详情页和 Reader 的共同数据来源。

第一阶段先在现有 Comic Reader 内接入：

- 本地已登记章节目录；
- Edition 筛选；
- 当前章节高亮；
- 章节进度；
- 上一章和下一章；
- 用户主动刷新；
- 章节状态；
- 不同来源章节的进度连续性；
- 页面变化时的最佳努力迁移结果。

本阶段不重新设计 Reader 的视觉布局，不引入新的 Provider 目录事实，不让前端根据章节号、标题或页数自行关联来源。

整体数据流：

    MangaDex 远端目录
            |
            v
    后端 Source Adapter
            |
            v
    来源身份、章节目录、匹配证据
            |
            v
    Work 级章节聚合用例
            |
       +----+----+
       |         |
       v         v
    Media Detail  Comic Reader
       |         |
       +----+----+
            |
            v
    本地 MediaItem 路由与统一阅读进度

## 2. 参考项目的使用边界

本设计吸收前期参考报告中对以下项目的观察：

- Suwayomi Server：
  - Provider、远端作品和远端章节身份保持来源边界；
  - 章节目录由后端获取、归一化和刷新；
  - Provider 异常不能直接清空本地用户数据；
  - 来源身份不是本地作品身份的唯一组成部分。
- reader_ws：
  - Reader 消费归一化后的章节和页面清单；
  - 页面导航状态和阅读进度属于本地阅读层；
  - Reader 不应该把 Provider 的原始对象、URL 或内部定位字段当作业务模型。

这些项目只作为设计参考，不直接复制其数据库结构、前端路由或 Provider 实现。

## 3. 功能范围与非目标

### 3.1 第一阶段包含

- 一个 Work 关联多个来源作品；
- 一个 Work 下拥有多个 Edition；
- Edition 区分语言、翻译线、真实扫描组和彩色/黑白版本；
- MangaDex 章节目录的本地读取；
- 用户主动刷新 MangaDex 目录；
- 章节目录持久化；
- Work 级章节聚合；
- Comic Reader 章节抽屉；
- Media Detail 和 Reader 使用同一份章节事实；
- 章节只通过本地 MediaItem ID 导航；
- 不同远端章节 ID 的进度收敛；
- 页面插入、删除、重排和数量变化时的最佳努力迁移；
- 后端返回迁移策略、置信度、证据和撤销信息；
- Available、TemporarilyUnavailable、ExternalOnly、Unknown、Missing、RefreshFailed 和 Truncated 状态；
- 低置信度迁移的可解释提示和可撤销快照。

### 3.2 第一阶段明确不包含

- Komga；
- Kavita；
- OCR；
- 翻译；
- AI 内容理解；
- 自动订阅调度；
- 系统通知；
- 后台定时刷新；
- 自动下载；
- 多 Provider 同时接入；
- 前端自定义来源匹配；
- 直接打开任意 Provider URL；
- 把请求头、Cookie、grant、CDN 签名或 Provider 内部字段交给前端；
- 重做 Reader 的视觉设计；
- 将跨语言、跨翻译线、跨扫描组内容默认归并为同一阅读进度。

## 4. 领域模型

逻辑关系如下：

    Work
     ├─ Work Source References
     │   ├─ MangaDex / remote work A
     │   └─ MangaDex / remote work B
     │
     └─ Edition
         ├─ Edition Profile
         │   ├─ language
         │   ├─ translation_line
         │   ├─ scan_group
         │   └─ color_mode
         │
         └─ Chapter Continuity
             ├─ MediaItem
             │   ├─ Resource
             │   ├─ Page Manifest
             │   ├─ Progress
             │   └─ ChapterSourceRef[]
             │
             └─ Match Evidence / Migration Receipt

各对象职责：

- Work：逻辑上的作品，不代表某一个 Provider 的远端作品。
- Work Source Reference：Work 与某个来源远端作品的关系。
- Edition：作品的内容版本。
- MediaItem：本地可阅读的章节内容实例。
- ChapterSourceRef：远端章节身份的受控记录。
- Chapter Continuity：不同来源章节在本地阅读层的连续性。
- Progress：属于稳定的本地阅读连续性，不永久属于某一个远端章节 ID。
- Match Evidence：后端生成的来源匹配事实和证据。
- Migration Receipt：一次进度迁移的可审计、可撤销记录。

章节来源身份仍采用：

    source_key + remote_work_id + remote_chapter_id

例如：

    mangadex + work-abc + chapter-001
    mangadex + work-def + chapter-999

两个远端章节 ID 可以在证据充分时绑定到同一个本地 MediaItem 或同一个 Chapter Continuity，但所有原始来源身份必须保留。

同一个来源身份只能有一个明确的本地归属。如果出现冲突，返回 SOURCE_REF_CONFLICT，不静默覆盖已有绑定。

## 5. Edition Profile 方案

Edition Profile 由以下四个维度组成：

    language
    translation_line
    scan_group
    color_mode

### 5.1 language

使用归一化语言标识，例如：

- zh-Hans；
- zh-Hant；
- en；
- ja；
- ko。

显示名称不参与身份比较，内部使用归一化后的稳定值。

### 5.2 translation_line

translation_line 表示翻译生产线或翻译版本。

它和 scan_group 不同：

- translation_line：负责翻译和本地化的一条内容生产线；
- scan_group：负责扫描、整理和发布图像的一组来源身份。

MangaDex 当前无法始终可靠提供翻译线时，记录为 Unknown。

### 5.3 scan_group

scan_group 表示真实内容扫描组。

如果 MangaDex 提供稳定的扫描组身份，则保存稳定 key 和显示 label：

    scan_group_key
    scan_group_label

显示名称变化不应自动创造新的 Edition。

### 5.4 color_mode

color_mode 使用受控枚举：

- Color；
- Grayscale；
- Mixed；
- Unknown。

MangaDex 当前无法可靠判断时记录为 Unknown。

### 5.5 Edition 比较规则

Edition 比较结果分为三类：

| 结果 | 判定规则 | 行为 |
| --- | --- | --- |
| Same | 每个维度都相同，或者双方该维度都是 Unknown | 可以归入同一个 Edition |
| Distinct | 任一维度双方都已知且值不同 | 必须拆分为不同 Edition |
| Unresolved | 至少一个维度一方已知、另一方是 Unknown | 不能自动确认为同一 Edition |

示例：

- language=zh-Hans、scan_group=Group-A 与相同 Profile 可以归入同一 Edition；
- scan_group=Group-A 与 scan_group=Group-B 必须拆分；
- scan_group=Group-A 与 scan_group=Unknown 不能直接合并；
- 双方 translation_line 都是 Unknown 时可以共享该维度，不因此制造多个 Edition。

Unknown 不是 wildcard。

对于 Known 与 Unknown 的组合，系统应保留为待确认来源或待确认版本，不得为了减少列表数量而伪造确定的 Edition 归属。

### 5.6 MirrorLabel 不参与 Edition 身份

镜像站、搬运站和代理来源只作为来源事实展示：

- source_key；
- source_label；
- mirror_label；
- last_seen_at；
- availability。

MirrorLabel 不单独创建 Edition。

只有真实内容属性不同，才拆分 Edition。

## 6. Work 级章节聚合目录

新增核心读取能力为 Work 级章节目录聚合用例，概念名称为：

    comic_work_chapter_catalog_get

该用例由后端完成：

1. 读取 Work 下登记的来源作品；
2. 读取各来源最近一次本地章节目录；
3. 读取 Edition 和 MediaItem；
4. 读取每个章节的 ChapterSourceRef；
5. 读取章节进度；
6. 读取来源可用性；
7. 读取后端生成的匹配证据；
8. 计算统一的章节顺序；
9. 返回详情页和 Reader 都能消费的聚合 DTO。

聚合目录至少包含：

- Work ID；
- Edition ID；
- Edition Profile；
- MediaItem ID；
- Chapter Continuity ID；
- 章节号；
- 卷号；
- 标题；
- 发布时间；
- 页数；
- 当前进度；
- 章节有效状态；
- 来源列表；
- 每个来源的状态；
- 每个来源的最近观察时间；
- 匹配结果；
- 匹配置信度；
- 匹配证据；
- 刷新状态；
- 是否截断；
- 是否可以打开；
- 后端计算的顺序信息。

章节号、标题、页数和发布时间是展示或后端匹配证据，不是前端写入来源关联的依据。

### 6.1 章节排序

章节顺序由后端返回，前端不得重新排序。

排序优先级：

1. 归一化后的卷号；
2. 归一化后的章节号；
3. Provider 稳定顺序；
4. 发布时间；
5. 稳定的本地排序键。

番外、无编号章节和特殊章节需要保留稳定顺序，不因标题字典序变化而随机移动。

上一章、下一章也由后端返回本地 MediaItem ID，前端不得通过章节号自行推导。

## 7. 目录读取与刷新

### 7.1 Reader 打开时

Reader 打开本地 MediaItem 后：

1. 根据 mediaItemId 获取章节上下文；
2. 后端解析所属 Work；
3. 后端解析所属 Edition；
4. 读取 Work 级聚合章节目录；
5. 返回当前章节；
6. 返回当前章节进度；
7. 返回上一章和下一章的本地 MediaItem ID；
8. 返回目录新鲜度和刷新状态。

Reader 不直接请求 MangaDex。

### 7.2 没有本地目录时

显示：

    尚未同步章节目录

提供用户主动操作：

    手动刷新

打开 Reader 不自动触发网络刷新。

### 7.3 用户手动刷新时

刷新流程：

    Reader 或 Media Detail 刷新按钮
        |
        v
    Feature API
        |
        v
    Typed HavenClient
        |
        v
    Tauri Command
        |
        v
    Application Service
        |
        v
    MangaDex Source Adapter
        |
        v
    SQLite 事务写入目录和来源状态

刷新需要记录每个来源的结果：

- 成功；
- 暂时不可用；
- 外部可用但本地不可读；
- 解析未知；
- 目录截断；
- 请求失败。

刷新失败时保留最近一次有效目录，不清空本地章节。

### 7.4 Truncated 目录

如果目录是部分结果、分页不完整或达到安全上限，记录：

    truncated = true

此时：

- 不把未出现的旧章节标记为 Missing；
- 保留旧章节；
- UI 显示目录不完整；
- 允许用户再次刷新；
- 记录刷新范围和来源状态。

只有完整刷新才能推断某个旧章节是否进入 Missing。

## 8. Comic Reader 章节抽屉

现有页面/章节抽屉和视觉结构继续复用。

生产环境需要正式启用：

- 页面 Tab；
- 章节 Tab。

第一阶段只做真实数据接入，不改变现有单页、双页、RTL/LTR、条漫、页面预加载、书签和页面资源池设计。

### 8.1 章节条目

章节条目显示：

- 章节号；
- 卷号；
- 标题；
- Edition 简要标识；
- 页数；
- 当前进度；
- 可用性；
- 来源数量；
- 当前章节高亮；
- 目录新鲜度提示；
- 低置信度迁移提示（如果本次切换触发迁移）。

章节条目的顺序、进度和状态全部来自后端 DTO。

### 8.2 Edition 筛选

当一个 Work 有多个 Edition 时，章节抽屉提供 Edition 筛选：

- 全部版本；
- 中文 / Group A / 黑白；
- 中文 / Group B / 彩色；
- 英文 / Group C / 黑白；
- 待确认版本。

打开 Reader 时优先显示当前章节所属 Edition。

切换 Edition 只使用后端返回的 MediaItem ID，不根据章节标题或章节号寻找目标。

### 8.3 点击章节

| 章节状态 | 行为 |
| --- | --- |
| Available | 导航到 /comic/:mediaItemId |
| ExternalOnly | 显示仅外部可用，不直接打开任意 URL |
| TemporarilyUnavailable | 显示暂不可用，提供刷新或重试入口 |
| Unknown | 显示状态未确认，不标记为可读 |
| Missing | 若本地资源仍可用则允许打开，否则显示可能失效提示 |

## 9. Media Detail 与 Reader 的共享数据流

Media Detail 可以继续使用通用 Work Header 和 Edition List，但漫画章节列表必须使用漫画 Work 级聚合目录。

目标结构：

    Media Detail
     ├─ Work Header
     ├─ Edition List
     └─ Comic Work Chapter Catalog
           ├─ Chapter 1 -> MediaItem ID
           ├─ Chapter 2 -> MediaItem ID
           └─ Chapter 3 -> MediaItem ID

    Comic Reader
     └─ 同一个 Comic Work Chapter Catalog

两者共享：

- Work；
- Edition；
- 章节顺序；
- 章节身份；
- 章节状态；
- 来源列表；
- 进度；
- 匹配证据；
- 刷新状态。

两者不共享：

- Reader 页面显示状态；
- 页面预加载状态；
- 单页/双页模式；
- RTL/LTR；
- Reader 局部 UI 状态。

两个页面的点击行为统一为：

    点击章节
        |
        v
    后端返回 mediaItemId
        |
        v
    /comic/:mediaItemId

Media Detail 不得自行从章节号拼接路由，也不得把 Provider URL 传递给 Reader。

## 10. 不同远端章节 ID 的进度收敛

远端章节 ID 只是来源身份，不是最终的本地阅读身份。

例如：

    MangaDex / chapter-A
    MangaDex / chapter-B

只要后端证据表明二者内容相同或高度相似，就可以共享本地阅读连续性。

### 10.1 Chapter Continuity

建议使用稳定的本地 Chapter Continuity：

- MediaItem 是具体的本地内容实例；
- ChapterSourceRef 是远端来源身份；
- Chapter Continuity 是不同来源章节在本地阅读层的连续性；
- Progress 绑定到稳定的本地 Progress Subject；
- Reader 路由仍使用 mediaItemId；
- 后端在读取和保存进度时解析到对应的稳定连续性。

这样可以避免：

    远端 ID 变化
        |
        v
    生成新的本地 MediaItem
        |
        v
    用户进度永久分裂

### 10.2 自动归并边界

可以自动建立同一连续性的条件：

- 属于同一个 Work；
- 属于同一个 Edition；
- Edition Profile 没有已知冲突；
- 匹配证据达到对应策略要求。

默认不自动归并的条件：

- 已知语言不同；
- 已知翻译线不同；
- 已知扫描组不同；
- 已知彩色/黑白模式不同；
- 明确属于不同内容版本。

结论：

    不同来源，不等于一定不同进度。
    已知不同 Edition，默认保持不同进度。

### 10.3 匹配策略

匹配由后端生成，前端只能读取。

| 策略 | 条件 | 结果 | 置信度 |
| --- | --- | --- | --- |
| RemoteIdentity | 来源和远端章节 ID 完全相同 | 直接复用现有身份 | High |
| AuthoritativeContentKey | 后端生成的权威内容 key 相同 | 合并来源身份 | High |
| FullPageIdentitySet | 页面身份集合完全匹配 | 归入同一连续性 | High 或 Medium |
| PartialPageIdentity | 页面部分重合且章节元数据一致 | 允许最佳努力迁移 | Medium |
| MetadataCandidate | 章节号、卷号、标题、时间等形成候选 | 保留候选，允许迁移预览 | Low |
| NoMatch | 没有足够证据 | 保持独立 | None |

标题、章节号、页数和发布时间可以参与后端证据计算，但绝不能由前端直接写入来源匹配结果。

### 10.4 已产生两个 MediaItem 时

如果两个远端章节已经分别生成本地 MediaItem：

1. 后端计算匹配证据；
2. 判断是否属于同一 Chapter Continuity；
3. 证据足够时建立本地连续性关系；
4. 选择稳定的 Progress Subject；
5. 迁移或归并进度；
6. 保留两个 MediaItem；
7. 保留所有来源身份；
8. 保存迁移快照；
9. 后续 Reader 读取统一进度。

不会删除旧章节，也不会删除旧历史记录。

## 11. 页面变化时的最佳努力迁移

页面数量或顺序变化时，不采用“无法完全证明安全就拒绝迁移”的硬拒绝策略。

迁移服务始终尽量产生目标位置，并返回结果和置信度。

页面定位顺序：

    StableKey
        |
        v
    ContentFingerprint
        |
        v
    NearestSurvivingPage
        |
        v
    ProportionalFallback
        |
        v
    NoTarget

### 11.1 StableKey

如果目标页面仍然拥有相同稳定身份，直接映射。

即使页面顺序变化，也按页面身份重新计算目标索引。

### 11.2 ContentFingerprint

如果稳定身份不可用，但页面内容指纹一致，则定位到对应页面。

### 11.3 NearestSurvivingPage

如果当前页面被删除，根据前后相邻页面身份和顺序寻找最近的仍存在页面。

例如：

    旧页面：A B C D E
    当前页：C
    新页面：A B X D E

后端根据邻接证据选择 X 或 D，不固定使用页码偏移。

### 11.4 ProportionalFallback

如果页面身份全部缺失，但页面总数发生变化，则按阅读比例迁移：

    旧位置比例 = 当前页索引 / 旧页面数量
    新位置 = 旧位置比例 × 新页面数量

这是低置信度策略，但允许执行。

### 11.5 NoTarget

只有以下情况可以返回 NoTarget：

- 目标页面清单为空；
- 目标页面清单损坏；
- 目标内容无法读取；
- 页面身份和内容都不可用，且比例也无法计算。

普通的插页、删页、重排和页数变化不能直接被判定为“不迁移”。

### 11.6 迁移结果

每次迁移返回：

- migration_id；
- source_media_item_id；
- target_media_item_id；
- strategy；
- confidence；
- evidence；
- source_progress_snapshot；
- target_progress_before；
- target_progress_after；
- page_mapping；
- algorithm_version；
- created_at；
- undoable。

置信度为：

- High；
- Medium；
- Low。

Low 不是失败，只代表目标页位置是最佳努力推断，UI 必须让用户能够看到这一事实。

### 11.7 目标已有进度

默认不覆盖目标进度。

如果目标已有进度，返回双方状态：

    来源进度：第 12 页 / 40 页
    目标进度：第 5 页 / 38 页

默认行为：

    保留目标进度

用户可以明确选择：

    使用来源进度

只有明确用户动作才允许覆盖目标，并保存可撤销快照。

前端不自行构造匹配结果，只调用后端提供的明确迁移动作。

### 11.8 撤销规则

撤销使用迁移记录：

    comic_progress_migration_undo(migrationId)

撤销必须经过版本检查：

- 目标进度仍是迁移后的版本时，可以恢复；
- 用户已经继续阅读、Revision 发生变化时，不能静默覆盖；
- 发生冲突时返回冲突结果，由用户明确选择；
- 不使用强制覆盖绕过 Revision。

## 12. 匹配证据持久化

后端应持久化匹配事实或能够稳定重建的匹配记录。

记录至少包含：

- source_media_item_id；
- target_media_item_id；
- source_ref_ids；
- match_strategy；
- confidence；
- evidence；
- algorithm_version；
- created_at；
- relationship；
- undoable。

证据可以包含：

    authoritative_content_key_equal = true
    stable_page_overlap = 0.92
    normalized_chapter_number_equal = true
    normalized_volume_equal = true
    page_count_delta = 2

证据中禁止保存：

- Provider 请求头；
- Cookie；
- 临时授权 grant；
- CDN 签名 URL；
- 未验证的任意远端 URL；
- 前端自行生成的匹配结论。

## 13. 状态语义

章节目录和来源目录需要区分：

| 状态 | 含义 | Reader 行为 |
| --- | --- | --- |
| Available | 存在可读取的本地资源 | 可以打开 |
| TemporarilyUnavailable | 来源曾确认存在，但当前暂不可获取 | 不直接打开，显示刷新/重试 |
| ExternalOnly | 外部来源存在，但本地没有受控资源 | 显示仅外部可用 |
| Unknown | 来源状态无法确认或解析不足 | 不标记为已确认可读 |
| Missing | 完整刷新中未再次发现 | 保留本地记录并标记可能失效 |
| RefreshFailed | 本次刷新失败 | 保留上次目录并显示失败原因 |
| Truncated | 本次目录不完整 | 不得据此推断 Missing |

聚合状态规则：

- 任一来源可打开，则章节整体可打开；
- 所有来源均为 ExternalOnly，则章节整体为仅外部可用；
- 所有来源暂不可用，则章节整体为暂不可用；
- 刷新不完整时，不能据此判定 Missing；
- 一个来源失败不能删除另一个来源的有效章节。

## 14. IPC、资源和安全边界

继续遵循现有架构：

    React UI
      -> Feature Hook / Action
      -> Feature API
      -> Typed HavenClient
      -> Tauri Command
      -> Application
      -> Domain / Port
      -> Infrastructure

禁止：

- React 组件直接调用 Tauri IPC；
- React 组件直接访问 SQLite；
- React 组件直接访问 MangaDex；
- 前端自行拼接 Provider URL；
- 前端持有请求头、Cookie 或 grant；
- 前端根据标题、页数、章节号写入匹配结果；
- 前端直接操作 SourceObject；
- 前端直接读取数据库实体。

Reader 通过受控 Session 和资源协议读取页面，例如：

    haven-resource://session/<uuid>

章节路由只使用：

    /comic/:mediaItemId

不使用 Provider 和远端章节 ID 作为路由主键，也不把 MangaDex URL 放入路由。

后端在打开章节时重新检查：

- MediaItem 是否存在；
- 是否属于合法 Work；
- 是否属于合法 Edition；
- Resource 是否可用；
- Session 是否仍有效；
- 资源大小是否满足限制；
- 页面 Manifest 是否完整；
- 请求是否属于受控资源协议。

## 15. 计划修改边界

本阶段写入设计文档，不修改实现代码。

进入实现后，预计允许修改的区域：

### 前端

- 前端/app/src/features/comic/pages/ComicReaderPage.tsx；
- 前端/app/src/features/comic/ipc/comic-chapter-catalog-gateway.ts；
- 前端/app/src/features/comic/api/；
- 前端/app/src/features/comic/hooks/；
- 前端/app/src/features/media/pages/MediaDetailPage.tsx。

### Application

- 后端/crates/haven-application/src/services/comic_catalog.rs；
- 后端/crates/haven-application/src/services/source_import.rs；
- 后端/crates/haven-application/src/services/comic_progress_migration.rs。

### Domain

- 后端/crates/haven-domain/src/comic_identity.rs；
- 后端/crates/haven-domain/src/comic_catalog.rs；
- 后端/crates/haven-domain/src/comic_page_identity.rs；
- 后端/crates/haven-domain/src/contracts.rs。

### Infrastructure

- 后端/crates/haven-infrastructure/src/db/repos/comic_identity.rs；
- 后端/crates/haven-infrastructure/src/db/repos/comic_progress_migrations.rs；
- 后端/migrations/。

### Tauri

- src-tauri/src/commands/comic.rs。

生成 wire 只通过现有生成流程更新，不手工编辑生成文件。

明确不修改：

    docs/reviews/

Reader 的视觉设计、阅读模式、页面预加载、书签和资源池保持不变；第一阶段只把生产环境中的 Demo 章节事实替换为后端真实目录。

## 16. 测试矩阵

### 16.1 Domain 单元测试

覆盖：

- 多来源章节绑定同一个 MediaItem；
- 同一个来源身份冲突；
- Edition 四个维度的比较；
- Unknown 与 Unknown；
- Known 与 Unknown；
- 已知字段冲突；
- MirrorLabel 不拆 Edition；
- 不同语言不共享连续性；
- 不同扫描组不自动共享连续性；
- 不同远端章节 ID 通过权威内容 key 收敛；
- 页面插入；
- 页面删除；
- 页面重排；
- 页面数量变化；
- 当前页删除；
- 比例回退；
- 无目标页面；
- 目标已有进度不覆盖；
- 撤销；
- Revision 冲突。

### 16.2 Application 与 SQLite 集成测试

覆盖：

- Work 级章节聚合；
- 多来源目录合并；
- 来源状态聚合；
- 完整刷新产生 Missing；
- 截断刷新不产生 Missing；
- 单来源刷新失败不清空已有目录；
- 来源章节归并；
- 匹配证据持久化；
- 迁移记录持久化；
- 进度和迁移快照在一个事务内写入；
- 撤销迁移；
- 并发 Revision 冲突。

### 16.3 Frontend 测试

覆盖：

- Reader 章节 Tab；
- 当前章节高亮；
- 章节进度条；
- Edition 筛选；
- Available 点击；
- ExternalOnly 展示；
- TemporarilyUnavailable 展示；
- Unknown 展示；
- Missing 展示；
- Truncated 提示；
- 手动刷新；
- 无目录空状态；
- 前端只使用 mediaItemId 导航；
- 前端不生成匹配证据；
- Media Detail 和 Reader 使用同一 DTO；
- 生产环境不再使用 DEMO_CHAPTERS 作为章节事实。

### 16.4 Tauri / Windows 验收

需要单独验证：

- 真实 Tauri command；
- 真实 MangaDex 目录刷新；
- 真实资源 Session；
- haven-resource://session/<uuid>；
- WebView2 下章节抽屉；
- Reader 切换章节；
- 刷新失败和恢复；
- 低置信度迁移提示；
- 撤销迁移；
- 目标已有进度时不被静默覆盖。

TypeScript 检查、Rust 测试和 CI 通过，不能替代真实 Windows/WebView2 Reader 验收。

## 17. 实施顺序

书面规格完成后，进入实现计划阶段，再根据计划执行以下大致顺序：

1. 重新核对 main、origin/main、工作树和 Worktree；
2. 从正确远端基线建立新的 codex/ 功能分支；
3. 先保留设计文档的独立提交；
4. 生成并审查实现计划；
5. 第一切片实现 Work 级聚合目录和 Reader 章节抽屉；
6. 第二切片接入 Media Detail；
7. 第三切片完善跨来源连续性、最佳努力迁移和撤销；
8. 独立验证生成 wire、前端、Rust、SQLite 和真实 Tauri；
9. 所有验证通过后，按既定 Git 流程合并和清理分支。

本设计不改变以下已经确认的原则：

    来源 ID 保留
    Edition 不误合并
    相同内容的进度可以收敛
    页面变化允许最佳努力迁移
    不确定性必须可见、可解释、可撤销
    前端只消费后端事实
    用户主动刷新，不做后台订阅调度

## 18. 规格状态

本文件对应用户已经批准的设计规格。

下一步应调用 writing-plans 流程生成实现计划，不应在没有实现计划的情况下直接开始编码。
