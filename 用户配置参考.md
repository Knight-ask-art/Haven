# 用户配置参考

> 本文件与仓库 `SOURCES.md`（内置来源能力说明）配合阅读：`SOURCES.md` 说明每个来源"能做什么"，本文件说明"需要用户手动填什么"。
> 这份清单仅供手动配置参考，不会被程序读取，也不会改变任何来源的默认状态。使用前请自行确认来源服务的可用性、内容授权情况以及所在地适用的法律和服务条款。

## 通用规则：哪些来源需要手动配置

- **必须手动填写端点**：`cms10`、`m3u`。首次安装时默认停用，且不预填任何端点；请在栖阅"设置 → 来源"填写端点并保存，再启用该来源。
- **可选自定义**：OPDS 书库。`opds_gutenberg` 自带出厂预设（见下），用户还可自行添加最多 20 个自定义 OPDS 来源（`custom_*`）。
- **无需配置**：其余内置来源（TVMaze、Bangumi、AniList、MangaDex、iTunes、Gutenberg 搜索、Internet Archive 搜索、arXiv、Europe PMC、中文维基文库、Crossref、OpenAlex）。地址固定在程序中，无需账号或凭据，只需在设置页按需启用/停用。首次安装默认启用的有 `mangadex`、`arxiv`、`europepmc`、`wikisource`（及出厂预设的 `opds_gutenberg`），其余默认停用。

## 通用端点安全策略（用户端点不满足会直接被拒绝）

- 仅接受 `http://` / `https://`；禁止 userinfo（`user:pass@host`）、fragment（`#...`）。
- 主机必须是公网域名或公网 IP；回环（`127.0.0.1`/`localhost`）、私网（`10/8`、`172.16/12`、`192.168/16`）、链路本地、云元数据地址一律拒绝。
  - 注意：**部署在局域网的自托管服务（如 `http://192.168.1.10:8080/...` 的 Komga/Kavita/Calibre）会被拒绝**，请使用公网可达地址。
- 端口仅允许 `80` / `443` / `8080` / `8443`；不接受单标签主机名（如 `http://nas/...`）。

## CMS10（苹果 CMS V10，聚合影视）

CMS10 在首次安装时默认停用，也不会预填任何端点。需要使用 CMS10 时，请在栖阅的"设置"页面手动填写端点并保存，然后再启用该来源。

端点是 CMS V10 采集接口的基地址（形如 `.../api.php/provide/vod`），程序会自动拼接 `?ac=videolist&wd=关键词`（搜索）与 `?ac=videolist&ids=...`（详情）参数，不要自己在后面加查询串。

下面是可供用户自行核对和配置的端点清单：

- 暴风资源：`https://bfzyapi.com/api.php/provide/vod`
- 无尽资源：`https://api.wujinapi.me/api.php/provide/vod`
- 索尼资源：`https://suonizy.net/api.php/provide/vod`
- 光速资源：`https://api.guangsuapi.com/api.php/provide/vod`

## M3U 播放列表（聚合影视）

配置项为一条 M3U 播放列表 URL（默认停用，需手动填写后启用）。要求：

- 播放列表正文为 UTF-8 编码，大小不超过 4 MiB；非 UTF-8 会直接报"编码无法读取"。
- 按 `#EXTINF` 解析：逗号后的文字为频道名（无名则取 `tvg-name="..."`，再无则记"未命名频道"），`group-title="..."` 为分组；其下第一个非 `#` 开头的行为流地址。
- 流地址同样受上面的端点安全策略约束，不合规的条目会被静默跳过（不影响其他条目）。
- 搜索按频道名包含匹配（大小写不敏感）。

公开列表参考（均为社区维护的公开索引，不托管视频；频道可用性、画质、地区与授权请自行核对，失效是常态）：

- iptv-org 全量：`https://iptv-org.github.io/iptv/index.m3u`（8000+ 频道，体积大，可能超过 4 MiB 上限，**不推荐直接使用**）。
- 按国家/语言/分类拆分见 [PLAYLISTS.md](https://github.com/iptv-org/iptv/blob/master/PLAYLISTS.md)，例如中国大陆频道：`https://iptv-org.github.io/iptv/countries/cn.m3u`。日常使用建议选拆分后的小列表。

## 自定义 OPDS（自建/第三方书库）

- 在设置页"添加自定义来源"：显示名（必填，不超过 100 字符）+ 端点 URL；默认停用；重复端点会被拒绝；最多 20 个。
- 需要账号的书库可在来源详情里配置凭据，凭据存入系统 keyring（`haven:opds:<sourceId>`），端点本身不出 IPC；删除凭据传空即可清除。
- 出厂预设 `opds_gutenberg`（古登堡计划）：`https://m.gutenberg.org/ebooks.opds/`，可在设置中覆盖；EPUB 在线正文受 32 MiB 读取上限约束。
- **仅当前 `opds_gutenberg` 支持"搜索后导入正文"**，其他自定义 OPDS 目前仅支持搜索（导入会明确提示尚未开放）。

公开 OPDS 目录参考（免登录 unless 注明；可用性请自行核对）：

- 古登堡计划：`https://m.gutenberg.org/ebooks.opds/`（即出厂预设，7 万+ 公版书）。
- Standard Ebooks：`https://standardebooks.org/feeds/opds`（排版精良的公版书；**完整目录需 Patrons Circle 登录**，配置凭据时用户名填邮箱）。
- Internet Archive：`https://bookserver.archive.org/catalog/`（量大，但偶发不稳定）。
- OAPEN（开放学术书）：`https://library.oapen.org/opds`。
- ManyBooks：`https://manybooks.net/opds/index.php`（5 万+ 免费书）。
- unglue.it：`https://unglue.it/api/opds/`（CC 免费书）。

明确不推荐：Feedbooks（2024 年已关闭）；Open Library OPDS（当前网络下经常 TLS 失败，程序已不再预设）。

## 无需配置的内置来源一览

| 来源 | 类型 | 能力 | 上游（固定） |
| --- | --- | --- | --- |
| TVMaze | 影视 | 搜索 | `https://api.tvmaze.com/search/shows` |
| Bangumi | 影视、漫画 | 搜索 | `https://api.bgm.tv/v0/search/subjects` |
| AniList | 影视、漫画 | 搜索 | `https://graphql.anilist.co` |
| MangaDex | 漫画 | 搜索 · 在线章节 · 下载 CBZ | `https://api.mangadex.org`（图片仅 `*.mangadex.network`） |
| iTunes Search | 影视 | 搜索 | `https://itunes.apple.com/search` |
| Project Gutenberg | 图书 | 搜索 | `https://www.gutenberg.org/ebooks/search.opds` |
| Internet Archive | 图书 | 搜索 | `https://archive.org/advancedsearch.php` |
| arXiv | 报刊文章 | 搜索 · 在线 PDF · 下载 PDF | `https://export.arxiv.org/api/query` |
| Europe PMC | 报刊文章 | 搜索 · 在线安全正文 · 下载 HTML | `https://www.ebi.ac.uk/europepmc/webservices/rest/search`（仅开放获取全文） |
| 中文维基文库 | 报刊文章 | 搜索 · 在线安全正文 · 下载 HTML | `https://zh.wikisource.org/w/api.php` |
| Crossref | 报刊文章 | 搜索（元数据，不含全文） | `https://api.crossref.org/works` |
| OpenAlex | 报刊文章 | 搜索（元数据） | `https://api.openalex.org/works` |

上游可能限流、变更协议或暂时不可用；Haven 不承诺第三方 SLA。海报缺失时由本地默认封面处理，不影响搜索结果。
