# 内置来源能力

栖阅的内置来源目录位于 [`后端/crates/haven-application/resources/builtin-sources.json`](后端/crates/haven-application/resources/builtin-sources.json)。下表是面向使用者的公开能力说明；代码中的来源 ID 是稳定标识，后续更新应同步修改来源目录和本文件。

## 能力边界

- 表中的“搜索”表示已经接入真实 Provider，可以返回受控的搜索结果；不是占位条目。
- Provider 只把安全的搜索/元数据投影交给应用，不把 Cookie、凭据、任意远端 Header 或完整来源响应交给前端。
- `cms10` 和 `m3u` 属于可包含多个站点/频道的聚合来源；其余来源是单一上游来源。聚合来源仍然只有一个受控端点配置入口。
- 海报可能缺失或被上游拒绝。此时由 Artwork Cache 和按内容类型选择的默认封面处理，不影响来源搜索结果。
- 上游服务可能限流、变更协议或暂时不可用；Haven 不承诺第三方 SLA。

## 来源清单

| sourceId / 名称 | 类型与模式 | 能力 | 默认与配置 | 上游与限制 |
| --- | --- | --- | --- | --- | --- |
| `tvmaze` · TVMaze | 影视 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [TVMaze Search API](https://api.tvmaze.com/search/shows)。受上游速率和字段变化影响 |
| `bangumi` · Bangumi | 影视、漫画 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [Bangumi API](https://api.bgm.tv/v0/search/subjects)。中文资料覆盖较好，字段由上游决定 |
| `anilist` · AniList | 影视、漫画 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [AniList GraphQL](https://graphql.anilist.co)。公共 API 可能限流 |
| `mangadex` · MangaDex | 漫画 · 单一来源 | 搜索 · 在线打开（章节） · 保存本地（CBZ） | 首次安装默认启用；无需账号或凭据 | [MangaDex API](https://api.mangadex.org/manga)。章节版权、地区和上游可用性由来源决定 |
| `itunes` · iTunes Search | 影视 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [iTunes Search API](https://itunes.apple.com/search)。结果受 Apple 目录和地区影响 |
| `gutenberg` · Project Gutenberg | 图书 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [Gutenberg OPDS Search](https://www.gutenberg.org/ebooks/search.opds)。书目资源许可和可用性以条目为准 |
| `archive` · Internet Archive | 图书 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [Advanced Search API](https://archive.org/advancedsearch.php)。条目权限和上游限流可能变化 |
| `opds_gutenberg` · 古腾堡计划（OPDS） | 图书 · 单一来源 | 搜索 · 在线打开（EPUB） · 保存本地（EPUB） | 首次使用可登记默认端点；无需账号或凭据 | [Gutenberg OPDS](https://m.gutenberg.org/ebooks.opds/)。在线正文受 32 MiB 读取上限约束，只接受符合安全策略的 EPUB 资源 |
| `arxiv` · arXiv | 报刊文章 · 单一来源 | 搜索 · 在线打开（服务端支持范围读取时） · 保存本地（PDF） | 首次安装默认启用；无需账号或凭据 | [arXiv API](https://export.arxiv.org/api/query)。遵循上游查询和速率限制 |
| `europepmc` · Europe PMC | 报刊文章 · 单一来源 | 搜索 · 在线打开（安全全文） · 保存本地（HTML） | 首次安装默认启用；无需账号或凭据 | [Europe PMC API](https://www.ebi.ac.uk/europepmc/webservices/rest/search)。仅开放获取全文可用 |
| `wikisource` · 中文维基文库 | 报刊文章 · 单一来源 | 搜索 · 在线打开（安全正文） · 保存本地（HTML） | 首次安装默认启用；无需账号或凭据 | [MediaWiki API](https://zh.wikisource.org/w/api.php)。版权和页面可用性以公版来源为准 |
| `crossref` · Crossref | 报刊文章 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [Crossref Works API](https://api.crossref.org/works)。完整正文不由本来源提供 |
| `openalex` · OpenAlex | 报刊文章 · 单一来源 | 搜索 | 默认停用；无需账号或凭据 | [OpenAlex Works API](https://api.openalex.org/works)。公共 API 可能限流 |
| `cms10` · 苹果 CMS V10 | 影视 · 聚合来源 | 搜索 · 在线播放 · 保存本地（按需） | 出厂预填并启用第一个受控预设；端点可在设置页切换，不需要账号才能使用预设 | 用户配置端点必须通过协议、Host、重定向和 SSRF 校验；上游站点内容和可用性不由 Haven 保证 |
| `m3u` · M3U 播放列表 | 影视 · 聚合来源 | 搜索 · 在线播放 | 默认停用；用户必须配置端点；本版本不自动收集账号或凭据 | M3U 地址由用户明确提供，并受出站安全策略、格式和重定向限制 |

## 在线正文与全文资源候选

下面记录内置正文访问使用的固定上游接口和安全边界。搜索卡片只携带 opaque candidate；点击“加入媒体库”只登记远端 Resource，不写正文文件。在线阅读由受控 Remote Session 提供；点击“下载到本地”后才由 DownloadTask 写入分类目录。

### 漫画在线正文

| Provider | 搜索 | 章节与页面 | 固定安全边界 | 当前结论 |
| --- | --- | --- | --- | --- |
| MangaDex | `https://api.mangadex.org/manga?title={query}&limit={limit}&includes[]=cover_art` | `https://api.mangadex.org/manga/{mangaId}/feed`；`https://api.mangadex.org/at-home/server/{chapterId}` 返回章节 hash、页名和页面主机；页面形如 `{baseUrl}/data/{hash}/{filename}` | API 只允许 `api.mangadex.org`；图片主机只允许 `*.mangadex.network` 的 label-aware allowlist，并重新执行 HTTPS、重定向和页面大小/页数限制；图片先校验魔数，再打包为本地 CBZ；不把 `baseUrl` 或图片 URL 交给前端 | 已接入搜索 → 加入媒体库仅登记 SourceObject → 章节在线按页读取；点击下载后才生成 CBZ；已删除/无页面章节会跳过，全部不可用时返回可重试错误 |
| Komga / Kavita（自托管） | 用户配置的 OPDS/API 端点 | OPDS/API 可提供漫画卷、章节和页面 | 仅作为用户自定义来源；端点、凭据和页面资源必须走现有 Source/Resource 安全策略，不进入内置默认源 | 适合后续扩展自托管漫画，不作为 v0.1.0-beta.1 公共默认来源 |

MangaDex 的章节 Feed 可能包含已删除或没有页面的章节；At-Home 返回 404 时应显示可重试的“章节不可用”，不能把它当成本地解析错误，也不能自动绕过来源策略。漫画内容的版权和地区可用性以来源和用户使用权为准。

### 报刊/文章全文

| Provider | 搜索 | 正文资源 | 固定安全边界 | 当前结论 |
| --- | --- | --- | --- | --- |
| arXiv | `https://export.arxiv.org/api/query?search_query=all:{query}&max_results={limit}` | `https://arxiv.org/pdf/{id}.pdf`（也可使用 `export.arxiv.org/pdf/{id}.pdf`） | 只允许 `export.arxiv.org`/`arxiv.org`；响应必须是 PDF、校验 `%PDF-` 魔数、大小上限和原子落盘；远端 ID 由后端生成，不能让前端提交任意 PDF URL | 已接入搜索 → 加入媒体库登记远端 PDF → 服务端支持 Range 时在线打开，否则提示下载；点击下载后才写入本地 PDF |
| Europe PMC（开放获取文章） | `https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=OPEN_ACCESS:Y&format=json` | `https://www.ebi.ac.uk/europepmc/webservices/rest/{pmcid}/fullTextXML` | 只接受 `OPEN_ACCESS:Y` 结果；XML 在后端清洗为安全 Article HTML，移除脚本、外链资源和任意媒体加载；响应大小、重定向和 MIME 受控 | 已接入搜索 → 加入媒体库登记 PMCID → 在线安全正文；点击下载后才写入 HTML 快照 |
| Wikisource（公版全文） | `https://zh.wikisource.org/w/api.php?action=query&list=search&srsearch={query}&format=json` | `https://zh.wikisource.org/w/api.php?action=parse&page={title}&prop=text&format=json` | 只允许 `zh.wikisource.org`；解析后的 HTML 由后端转换为纯文本段落，禁止图片、脚本、iframe、表单和外链资源 | 已接入搜索 → 加入媒体库登记页面标题 → 在线安全正文；点击下载后才写入 HTML 快照 |

这些候选都不携带 Cookie、Authorization、用户搜索正文之外的额外信息或任意远端 Header。正文始终先落本地，再复用现有 `haven-resource://session/*` / 受控页面协议，不恢复前端第三方图片或 PDF 直连。

## 隐私与请求边界

内置 metadata Provider 使用源码中固定的 HTTPS API。用户配置的 CMS10、OPDS 和 M3U 端点只在用户明确登记后使用；应用会重新执行协议、Host、DNS、重定向和响应大小校验。来源搜索结果只通过 Typed HavenClient 进入 UI，前端不获得任意网络代理能力。

来源返回的海报不会绕过 Artwork Cache。缓存失败、缺图或来源暂时不可用时，栖阅显示本地默认封面或已有的 stale 缓存；不会把远端 URL 当作前端图片直链。

## 导入、在线阅读与下载的边界（V02-ONLINE-READ-DOWNLOAD-001）

搜索结果的“加入媒体库”只登记作品元数据、来源去重身份和受控
`SourceObject`。它不会创建正文文件、`DownloadTask` 或 `Offline Resource`，也不会
把远端 URL 交给前端。

只有用户明确点击“下载到本地”后，统一的 `DownloadTask` 才会调用对应 Provider
获取完整对象，先写入受控临时目录并校验大小、MIME 和格式，再原子移动到：

```text
下载/栖阅/books       # EPUB 等图书文件
下载/栖阅/comics      # MangaDex 章节 CBZ
下载/栖阅/articles    # PDF 或清洗后的 HTML 文章快照
```

当前代码已接入固定来源的 SourceObject、Remote Session 和远端下载分支；OPDS/Gutenberg
EPUB 可在 32 MiB 限制内在线打开，超限或来源不可用时明确提示下载后阅读。远端 Provider
当前按受控完整对象获取后切片，不把这条路径描述为上游 HTTP Range 或长期断点续传能力。

本文件记录的是代码与契约边界，不替代真实网络和 Windows custom-protocol EXE 验收。
MangaDex、Europe PMC、Wikisource、arXiv、OPDS/Gutenberg 的真实网络、关闭/重启、断网
和本地目录证据仍需在独立桌面构建中逐项留下；自动化测试通过不等于这些证据已经取得。
