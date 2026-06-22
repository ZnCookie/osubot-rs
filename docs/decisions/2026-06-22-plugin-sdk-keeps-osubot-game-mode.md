# plugin-sdk 保持 `osubot_game_mode::GameMode` 直接引用

日期：2026-06-22
状态：已决定

## 背景

GameMode 单一来源重构计划 Task 2 原本要求将 `osubot-plugin-sdk` 迁移到
`osubot_types::GameMode`。审计发现此变更会破坏 SDK 的 `no_std` 契约与 WASM 插件 ABI，
故跳过源码改动，仅记录本决策。

## 受影响 crate

`osubot-plugin-sdk/` —— 暴露给第三方插件作者使用，被编译为
`wasm32-unknown-unknown` 目标，**必须**保持 `no_std` 兼容。

## 关键事实

`osubot-plugin-sdk/Cargo.toml`：

```toml
serde = { version = "1", default-features = false, features = ["derive", "alloc"] }
serde_json = { version = "1", default-features = false, features = ["alloc"] }
osubot-game-mode = { path = "../osubot-game-mode" }
```

`osubot-plugin-sdk/src/lib.rs:6` 使用 `extern crate alloc;`，仅依赖 `core::ptr`、
`core::slice`、`core::str` —— 显式 `no_std` 设计。

`osubot-types/Cargo.toml:7` 依赖 `chrono = "0.4"`，而 `chrono` 是 std-only。

`osubot-types/src/lib.rs:4` 仅为 `pub use osubot_game_mode::GameMode;` 的重导出。

## 决策

`osubot-plugin-sdk` 继续直接依赖 `osubot_game_mode`，**不**引入 `osubot-types`。
`Cargo.toml` 与 `src/lib.rs` 维持现状。

## 为何可接受

1. **类型同一**：`osubot_types::GameMode` 与 `osubot_game_mode::GameMode` 指向
   同一类型（前者为重导出），不构成重复定义。
2. **职责清晰**：`osubot_game_mode` 是定义之家，`osubot_types` 仅作便利重导出
   与宿主代码的导入路径。
3. **ABI 稳定**：WASM 插件二进制接口不依赖 `osubot-types`，避免 `chrono` 牵连
   进入插件产物。
4. **消费方一致**：所有宿主侧代码（`osubot`、`osubot-plugin`、`osubot-core` 等）
   已统一从 `osubot_types::GameMode` 导入，单一来源原则在外部视角仍然成立。

## 验证

```bash
cargo build --locked --workspace
cargo test --locked --workspace
cargo clippy --locked --workspace -- -D warnings
```

均应通过（无代码改动，仅追加本文件）。
