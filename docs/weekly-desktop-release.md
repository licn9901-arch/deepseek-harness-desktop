# DSH 桌面版每周版本同步

本文规定 DeepSeek Harness Desktop 的每周发版节奏，以及 DSH 内核、DSH Market、内置插件和托管
Skill 的版本同步方式。本文是版本选型与发布记录的事实来源；具体构建、测试和发布操作仍以
[预览版发布检查表](release-checklist.md)和[测试说明](testing.md)为准。

## 发布目标

- 原则上每周产出一个 Windows x64 桌面版本；存在阻断问题时允许延期，不以跳过门禁换取周更。
- 每周同时核对 DSH 内核、DSH Market、全部内置插件和托管 Skill 的上游版本。
- “线上最新版本”只表示上游已经发布，不代表可以直接进入桌面版；最终以“本周目标版本”为准。
- 所有进入安装包的版本必须写入锁文件并通过完整性、兼容性、升级和安装器验证。
- Node.js 和 pnpm 不要求每周升级，但每周必须确认现有版本仍满足 DSH 与 Market 的运行范围；发生变化时在当周记录中说明。

## 每周节奏

1. **盘点**：读取 `package.json`、`runtime.lock.json` 和 `plugins.lock.json`，记录当前已发布桌面版及其内置版本。
2. **查新**：核对 DSH GitHub Releases、npm Registry，以及使用 GitHub tarball 交付的插件 Release 或 tag。
3. **选版**：评估 release notes、依赖范围、数据兼容性、权限变化和已知问题，确定本周目标；不采用的最新版本必须说明原因。
4. **锁定**：更新桌面版本、运行时锁、插件锁、包锁、第三方许可证和用户可见版本说明；移除组件时同步清理 bundle、资产、测试和文档引用。
5. **验证**：完成聚焦兼容验证后执行正式发布门禁，所有报告绑定同一个 release commit。
6. **发布**：生成版本化产物、SHA-256 和发布说明，复核后发布 prerelease，并在本文追加最终结果。

建议每周只维护一行发布记录。候选未通过门禁时保留“阻断”状态，不得提前写成已发布。

## 版本来源与选版规则

