# 桌面运行时与安装包优化设计

本文是 DeepSeek Harness Desktop 的运行时交付、升级回滚和打包性能契约。实现基线固定为
`@deepseek-ai/dsh@0.1.2-alpha.2`（npm integrity 见 `runtime.lock.json`）、Node.js `22.22.3`、
`dshmarket@1.38.1`、`pnpm@10.34.5` 和 Windows x64。上游概念说明固定参考 rc.8 发布 commit
[`15148dbd` 的 DSH 官方基础文档](https://github.com/deepseek-ai/deepseek-harness/blob/15148dbd9a1d1f1ef1a26e5749b32af0cd663935/docs/user/develop/basic/index.md)，
行为判定以锁定 npm 包的实际入口、配置和 loader smoke 为准，不跟随主分支文档漂移。

macOS 尚未进入本实现基线；平台适配完成后的单架构打包、包体预算、签名公证与发布验收见
[macOS 单架构打包与发布门禁](macos-packaging-and-release.md)。

## 当前状态

payload 链路已经实现，并已由 preview.8、preview.9 连续完成两轮公开门禁。preview.10 已将
`npm run build` 切换到 `build:payload`，并通过切换后的完整门禁；还需一个稳定 preview，才能删除
legacy 暂存路径。

| 能力 | 当前状态 | 发布约束 |
|---|---|---|
| schema 2 runtime/plugin lock | 已实现 | 固定单一 pnpm 10，插件必须声明 `delivery` |
| Host/插件裁剪与 debug 分离 | 已实现 | PDB/source map 不进入默认安装器 |
| 三个确定性 ZIP 与 manifest | 已实现 | 每次缓存复用前重新校验 |
| candidate/active/previous 状态机 | 已实现 | candidate 通过真实 readiness 后才能晋升 |
| legacy 与 payload 双构建 | 已实现 | 默认为 payload，legacy 继续用于回归 |
| clean payload 安装器 smoke | 已实现并纳入门禁 | 使用隔离 runtime 根，不触碰真实 profile |
| legacy/payload 连续升级矩阵 | preview.8 已通过 4/4，preview.9、preview.10 已通过 6/6 | 再经过一个稳定 preview 才删除 legacy |
| 20 对交替启动 P95 | preview.8、preview.9、preview.10 均已通过 | 不得以单独 payload 样本替代相对 P95 |
| `pnpm@10.34.5` 依赖审计 | 已通过 | 三组 audit 必须持续为 0 total/high/critical |

## 目标与基线

2026-08-17 的 legacy 正式暂存资源包含 47,418 个文件、584,489,650 字节（557.4 MiB）：

| 资源 | 文件数 | 字节数 |
|---|---:|---:|
| Host | 35,956 | 344,045,447 |
| 内置插件 | 11,459 | 153,325,311 |
| Node.js | 3 | 87,118,892 |

payload 发布门禁如下：

- 默认安装包不超过 100 MiB，active runtime 不超过 300 MiB；
- NSIS 安装资源固定为 manifest 与三个 ZIP，共 4 个文件，相对基线减少至少 90%；
- warm cache 重复打包不超过 10 分钟，冷暂存与打包不超过 20 分钟；
- 启动 P95 劣化不得超过 5% 或 100 ms，两者取较大值；
- 默认安装器不包含 PDB、source map、声明文件、测试或示例源码；
- 功能、回滚、体积或速度任一门禁失败时继续发布 legacy，不放宽阈值。

preview.8 最终 payload 为 12,897 个展开文件、239,303,045 字节（228.22 MiB），三个 ZIP 共
84,702,330 字节（80.78 MiB），NSIS 安装器 97,299,624 字节（92.79 MiB），SHA-256 为
`c16498e160cc94b73082edf249353d54e2b6a3129920a2587963815f7036ba5e`。两次强制完整构建分别为
718,874 ms 与 695,754 ms，warm cache 完整构建为 225,946 ms；四个 payload 资源的 SHA-256 逐项一致，
payload digest 为 `ea75cc9ff05bb557e5b53360dad42ac5c60dc50bba29d6f63b1fc54e3a4aa08b`。

在提交 `f76a7d7e81dcfa6f3f40e4ec20a630323f03a6ba` 上使用相同 exe 和四个 payload 资源执行 NSIS
同输入 A/B：`compression: "none"` 为 97,299,624 字节，solid LZMA 为 82,941,995 字节，减少
14,357,629 字节（13.693 MiB）。NSIS bundling 从 5,551 ms 增至 77,803 ms，仍远低于 warm cache
10 分钟预算。该收益超过 10 MiB 采用阈值，因此 preview.9 改用 solid LZMA；最终 LZMA 安装器仍必须完成
preview.9 全部门禁，不能复用 preview.8 的 `none` 安装器报告。

preview.9 最终 solid LZMA 安装器为 82,934,193 字节（79.09 MiB），SHA-256 为
`12de9ab77e7af167534356cfa93e0aa887a363f0a4857505be40e422f83379c4`。payload 仍为 12,897 个展开文件、
239,303,045 字节，三个 ZIP 共 84,702,328 字节；两次强制完整构建分别为 714,876 ms 与 729,395 ms，
四个资源文件逐字节一致，payload digest 为
`3502e8685c168414beb1da3855205f5d38dd234e4147563f361af0c635047848`。升级矩阵 6/6 通过；20 对安装版
warm 启动中，legacy P50/P95 为 4,318/4,999 ms，payload 为 4,056/4,601 ms，门限为 5,249 ms；
各 3 次 cold 为 legacy 16,509/16,000/16,130 ms、payload 5,306/5,207/4,647 ms。

preview.10 将默认构建切换为 payload。Skin Center 画廊的 26 张预览在 staging 副本内确定性缩放为
最大 480px 的调色板 PNG，从 48,108,062 字节降至 901,836 字节；主题运行时背景原图不变。最终 payload
为 12,972 个展开文件、254,073,088 字节，三个 ZIP 共 93,546,324 字节，安装器为 91,814,862 字节
（87.56 MiB），SHA-256 为 `ee7f9a4613a920a0b76eec5af696a2e0aebee21d856e188da87bcfb734fd52df`。
两次强制构建的四个资源逐字节一致，payload digest 为
`fbdb8a56786a1fe331ba2cf400021be33ce9b74b1888404db0d8889e848c7f59`。6/6 升级矩阵通过；20 对 warm
启动中，legacy P50/P95 为 7,662/11,628 ms，payload 为 7,921/9,707 ms，门限为 12,210 ms；各 3 次
cold 为 legacy 43,969/25,106/42,109 ms、payload 8,833/11,089/12,494 ms。

preview.13 的 Sidebar 0.15.0 已把 Mermaid 完整内联到浏览器懒加载 chunk。payload staging 根据 npm
lockfile 的真实依赖图裁剪只由该已内联依赖可达的 112 个 package，并保留仍被其他插件引用的共享节点。
裁剪后 payload 为 11,329 个展开文件、242,193,184 字节，三个 ZIP 共 93,159,595 字节（88.84 MiB），
继续满足 90 MiB 压缩预算，不需要放宽安装器 100 MiB 的发布门限。正式安装器大小和两次可复现构建结果
仍以绑定 release commit 的门禁报告为准。

preview.8 最终同机安装版 20 对 warm 启动中，legacy P50/P95 为 4,902/5,533 ms，payload 为
4,973/5,547 ms，门限为 5,810 ms；各 3 次 cold 为 legacy 17,278/16,902/17,827 ms、payload
6,002/5,377/5,416 ms。公开 preview.7 仅是 SHA-256 为
`e331e628b07bf574e823610324130c258d77ed1e57113b59426feed1a3a9d3d9` 的 legacy 基线，不能计作 payload 灰度。

## 构建架构

### 锁文件

`runtime.lock.json` schema 2 只允许 Node `22.22.3`、DSH `0.1.2-alpha.2`、Market `1.38.1`
和 pnpm `10.34.5`。构建不得探测或调用全局 pnpm，也不得下载另一个 major。

原 `pnpm@10.33.2` pin 因 high advisories 被废止。批准的 `10.34.5` 使用 registry integrity
`sha512-pO4F8vc2WCVb1qiYWcBlpFwopX2u+uLIk6Fo7itzFow3uR6D5X6mdlStA/AwMXRkMOi84442LgQmBfuKvIAZLg==`；
主项目、runtime-host 和 plugin-runtime 三组 audit 当前均为 0 漏洞。每个 preview 仍必须重新执行 audit、
历史 profile fixture、Market add/update/remove 和 payload 可复现性门禁，任何 residual advisory 都阻断发布。

`plugins.lock.json` schema 2 的每个插件必须声明：

- `serverEntries`：DSH/Cordis 服务端入口；
- `clientEntries`：已编译客户端入口；
- `assets`：patch、Skill、schema、图片等运行时资产；
- `runtimeExternals`：Node 动态解析的 JavaScript 依赖；
- `nativeExternals`：Windows x64 native addon；
- `licenseFiles`：许可证或可追溯的包元数据。

`verify-plugins.ps1` 同时验证 `requiredFiles` 与 delivery 契约。缺少入口、资产、native 文件或许可证时
构建失败，不能在运行时回退下载。

### Host 与插件闭包

`stage-runtime.ps1` 和 `stage-plugins.ps1` 先在 `.runtime-cache` 的受控目录执行锁定安装，再向
`src-tauri/resources` 暂存；不会直接在 Tauri resources 内执行 `npm ci`。Host 入口经固定版本
esbuild 处理后仍位于：

```text
host/node_modules/@deepseek-ai/dsh/lib/bin.js
```

该路径保持 `import.meta.url`、DSH install anchor 和动态包名解析语义。Node 内置模块、Cordis 动态配置、
用户插件、native addon 与运行时资产保持 external；esbuild metafile、delivery allowlist 和真实 loader
smoke 共同约束闭包。

裁剪规则只作用于已经证明不参与 Windows x64 运行的内容：PDB、map、类型、测试、示例、多平台 native
文件和已内联客户端依赖。目录名 `doc/docs` 不作为删除依据，因为 npm 包可能把运行时代码放在同名目录。
`node-pty` 与 `sharp` 在各自真实 Node 解析根下保留所需副本，不依赖 `NODE_PATH` 或偶然搜索顺序共享。

PDB 和 source map 收集到独立的 `.deploy-artifacts/runtime-debug-symbols/*.zip`。默认安装器只包含运行时
许可证和 NOTICE；debug artifact 不参与 payload digest。

### 确定性 payload

Tauri payload resources 固定为：

```text
payload-manifest.json
node-runtime.zip
host-runtime.zip
builtin-plugins.zip
```

Node ZIP 只含 `node.exe` 与官方许可证，不原样交付官方归档的其他文件。三个 ZIP 按规范化相对路径排序、
固定时间戳并使用 Deflate level 6。Rust `payload` 模块与 `payload-tool` 共享 ZIP、manifest、摘要和路径校验
实现，避免构建期与运行期出现两套规则。

manifest 固定记录 `schemaVersion`、`runtimeAbi`、`desktopVersion`、`payloadDigest`、Node/pnpm 版本、三个
入口，以及每个 ZIP 的 SHA-256、压缩大小、展开大小和文件数。`payloadDigest` 按固定顺序对 schema、ABI
和三个 ZIP SHA-256 计算。相同输入连续构建时，三个 ZIP 与 manifest 必须逐字节一致。

缓存键覆盖两套运行时 lockfile、主 package lock、Cargo lock、构建脚本、payload 实现、目标平台以及
Node/npm/esbuild/Rust 工具版本。命中缓存后必须先执行完整 `verify`，校验失败即放弃缓存。

## pnpm 10 profile 兼容事务

Runtime Services 是桌面与 Market 的包管理边界。每个用户发起的 add/update/remove 操作遵循：

1. 首次操作前按原始字节快照 `package.json`、`pnpm-lock.yaml`、`pnpm-workspace.yaml`、profile 与全局
   Cordis patch；bundle 状态包含在 profile manifest 快照内。
2. 只使用内置 pnpm `10.34.5` 执行原操作，并关闭项目 `packageManager` 自动切换或下载其他版本。
3. 仅当 pnpm 明确报告 modules 或 hoist major 不兼容时，恢复控制文件并将旧 `node_modules` 在同卷
   原子改名为备份。
4. 执行一次 `install --no-frozen-lockfile`，不使用 `--force`，成功后只重试原操作一次。
5. 重建或重试失败时恢复全部控制文件与旧依赖树；不循环、不切换 major、不访问全局 pnpm。

普通网络、解析、脚本或权限错误不会触发重建。事务保证只覆盖 profile 控制文件和依赖树；第三方安装
脚本在 profile 外产生的副作用无法回滚，失败日志必须明确这个边界。

## Provision 与启动状态机

运行时根固定为 `%LOCALAPPDATA%\dsh-desktop\runtime`，状态只存于一个原子
`runtime-state.json`：

```json
{
  "schemaVersion": 1,
  "active": null,
  "previous": null,
  "candidate": { "payloadDigest": "...", "runtimeAbi": 1, "desktopVersion": "..." }
}
```

`dsh-desktop.exe --provision-runtime` 在 Tauri 与 single-instance 初始化前执行，只完成校验、展开和 candidate
登记。正式构建不接受任意 runtime 根覆盖；`--provision-test-mode` 与路径覆盖只供安装器隔离 smoke。

Provision 使用 `runtime/.provision.lock` 跨进程串行化，展开到 `<digest>.staging.<pid>`，完整验证后同卷
rename 为 `<digest>`。Windows ZIP 校验拒绝：

- 绝对路径、父级路径、UNC/device path、ADS；
- 保留设备名、尾随点或空格；
- 大小写冲突、重复目标；
- symlink/reparse point、加密条目；
- 超过 manifest 与全局上限的文件数或展开大小。

应用启动时优先准备 candidate 的不可变插件 junction，启动真实 Host 并等待 core/plugins readiness。成功后
先原子晋升 active，再提交 profile 插件事务；状态或提交失败则回滚链接和状态。candidate 失败会被拒绝，
应用继续使用旧 active。晋升后 `previous` 保留上一代，垃圾清理只删除未被 active、previous、candidate
引用的 runtime。

内置插件直接从不可变 runtime 的 `plugins/node_modules` 建立 profile junction，不再复制到第二个 managed
store。用户通过 Market 安装的插件仍位于用户 profile，桌面不改变其 schema 和安装语义。

runtime ABI 初始为 1。每个桌面版本必须兼容 active 和 previous 两代 ABI；升级 ABI 时至少发布一个同时
支持旧、新 ABI 的过渡版本。

## NSIS 与卸载

`tauri.payload.conf.json` 只打入四个 payload 文件，并使用 `compression: "lzma"` 生成 solid LZMA
安装器。该选择来自同输入 A/B 的 13.693 MiB 实测收益；三个内部 ZIP 的格式和 payload digest 不变。
升级的 PREINSTALL 先调用旧 exe 的 `--quit-existing` 并等待最多 10 秒，失败或超时会在覆盖文件前
终止。POSTINSTALL 调用 provision：clean install 失败则安装失败；upgrade 失败保留旧 active，新 exe
依靠 ABI 兼容继续运行。

卸载始终删除桌面托管 runtime。只有用户勾选“删除应用数据”时才删除日志等其余 LocalAppData；任何情况
都不删除 `~/.dsh`。安装器 smoke 的 runtime 根必须位于系统临时目录的固定前缀下，并在卸载前规范化校验，
防止测试污染真实安装状态或把任意路径交给递归删除。

## 命令与产物

```powershell
# 默认发布路径，验收完成前保持 legacy
npm run build
npm run build:legacy

# payload 构建与独立验证
npm run package:payload
npm run verify:payload
npm run build:payload

# 当前 preview 的完整发布门禁
npm run release:gate -- -LegacyInstaller '<preview.7>' -PayloadInstaller '<current payload>'
```

该命令必须在专用、可丢弃的 Windows 用户中运行。Tauri NSIS 的 HKCU 产品键和 Shell 快捷方式名不受
`/D=` 或 `LOCALAPPDATA` 重定向影响；门禁会拒绝任何既有进程、产品键、自动启动项或快捷方式，并在
测试结束时只清理由系统临时安装根拥有的 Shell 状态。

`stage-payload.ps1` 使用按 cache key 的跨进程独占文件锁发布缓存，异常退出由系统释放句柄，避免并发构建
互删或同时 rename 同一目录。缓存元数据记录缓存输入、复制、裁剪、bundle、loader smoke、ZIP/manifest 耗时及
输入输出文件数和字节数；`build-payload.ps1` 另行生成 `.release-work/<version>/reports/payload-build-report.json`，记录
完整的 Tauri/NSIS 分阶段耗时与安装器摘要。默认发布产物不包含 `payload-tool.exe`；该工具只作为 Cargo
example 在构建期运行。

`write-release-artifacts.ps1` 先在 `.deploy-artifacts/<version>.staging.<pid>` 汇总安装器、SHA-256、manifest、
构建/审计/兼容/可复现性/启动/升级报告、许可证和 debug symbols，交叉校验版本与摘要后再同卷 rename 为
`.deploy-artifacts/<version>`。它不会清空共享的 debug symbols 或复用旧 preview summary。

## 测试与发布门禁

实现级门禁包括：

- runtime-services fixture：明确错误识别、字节快照、单次重建、重试失败恢复、PATH/Git 环境隔离；
- payload 单测：摘要、截断 ZIP、ZIP bomb、路径穿越、ADS、大小写冲突、并发 provision、中断恢复、
  state 晋升/回滚和垃圾清理；
- 真实 loader/Host：DSH Web、插件锁中的 7 个内置 bundle、PTY、sharp、GenUI、Hindsight、Skills/MCP、主题；
- 启动 fast path：Windows junction 使用 Win32 路径语义比较，健康 active 不重建链接；插件准备与 WebView2 初始化并行；
- 安装器：clean install、candidate 晋升、single-instance、关闭到托盘、进程树退出、卸载 runtime 且保留
  profile；
- 构建：lint、Rust/Node 测试、80% 核心覆盖率、三组 npm audit、runtime/plugin/payload verify、
  `git diff --check`。

灰度顺序固定为：preview.8 是第一轮公开 payload，preview.9 重复全部门禁并增加 preview.8 到 preview.9
的停止/运行中升级；两轮默认 build 均保持 legacy。preview.10 已在两轮通过后切换默认 payload，同时保留
`build:legacy`，并完成切换后的 6/6 升级矩阵与启动门禁。还需一个稳定 preview 才删除 legacy 暂存路径。
矩阵必须覆盖 candidate 启动失败、损坏资源与回滚，并在相同机器和防病毒状态下记录交替 20 对 warm 与
各 3 次 cold 启动。未完成项会阻止发布和后续 legacy 删除。

## 不变量与非目标

- 不修改 DSH Web、Agent、用户 profile schema、业务配置或 Market 用户插件安装语义；
- 不把用户插件编入安装器，不增加运行时静默下载，不依赖全局 pnpm；
- 不通过删除许可证、动态资产或 native 运行文件达成体积目标；
- ZIP SHA-256 提供内容完整性，不替代 Authenticode、插件签名或进程沙箱；
- profile 回滚是字节级保证，profile 外第三方脚本副作用不在事务范围内。
