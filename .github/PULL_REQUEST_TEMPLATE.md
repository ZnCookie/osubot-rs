## 标题格式

请使用以下格式：`<type>: <核心变更>`

其中 `<type>` 必须使用以下英文类型之一：

| 类型 | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `refactor` | 重构（不改变功能） |
| `docs` | 文档更新 |
| `deps` | 依赖更新 |
| `chore` | 其他（构建、CI、工具等） |

示例：`feat: 添加 TA 模式 PP 计算`、`fix: 修复用户绑定查询超时`

## 概述

<!-- 简要描述这个 PR 做了什么，解决什么问题 -->

## 变更内容

<!-- 列出具体变更，格式如下：
- **模块名称**：问题/现象 → 解决方案
- **另一个模块**：...
-->

## 变更类型

- [ ] `feat` 新功能
- [ ] `fix` Bug 修复
- [ ] `refactor` 重构
- [ ] `docs` 文档更新
- [ ] `deps` 依赖更新
- [ ] `chore` 其他

## 相关 Issue

<!-- 关联的 Issue，如：Closes #123、Fixes #456 -->

## 性能影响

<!-- 如适用：说明是否影响查询速度、内存占用、渲染性能等 -->

## 验证

- [ ] `cargo clippy --locked -- -D warnings` 通过
- [ ] `cargo fmt --check` 通过
- [ ] `cargo build --release --locked` 通过
- [ ] `cargo test --locked --workspace` 通过
- [ ] `cargo audit` 通过
- [ ] `cargo +nightly udeps` 通过（如 CI 触发）
- [ ] 已添加/更新单元测试（如适用）
- [ ] 已添加/更新集成测试（如适用）

## 检查清单

- [ ] 未引入新的 warning
- [ ] 遵循项目命名规范（snake_case 函数/模块，PascalCase 类型，SCREAMING_SNAKE_CASE 常量）
- [ ] 日志使用 `log_fmt!` 宏
- [ ] 用户可见文本使用 `user_str()` 函数
- [ ] 未提交敏感信息（API 密钥等）
- [ ] 新增功能已添加测试覆盖
- [ ] 代码变更符合项目代码规范（详见 [`CONTRIBUTING.md`](../CONTRIBUTING.md)）

## WASM 插件相关（如适用）

- [ ] 插件中未使用 `unwrap()`/`expect()`
- [ ] 使用 `get_plugin_config()` 获取配置
- [ ] 未直接访问文件系统

## 许可确认

- [ ] 我同意将此代码贡献给项目，并遵循项目许可证（AGPL-3.0）
