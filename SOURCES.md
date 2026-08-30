# 内置来源能力

栖阅的内置来源目录位于 [`后端/crates/haven-application/resources/builtin-sources.json`](后端/crates/haven-application/resources/builtin-sources.json)。下表是面向使用者的公开能力说明；代码中的来源 ID 是稳定标识，后续更新应同步修改来源目录和本文件。

## 能力边界

- 表中的“搜索”表示已经接入真实 Provider，可以返回受控的搜索结果；不是占位条目。
- Provider 只把安全的搜索/元数据投影交给应用，不把 Cookie、凭据、任意远端 Header 或完整来源响应交给前端。
- `cms10` 和 `m3u` 属于可包含多个站点/频道的聚合来源；其余来源是单一上游来源。聚合来源仍然只有一个受控端点配置入口。
- 海报可能缺失或被上游拒绝。此时由 Artwork Cache 和按内容类型选择的默认封面处理，不影响来源搜索结果。
- 上游服务可能限流、变更协议或暂时不可用；Haven 不承诺第三方 SLA。

## 来源清单

| sourceId / 名称 | 类型与模式 | 搜索与元数据 | 播放 / 下载 | 默认与配置 | 上游与限制 |
| --- | --- | --- | --- | --- | --- |
| `tvmaze` · TVMaze | 影视 · 单一来源 | 真实剧集、季和作品元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [TVMaze Search API](https://api.tvmaze.com/search/shows)。受上游速率和字段变化影响 |
| `bangumi` · Bangumi | 影视、漫画 · 单一来源 | 真实动画、漫画和番组元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [Bangumi API](https://api.bgm.tv/v0/search/subjects)。中文资料覆盖较好，字段由上游决定 |
| `anilist` · AniList | 影视、漫画 · 单一来源 | 真实动画和漫画元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [AniList GraphQL](https://graphql.anilist.co)。公共 API 可能限流 |
| `mangadex` · MangaDex | 漫画 · 单一来源 | 真实漫画标题和作品元数据搜索 | v0.1.0 只使用元数据搜索，不声明漫画正文下载 | 默认停用；无需账号或凭据 | [MangaDex API](https://api.mangadex.org/manga)。服务可用性和地区访问由上游决定 |
| `itunes` · iTunes Search | 影视 · 单一来源 | 真实剧集、季和作品元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [iTunes Search API](https://itunes.apple.com/search)。结果受 Apple 目录和地区影响 |
| `gutenberg` · Project Gutenberg | 图书 · 单一来源 | 真实公共领域图书目录搜索 | 本来源只提供元数据；允许下载由 `opds_gutenberg` 负责 | 默认停用；无需账号或凭据 | [Gutenberg OPDS Search](https://www.gutenberg.org/ebooks/search.opds)。书目资源许可和可用性以条目为准 |
| `archive` · Internet Archive | 图书 · 单一来源 | 真实文本类图书资料搜索 | 本版本不把任意 Archive 下载权限当作保证 | 默认停用；无需账号或凭据 | [Advanced Search API](https://archive.org/advancedsearch.php)。条目权限和上游限流可能变化 |
| `opds_gutenberg` · 古腾堡计划（OPDS） | 图书 · 单一来源 | 真实 OPDS 搜索和浏览 | 对 OPDS 明确允许的 EPUB 提供受控下载 | 首次使用可登记默认端点；无需账号或凭据 | [Gutenberg OPDS](https://m.gutenberg.org/ebooks.opds/)。只接受符合安全策略的 EPUB 资源 |
| `arxiv` · arXiv | 报刊文章 · 单一来源 | 真实学术文章标题、作者和摘要搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [arXiv API](https://export.arxiv.org/api/query)。遵循上游查询和速率限制 |
| `crossref` · Crossref | 报刊文章 · 单一来源 | 真实 DOI、期刊和文章元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [Crossref Works API](https://api.crossref.org/works)。完整正文不由本来源提供 |
| `openalex` · OpenAlex | 报刊文章 · 单一来源 | 真实学术作品、期刊和作者元数据搜索 | 无内置播放或下载候选 | 默认停用；无需账号或凭据 | [OpenAlex Works API](https://api.openalex.org/works)。公共 API 可能限流 |
| `cms10` · 苹果 CMS V10 | 影视 · 聚合来源 | 对已配置 CMS 目录执行真实影视搜索和元数据投影 | 返回受控在线播放候选和允许的下载候选 | 出厂预填并启用第一个受控预设；端点可在设置页切换，不需要账号才能使用预设 | 用户配置端点必须通过协议、Host、重定向和 SSRF 校验；上游站点内容和可用性不由 Haven 保证 |
| `m3u` · M3U 播放列表 | 影视 · 聚合来源 | 对用户配置的播放列表按频道名称真实搜索 | 返回受控频道播放候选 | 默认停用；用户必须配置端点；本版本不自动收集账号或凭据 | M3U 地址由用户明确提供，并受出站安全策略、格式和重定向限制 |

## 隐私与请求边界

内置 metadata Provider 使用源码中固定的 HTTPS API。用户配置的 CMS10、OPDS 和 M3U 端点只在用户明确登记后使用；应用会重新执行协议、Host、DNS、重定向和响应大小校验。来源搜索结果只通过 Typed HavenClient 进入 UI，前端不获得任意网络代理能力。

来源返回的海报不会绕过 Artwork Cache。缓存失败、缺图或来源暂时不可用时，栖阅显示本地默认封面或已有的 stale 缓存；不会把远端 URL 当作前端图片直链。
