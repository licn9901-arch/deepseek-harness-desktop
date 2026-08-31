# DeepSeek Harness Desktop v0.1.0-preview.14

这是 Windows x64 社区预览版。本项目不是 DeepSeek 官方产品，也不代表 DeepSeek 官方立场。

## 本版内容

- 本地候选仅更新桌面版与桌面托管组件版本号至 `0.1.0-preview.14`；未变更 DSH、Market、内置插件或托管 Skill 的锁定输入。
- DSH 内核保持 `0.1.1-rc.2`，Market 保持 `1.17.1`；GenUI、Better Sidebar、Skin Center、Hindsight 与 Skills/MCP Manager 均保持上一版已验证的锁定版本。
- 本候选只生成本地安装包，不创建 Release、不推送 tag，也不上传任何资产。

## 视觉能力边界

- 只有选择 `DeepSeek-V4-Flash-Vision-Exp` 或其他明确支持图片的原生多模态模型时，DSH 才能直接理解图片。
- 纯文本模型不再获得 ModLens 的外部视觉端点或本机 Agent CLI 回退；本版本不应被理解为所有模型自动支持图片。

## 数据兼容提醒

- DSH `rc.8` 起调整了 SQLite 存储结构，`0.1.1-rc.2` 写入的数据不能假定可由旧内核读取。
- 手动安装旧桌面版只会回退应用与 payload，不等于完成数据回滚。
- 若要发布，仍须使用隔离的数据副本完成 `preview.13` 到 `preview.14` 的会话读取、新建、分叉、继续对话和候选失败回退验证。

## 安装提示

- 支持 Windows 10 22H2 / Windows 11 x64。
- 请先核对随 Release 提供的 `.sha256` 文件。
- 安装包未提供 Authenticode 签名，SmartScreen 可能显示“未知发布者”；这不影响 SHA-256 校验的必要性。
- 卸载不会删除 DSH 用户会话、profile、插件状态、Skill 或第三方配置。
