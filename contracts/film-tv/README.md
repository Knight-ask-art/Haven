# 影视四层证据合同

本目录是影视优化分支的可执行验收合同。机器可读的唯一矩阵是
[`acceptance-matrix.json`](acceptance-matrix.json)；本说明只解释如何使用它。

## 结论规则

影视能力的状态不是“代码存在”或“构建成功”。某项能力只有在同一个候选版本上同时满足以下四层，才可以记录为 `pass`：

1. **local**：开发机上的类型、契约、协议、状态机和安全边界检查。
2. **ci**：干净 CI runner 上真实执行的影视专项检查，并有独立步骤和运行摘要。
3. **runtime**：桌面应用使用固定版本的可控测试源，验证真实调用链、失败路径和恢复行为。
4. **release**：候选包、候选配置、数据兼容、回滚材料和是否启用分别核对。

构建成功只产生“制品构建证据”，部署成功只产生“部署动作证据”。两者不能填充 runtime，也不能填充 release 的功能验收项。矩阵中每项能力当前都明确保持 `not-accepted`，直到后续产生四层新鲜记录。

## 可执行入口

从仓库根目录运行：

```text
python tools/film-tv/evidence-check.py --layer contract
python tools/film-tv/evidence-check.py --layer local --output 测试/film-tv/evidence/local.json
```

`--layer contract` 只检查矩阵自身是否完整、引用是否存在以及是否错误地把构建/部署列为功能证明。

`--layer local` 会执行矩阵中标记为本地检查的 fixture、前端、Rust/Tauri 测试，输出只含命令 ID、退出码、耗时、提交 SHA、工作树指纹和输出哈希，不保存命令原文输出。工作树指纹包含 HEAD 到当前工作树的差异以及非忽略的新文件，因此脏工作树上的本地记录不会错误地退化为只有 HEAD SHA 的旧证据。失败时命令的最后几行只以脱敏形式打印到终端。`FTV-LCL-FIXTURE-SOURCE-001` 只证明固定 fixture 自身可启动并返回预设输入，不能证明桌面应用已经消费它。`测试/` 已被仓库忽略，不能把其中的记录当作公开源代码或发布材料。

CI 使用同一个执行器生成 `ci` 记录，但生成器还要求真实 GitHub Actions 上下文
（`GITHUB_ACTIONS=true`）、有效 run ID、只读 `GITHUB_TOKEN` 和 `GITHUB_REPOSITORY`，并
通过 GitHub Actions API 核对 run、仓库和当前 commit。这样开发机手动注入数字
`GITHUB_RUN_ID` 或伪造 `GITHUB_API_URL` 不能冒充 CI 证据；最终仍需在 GitHub 上读取
run、job 和 artifact，确认它确实对应目标 commit。token 只存在于 CI 进程环境，不写入记录。

运行时和发布不由构建脚本自动“猜测通过”。验收执行人先根据矩阵逐项操作，再用以下命令校验脱敏记录：

```text
python tools/film-tv/evidence-check.py --validate-record 测试/film-tv/evidence/runtime.json --layer runtime
python tools/film-tv/evidence-check.py --validate-record 测试/film-tv/evidence/release.json --layer release
```

运行时记录必须引用测试源版本、应用构建身份、候选对象 SHA-256、环境和每个场景的观察结果；不得写入真实 URL、Cookie、Authorization、Token、签名 URL、密码、绝对路径或原始日志。每条记录还必须带有 `recordSha256`，它是去掉自身字段后对完整结构化记录计算的 SHA-256，防止记录内容在生成后被悄悄替换。发布记录的 `evidenceRefs.local/ci/runtime` 必须保存被引用记录的 `recordSha256`，而不是无来源的自由文本；发布记录还必须引用候选对象 SHA-256、候选制品 SHA-256、可用且已演练的回滚材料和 `enabled` 状态。`candidateSha256` 只标识被 runtime/release 实际验证的候选对象，不能用“部署成功”或流水线构建日志代替。

发布前还要执行一次 bundle 交叉校验。它会重新读取四条记录，核对记录摘要、共同 commit、local/CI 工作树指纹、能力、候选对象字节和候选制品字节；命令参数中的路径只用于读取，不会写入记录：

~~~text
python tools/film-tv/evidence-check.py --validate-bundle \
  --local-record 测试/film-tv/evidence/local.json \
  --ci-record 测试/film-tv/evidence/ci.json \
  --runtime-record 测试/film-tv/evidence/runtime.json \
  --release-record 测试/film-tv/evidence/release.json \
  --candidate-file <candidate-object> \
  --artifact-file <candidate-artifact>
~~~

单条 `--validate-record` 负责结构、场景集合和脱敏检查；只有 bundle 交叉校验成功，才证明 release 记录确实指向相同版本和实际字节。它仍不会把构建或部署动作变成功能验收。

## 记录状态

- `not-accepted`：尚未满足四层完成条件；这是当前所有影视能力的初始状态。
- `partial`：有部分证据，但至少一层或一个场景缺失。
- `pass`：矩阵要求的检查和场景全部通过，且没有以 build/deploy 代替功能证据。
- `fail`：已执行的必要检查或场景失败。
- `blocked`：有明确的外部条件阻塞，并记录阻塞原因；不能用“暂时没测”代替。

记录应使用不含秘密的摘要。原始日志、截图、运行环境导出和测试媒体只放在受控的本地验收目录或 CI artifact 中，发布仓库只保留合同和可审计的检查入口。

## 当前边界

矩阵故意覆盖以下九项影视能力：多源搜索、来源协议、作品身份、播放与 HLS、代理安全、字幕、播放进度、本地缓存、离线下载。共享网络策略也有独立的 `FTV-LCL-COMMON-NETWORK-001` local/ci 检查，防止来源端点、媒体流和受控代理的 URL/主机/端口规则再次漂移。现有基础测试可以作为 local/ci 的底座，但不能自动证明字幕、DNS/重定向安全、远程 HLS 下载、桌面重启恢复等运行时场景已经完成。

后续实现每个能力时，应先补齐对应的协议/领域合同和矩阵检查，再使用 `python tools/film-tv/fixture_server.py --serve`（默认命令直接启动服务）建立可控 fixture source，最后取得 candidate package 的 runtime/release 记录。需要消费 loopback fixture 的候选包必须显式编译 `src-tauri` 的 `film-tv-fixture` feature；该 feature 不属于默认或发布构建。不要把 MoonTV 或 MoonTVPlus 的真实服务、第三方站点或用户数据作为验收依赖。