| 组件类型 | 当前版本来源 | 线上最新来源 | 选版规则 |
|---|---|---|---|
| 桌面版 | `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` | 本仓库 GitHub Releases | 沿用现有 preview 序列，三个版本声明必须一致 |
| DSH 内核 | `runtime.lock.json` | [DeepSeek Harness Releases](https://github.com/deepseek-ai/deepseek-harness/releases) 与 npm dist-tag | 预览版可采用上游 prerelease，但必须记录 GitHub Release 与 npm dist-tag 的差异 |
| DSH Market | `runtime.lock.json` | [dshmarket npm](https://www.npmjs.com/package/dshmarket) | 采用通过 DSH、私有 pnpm 和用户插件事务验证的版本 |
| npm 插件 | `plugins.lock.json` | 对应 npm 包的 `latest` dist-tag | 校验 peer dependency、delivery、权限和运行时依赖后再升级 |
| GitHub 插件 | `plugins.lock.json` 中的 tag、commit 和归档 SHA-256 | 对应仓库的最新 Release 或稳定 tag | 必须同时锁定 tag、解析后的 commit 和归档 SHA-256 |
| 托管 Skill | `plugins.lock.json` 的 `skills` | 所属插件的锁定归档 | Skill 版本跟随来源插件，并重新计算内容 SHA-256 |

查新时间、查询结果和最终选择必须一起保留。上游在候选冻结后发布的新版本顺延到下一周，不在正式门禁中途换版。

## 2026-W35 版本计划

盘点日期：2026-08-31。当前已发布基线为 `v0.1.0-preview.13`，本周本地候选版本为
`v0.1.0-preview.14`。本周重新盘点 DSH、Market、全部内置插件及托管 Skill；不创建 Release、不推送
tag、不上传资产，因此候选不能表述为已发布版本。

| 组件 | 当前版本 | 线上最新 | 本周目标 | 处理 |
|---|---:|---:|---:|---|
| DSH Desktop | `0.1.0-preview.13` | 不适用 | `0.1.0-preview.14` | 更新 package、Cargo、Tauri 配置、桌面托管组件及其包锁的版本声明 |
| [`@deepseek-ai/dsh`](https://www.npmjs.com/package/@deepseek-ai/dsh) | `0.1.1-rc.2` | `0.1.1-rc.2` | `0.1.1-rc.2` | npm `latest` 与 `next` 均未变化；GitHub alpha 尚未作为 npm 稳定候选交付，不纳入桌面包 |
| [`dshmarket`](https://www.npmjs.com/package/dshmarket) | `1.17.1` | `1.38.1` | `1.38.1` | 更新运行时锁和包锁，重新验证私有 pnpm 与 profile 事务 |
| [`@changfenhuang/dsh-genui`](https://github.com/omdsh-dev/dsh-genui/releases/tag/v0.9.6) | `@omdsh-dev/dsh-genui` `0.8.6` | `0.9.6` | `0.9.6` | 上游更换包名；迁移桌面托管的旧依赖，保留用户自装旧包，交付 ECharts 资产与更新后的 Skill |
| [`dsh-better-sidebar`](https://www.npmjs.com/package/dsh-better-sidebar) | `0.15.0` | `0.17.1` | `0.17.1` | 更新客户端 Mermaid 动态入口的 delivery 清单 |
| [`@linxin666/dsh-client-ui-skin-center`](https://www.npmjs.com/package/@linxin666/dsh-client-ui-skin-center) | `0.2.7` | `0.3.10` | `0.3.10` | 验证 Skin 格式 v2 迁移、切换和本机媒体背景 |
| [`@vectorize-io/hindsight-coding-agents`](https://www.npmjs.com/package/@vectorize-io/hindsight-coding-agents) | `0.4.1` | `0.4.3` | `0.4.3` | 保持项目显式 opt-in，验证 Windows 子进程行为 |
| [`@cubee-slide/skills-mcp-manager`](https://www.npmjs.com/package/@cubee-slide/skills-mcp-manager) | `0.2.4` | `0.2.4` | `0.2.4` | 上游无新 npm 版本，保持锁定输入 |

### 本次结果

- [x] 版本声明统一更新为 `0.1.0-preview.14`。
- [x] 更新 Market、全部有新版的内置插件和 GenUI Skill；DSH 与 Skills/MCP Manager 经盘点后保持当前最新可用版本。
- [x] 为 GenUI 包名迁移增加桌面托管升级保护：仅替换仍由桌面托管的旧包，不重启已禁用 bundle，也不修改用户自装旧包。
- [ ] 重新生成并校验本地 Windows x64 NSIS 安装包。
- [ ] 发布门禁、GitHub Release、tag 与资产上传：本次明确不执行。

此前仅更新版本号生成的本地 `preview.14` 安装器不包含本周组件升级，已作废；本节将在重新打包后记录新的
构建提交、安装器 SHA-256 与 payload digest。新安装器同样只保留在本地，不发布。

## 2026-W34 版本计划

盘点日期：2026-08-22。当前已发布基线为 `v0.1.0-preview.12`，本周计划版本为
`v0.1.0-preview.13`。候选在 DSH `0.1.1-rc.2` 发布后重新冻结；下表中的“线上最新”是本次冻结时结果，
“本周目标”仍需通过锁定、构建和门禁后才成为发布事实。

### 桌面与运行时

| 组件 | 当前版本 | 线上最新 | 本周目标 | 处理 |
|---|---:|---:|---:|---|
| DSH Desktop | `0.1.0-preview.12` | 不适用 | `0.1.0-preview.13` | 生成新的每周预览版 |
| [`@deepseek-ai/dsh`](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.1-rc.2) | `0.1.0-rc.8` | `0.1.1-rc.2` | `0.1.1-rc.2` | 升级并验证原生视觉、数据与插件兼容性 |
| [`dshmarket`](https://www.npmjs.com/package/dshmarket) | `1.17.1` | `1.17.1` | `1.17.1` | 保持版本，重新验证 add/update/remove 事务 |
| `@dsh-desktop/runtime-services` | `0.1.0-preview.12` | 不适用 | `0.1.0-preview.13` | 跟随桌面版升级 |
| `@dsh-desktop/settings` | `0.1.0-preview.12` | 不适用 | `0.1.0-preview.13` | 跟随桌面版升级 |

补充说明：重新冻结时 npm 的 `latest` 与 `next` dist-tag 均指向 DSH `0.1.1-rc.2`，GitHub tag 为
`dsh-v0.1.1-rc.2`。本版本仍是 upstream prerelease，发布说明不得将其表述为稳定版本。

### 内置插件与 Skill

| 组件 | 当前版本 | 线上最新 | 本周目标 | 处理 |
|---|---:|---:|---:|---|
| [`dsh-at-file`](https://github.com/omdsh-dev/dsh-at-file) | `0.6.0` | `0.6.3` | 移除 | DSH `rc.8` 已原生提供 `@` 文件引用，不再重复内置 |
| [`@omdsh-dev/dsh-genui`](https://github.com/omdsh-dev/dsh-genui/releases/tag/v0.8.6) | `0.8.6` | `0.8.6` | `0.8.6` | 保持插件及其托管 Skill 版本 |
| [`dsh-better-sidebar`](https://www.npmjs.com/package/dsh-better-sidebar) | `0.14.0` | `0.15.0` | `0.15.0` | 升级并验证侧边对话、上传、文件、Git 与 PTY |
| [`@linxin666/dsh-client-ui-skin-center`](https://www.npmjs.com/package/@linxin666/dsh-client-ui-skin-center) | `0.2.6` | `0.2.7` | `0.2.7` | 升级并验证皮肤切换与本机媒体背景 |
| [`@vectorize-io/hindsight-coding-agents`](https://www.npmjs.com/package/@vectorize-io/hindsight-coding-agents) | `0.4.1` | `0.4.1` | `0.4.1` | 保持版本并验证项目 opt-in 和服务连接 |
| [`@liustack/modlens`](https://www.npmjs.com/package/@liustack/modlens) | `3.16.7` | `3.22.1` | 移除 | DSH 已提供原生多模态模型，不再内置视觉回退插件 |
| [`@cubee-slide/skills-mcp-manager`](https://www.npmjs.com/package/@cubee-slide/skills-mcp-manager) | `0.2.4` | `0.2.4` | `0.2.4` | 保持不变，仍需执行 `rc.8` 兼容验证 |
| GenUI Skill | `0.8.6` | `0.8.6` | `0.8.6` | 保持版本并复核内容 SHA-256 |

移除 `dsh-at-file` 和 ModLens 后，桌面托管 bundle 从 9 个减少为 7 个，托管 Skill 仍为 1 个。升级只清理
仍由 `plugins-state.json` 和桌面 junction 共同证明属于桌面托管的依赖；用户自行安装或替换来源的同名插件
属于用户 profile，不得由桌面升级流程擅自卸载。

## 本周内核变更影响

DSH `v0.1.1-rc.1` 的[官方发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.1-rc.1)
新增 `DeepSeek-V4-Flash-Vision-Exp` 多模态视觉模型，`v0.1.1-rc.2` 的
[官方发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.1-rc.2)进一步增加 Files API
图片复用、自动缩放和格式转换。这一原生能力是本周移除 ModLens 的依据。验收至少覆盖：

- 模型列表可以选择 `DeepSeek-V4-Flash-Vision-Exp`，PNG、JPEG、超尺寸和需转换格式的图片均可处理。
- 重复引用同一图片时保留 Files API 复用证据，页面和安装目录没有 ModLens 残留或重复视觉入口。
- 纯文本模型不宣传具备视觉能力，也不再提供 ModLens 的外部视觉端点或本机 Agent CLI 回退。

`rc.8` 起原生 `@` 菜单支持引用文件和会话，因此仍需完成以下退役验证：

- 输入 `@` 后可以搜索并选择工作区文件，发送后的引用能被模型正确接收。
- 会话引用与文件引用可以共存，键盘选择、取消和窄屏输入框行为正常。
- 旧版桌面托管的 `dsh-at-file` 在升级后不再加载，不出现重复菜单、重复引用或残留 bundle。
- 用户自行安装的同名插件不被删除；若与原生能力冲突，应给出人工卸载提示而不是静默修改用户依赖。

`rc.8` 同时调整了 SQLite 后端，官方说明其存储结构不兼容；升级到 `0.1.1-rc.2` 仍必须完成以下验证：

- 使用隔离副本验证 `preview.12` 用户数据升级到 `preview.13` 后，会话读取、新建、分叉和继续对话正常。
- 验证运行中升级、连续 payload 升级、候选失败回退和卸载保留 `~/.dsh` 的既有边界。
- 发布说明明确指出：新内核写入的数据不能假定可被旧内核读取，安装旧桌面版不等于完成数据回滚。
- 在确认真实迁移与回退行为前，不得承诺无损降级；发现数据迁移阻断时停止本周发布。

## 本周实施清单

- [x] 将桌面版本统一更新为 `0.1.0-preview.13`。
- [x] 将 DSH `0.1.1-rc.2`、Market、pnpm 和 Node 的最终选择写入 `runtime.lock.json`，更新 integrity 和运行时包锁。
- [x] 更新全部目标插件的版本、来源、integrity、归档 commit/SHA-256、delivery 和许可证信息。
- [x] 从插件锁、依赖、托管开关、暂存校验、测试和用户文档中移除桌面托管的 `dsh-at-file` 与 ModLens；用户自装或接管的同名插件保留。
- [x] 更新 GenUI Skill 版本和内容 SHA-256；`0.8.6` 的 Skill 内容摘要与 `0.8.4` 相同。
- [x] 更新第三方声明、候选发布说明、About 文案和 bundle 数量；公开 README 的版本表仍展示当前已发布的 `preview.12`。
- [x] 在开发工作区重新完成 runtime/plugin staging、格式检查、Clippy、`100 + 12` 项 Rust 测试、21 项 Node 测试、
  `82.51%` 核心行覆盖率、三组零漏洞 audit、pnpm 兼容矩阵和 payload verify；旧 `rc.8` 候选报告不复用。
- [x] 对 Sidebar 已内联的 Mermaid 依赖执行 lockfile 可达性裁剪；payload 为 11,329 个文件、
  242,194,493 字节，三个 ZIP 共 93,159,695 字节（88.84 MiB），继续满足 90 MiB 预算。
- [x] 确认公开 `preview.12` 没有完整 release-gate 报告；安装器、同名 `.sha256` 与 GitHub Asset
  digest 三方基线均为 `df1ce1376ca57395492bbf5f53d4b56c840c23e1527b53cf347abd09b6927d82`，不补造历史报告。
- [x] 完成 DSH `0.1.1-rc.2` 原生视觉、文件/会话引用与 SQLite 升级验证。
- [x] 在干净 release worktree 完成两次可复现构建、各插件聚焦验证、9 场景升级矩阵和 20 对启动性能比较。
- [x] 记录 release commit、安装器 SHA-256、payload digest、门禁报告与最终发布地址。

### 发布结果

- Release commit：`d1974dfc599eae575952847aac68802c30331ea7`
- 安装器 SHA-256：`451de3f5ed4966f6c3aa4cc1dcbe90cf4dad91ce61ec11d5e75cbb96b33ebc81`
- Payload digest：`b8fd07c04725078ee43fb8eaeb65b46c214198aee6998205c696c8cc3b43be78`
- Release URL：https://github.com/licn9901-arch/deepseek-harness-desktop/releases/tag/v0.1.0-preview.13
- 资产审计：18 个资产回下载后名称、大小和 SHA-256 全部一致；公开安装器 URL 返回 `HTTP 206`。
- 门禁决策：同一提交的 14 个行为阶段全部通过，最终重新构建修复了旧 build-report 身份；根据发布指令豁免再次重复完整统一门禁，正式报告保留 `approved-with-user-waiver` 记录。

## 发布记录

| 周次 | 桌面版本 | DSH | Market | Bundle/Skill | 状态 | 结果或阻断原因 |
|---|---|---|---|---:|---|---|
| 2026-W34 | `0.1.0-preview.13` | `0.1.1-rc.2` | `1.17.1` | 7 / 1 | 已发布 | 原生视觉与 SQLite 实机验收通过；18 个资产公开审计一致，安装器 87.46 MiB，payload 88.84 MiB；最终重复统一门禁按发布指令豁免并留痕 |

后续每周复制上一行并更新实际结果，不覆盖历史记录。版本未变化时也要明确写“保持不变”，不能用空白表示。

## 每周记录模板

```markdown
## YYYY-Www 版本计划

- 盘点日期：YYYY-MM-DD
- 当前桌面版：vX.Y.Z
- 本周计划版：vX.Y.Z
- 候选冻结时间：待定
- 最终状态：计划中 / 阻断 / 已发布

### 版本矩阵

| 组件 | 当前版本 | 线上最新 | 本周目标 | 处理与原因 |
|---|---:|---:|---:|---|

### 兼容性与权限变化

- 待填写。

### 验证结果

- 聚焦验证：待执行。
- 完整门禁：待执行。
- 人工验收：待执行。

### 发布结果

- Release commit：待定
- 安装器 SHA-256：待定
- Payload digest：待定
- Release URL：待定
```
