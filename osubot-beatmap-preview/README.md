# osubot-beatmap-preview

osu! beatmap preview renderer, ported from [osu-beatmap-preview](https://github.com/2710165659/osu-beatmap-preview) with MIT license.

Renders beatmap preview images (GIF for standard mode, PNG for others) supporting standard, taiko, catch, and mania game modes.

## 与上游的差异

本项目在上游基础上进行了以下修改：

- **项目结构重组**：将上游的 `convert/` 目录拆分为各模式子模块（如 `convert/mania/{circle,slider,spinner}.rs`），新增 `mania/constants.rs`、`gif_common.rs`、`lib.rs` 等模块
- **安全性改进**：添加 parser 输入边界检查（防止 DoS）、sprite cache 半量淘汰、canvas bounds debug_assert、timing line 迭代上限等
- **依赖差异**：添加 `rayon` 用于 GIF 并行渲染；`Rc` 替换为 `Arc` 以支持跨线程共享
- **API 适配**：作为库使用，移除了上游的 CLI/service 层代码

## 同步版本

最新同步上游提交：`38e4fb5` (fix: 图片宽度调整, 2026-06-23)

已同步的渲染逻辑改进：
- 性能优化：canvas 快速填充、PNG 采样优化、GIF 并行渲染、text glyph 缓存
- taiko 高 BPM 标签绘制修复
- 转谱目标=原始模式时不报错
- taiko grid 支持自定义 BPM 间隔
- mania 小节线使用 beat_divisor
- mania PNG 绘制 BPM 标签
- 跳过空白区域时小节线对齐到节拍网格
- std 增加 TC (Traceable) mod 支持
