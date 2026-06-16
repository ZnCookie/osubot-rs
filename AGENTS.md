# AGENTS.md

This file provides guidance to Agents when working with code in this repository. Keep this file concise — do not exceed 60 lines without user approval.

## 项目概述

osubot-rs 是通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。所有用户可见文本为中文。与用户沟通时尽量使用中文。通过 WebSocket 连接 QQ 机器人框架（go-cqhttp/Lagrange），解析群消息，调用 osu! API v2，返回文本或渲染图片。

## 构建与开发

完成代码修改后必须执行以下步骤

```bash
cargo clippy --locked -- -D warnings  # lint（CI 将警告视为错误）
cargo fmt --check                     # 格式检查
cargo build --release --locked        # 构建
cargo test --locked --workspace       # 测试
```
完成上述测试后，检查文档和配置文件是否过时/不正确。永远不要在用户没有同意的前提下提交git。**永远不要将 `docs/superpowers/` 目录下的文件提交到 git。**

## PR 提交规范

创建或更新 PR 时遵循以下格式：

### 标题
`<type>: <核心变更>`

### 正文
```
## 概述
<简述做了什么>

## 变更内容
- **<模块>**：<问题/现象> → <怎么解决的>
- ...

## 验证
- cargo clippy -- -D warnings
- cargo fmt --check
- cargo build --release
- cargo test --workspace
```

### 注意事项
使用 `gh pr edit` 时，body 若包含反引号、`$`、`{}` 等特殊字符，需用单引号包裹或写入临时文件后以 `--body-file` 传入，避免 bash 将其解析为命令。
