//! 端到端回归测试：blitz 通过 usvg/anyrender_svg 把内联 SVG 真的画到 RGBA buffer。
//!
//! 防回归：2026-06-21 bug — `blitz 0.2.1` 内部用 `default-features = false` 引用
//! `blitz-paint 0.2.1`，覆盖了 `blitz-paint` 自己的 `default = ["svg"]`，
//! 导致 `anyrender_svg` 永远不在依赖图里，`blitz-paint` 里的 `cx.draw_svg()`
//! 是 `#[cfg(feature = "svg")]` 死代码。SVG 被解析成 `ImageData::Svg` 但永远
//! 不被画。修法：在 `osubot-render` 直接拉 `blitz-paint` 显式开 `svg` feature。
//!
//! 不需要 osu! 账号或网络。`cargo test` 默认跑。
//!
//! 必须用 `flavor = "multi_thread"`：blitz-net 通过 `tokio::spawn` 调度 fetch 任务，
//! 渲染主循环同步阻塞在 `mpsc::recv_timeout` 上。`current_thread` runtime 下
//! spawned task 永远不被 poll，资源 callback 永远不调用。

use base64::Engine;
use osubot_render::render_html_to_image;
use parley::FontContext;
use std::sync::atomic::AtomicBool;

/// 一个红底白圆的内联 SVG，验证 SVG 真的被画到 RGBA buffer。
const RED_DOT_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="50" height="50" fill="rgb(255,0,0)"/>
  <circle cx="25" cy="25" r="12" fill="rgb(255,255,255)"/>
</svg>"#;

/// 把 SVG 嵌进 `<img src="data:image/svg+xml;base64,...">` 的最小 HTML。
fn html_with_inline_svg(svg: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    format!(
        r#"<!DOCTYPE html><html><head><style>
html,body{{margin:0;padding:0;background:#ffffff}}
img{{display:block}}
</style></head><body>
<img src="data:image/svg+xml;base64,{b64}" width="50" height="50"/>
</body></html>"#
    )
}

fn render(html: &str, w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let font_ctx = FontContext::new();
    let cancel = AtomicBool::new(false);
    let (pixels, out_w, out_h) =
        render_html_to_image(html, &font_ctx, w, h, &cancel).expect("render_html_to_image ok");
    assert_eq!(
        (out_w, out_h),
        (w, h),
        "expected {w}x{h} output, got {out_w}x{out_h}",
    );
    assert_eq!(
        pixels.len(),
        (w as usize) * (h as usize) * 4,
        "RGBA buffer length mismatch",
    );
    (pixels, out_w, out_h)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_svg_red_background_is_painted() {
    let html = html_with_inline_svg(RED_DOT_SVG);
    let (pixels, w, _h) = render(&html, 50, 50);

    // 左上角 (0, 0) 应该是 SVG 红色
    let p = &pixels[0..4];
    let (r, g, b, a) = (p[0], p[1], p[2], p[3]);
    assert!(
        r > 200 && g < 50 && b < 50 && a == 255,
        "top-left pixel expected red (alpha=255), got r={r} g={g} b={b} a={a}",
    );

    // 中心 (25, 25) 应该是白色圆点
    let center_idx = ((25 * w + 25) * 4) as usize;
    let p = &pixels[center_idx..center_idx + 4];
    let (r, g, b, a) = (p[0], p[1], p[2], p[3]);
    assert!(
        r > 200 && g > 200 && b > 200 && a == 255,
        "center pixel expected white (alpha=255), got r={r} g={g} b={b} a={a}",
    );

    // 至少要看到非白色像素出现（证明 SVG 真的被画了）
    let non_white_count = pixels
        .chunks_exact(4)
        .filter(|px| !(px[0] > 250 && px[1] > 250 && px[2] > 250))
        .count();
    assert!(
        non_white_count > 100,
        "expected >100 non-white pixels from SVG, got {non_white_count} \
         (this means blitz didn't paint the inline SVG)",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_svg_with_css_variables_keeps_basic_shapes() {
    // 即使 CSS 变量 usvg 不支持，basic shape 仍应画出来。
    // 防 usvg parse 失败的回归。
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
  <style>.bg { fill: rgb(0, 128, 255); }</style>
  <rect class="bg" width="40" height="40"/>
</svg>"#;
    let html = html_with_inline_svg(SVG);
    let (pixels, w, _h) = render(&html, 40, 40);

    // 右下角应该是蓝色 rect
    let p_idx = ((39 * w + 39) * 4) as usize;
    let p = &pixels[p_idx..p_idx + 4];
    let (r, g, b, a) = (p[0], p[1], p[2], p[3]);
    assert!(
        b > 200 && r < 50 && g > 100 && g < 200 && a == 255,
        "bottom-right pixel expected blue (alpha=255), got r={r} g={g} b={b} a={a}",
    );
}

/// Phase 2 修复（2026-06-21）：usvg + simplecss 0.2.2 不解析 CSS 变量，
/// `var(--name)` 在 `stop-color` 里 fallback 成黑色（usvg/src/parser/paint_server.rs
/// 调用 svgtypes::Color::from_str 失败后回退 Color::black()）。
///
/// `osubot_render::svg_css::resolve_svg_css_vars` 预处理：把 `var(--name)`
/// 替换成立面值。**但 inline_external_images 才会调它** —— 这个测试直接用
/// 预处理器（不走 inline 路径），验证预处理器本身正确性。
#[test]
fn svg_css_preprocessor_resolves_vars() {
    use osubot_render::svg_css::resolve_svg_css_vars;
    // 复刻 osu! profile SVG 结构：class 在 root + `*` 兜底。
    // 预处理器只取 `*` 值，class 主题色被忽略（符合 CSS 变量级联实际语义）。
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" class="c60" width="40" height="40">
<style>.c60 { --c1: 255, 0, 0; --c2: 0, 0, 255; }</style>
<style>* { --c1: 255, 0, 0; --c2: 0, 0, 255; }</style>
<defs>
  <linearGradient id="g" x1="0%" y1="0%" x2="100%" y2="0%">
    <stop offset="0%" stop-color="rgb(var(--c1))"/>
    <stop offset="100%" stop-color="rgb(var(--c2))"/>
  </linearGradient>
</defs>
<rect width="40" height="40" fill="url(#g)"/>
</svg>"#;
    let resolved = resolve_svg_css_vars(SVG);
    assert!(
        !resolved.contains("var(--c1)"),
        "var() not substituted: {resolved}"
    );
    assert!(
        resolved.contains("rgb(255, 0, 0)"),
        "missing c1: {resolved}"
    );
    assert!(
        resolved.contains("rgb(0, 0, 255)"),
        "missing c2: {resolved}"
    );
}

/// Phase 2 端到端：渲染一个 CSS 变量驱动的渐变 SVG，验证渐变真的用变量色。
///
/// 复刻 osu! profile SVG 结构：class 在 root + `*` 兜底。预处理器**只**取 `*` 的值
/// （CSS 变量级联下子元素没 class 时直接命中 `*`，压过继承的 .class 值），
/// 跟浏览器/osekai 实际渲染一致。
///
/// **注意**：data: URI 不走 inline_external_images，所以需要测试 fetch path。
/// 但 inline_external_images 需要网络 —— 没法在无网测试里直接跑。
///
/// 折中：手动模拟 inline 流程 —— 先用 svg_css 预处理 SVG，再内联为 data: URI，
/// 再 render。这样验证「预处理 + render」组合是对的。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_svg_with_css_var_gradient_uses_literal_colors() {
    use osubot_render::svg_css::resolve_svg_css_vars;
    // var(--c*) 在 stop-color 里，usvg 默认会渲染成黑色。
    // 预处理器只取 `*` 主题色，渐变左右两端应该是红/蓝。
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" class="c60" width="40" height="40">
<style>.c60 { --c1: 255, 0, 0; --c2: 0, 0, 255; }</style>
<style>* { --c1: 255, 0, 0; --c2: 0, 0, 255; }</style>
<defs>
  <linearGradient id="g" x1="0%" y1="0%" x2="100%" y2="0%">
    <stop offset="0%" stop-color="rgb(var(--c1))"/>
    <stop offset="100%" stop-color="rgb(var(--c2))"/>
  </linearGradient>
</defs>
<rect width="40" height="40" fill="url(#g)"/>
</svg>"#;
    let resolved = resolve_svg_css_vars(SVG);
    let html = html_with_inline_svg(&resolved);
    let (pixels, w, _h) = render(&html, 40, 40);

    // 左上角 (2, 20) 应该是红色端
    let left_idx = ((20 * w + 2) * 4) as usize;
    let p = &pixels[left_idx..left_idx + 4];
    let (r, g, b, _a) = (p[0], p[1], p[2], p[3]);
    assert!(
        r > 150 && g < 100 && b < 100,
        "left edge expected reddish, got r={r} g={g} b={b}",
    );

    // 右上角 (37, 20) 应该是蓝色端
    let right_idx = ((20 * w + 37) * 4) as usize;
    let p = &pixels[right_idx..right_idx + 4];
    let (r, g, b, _a) = (p[0], p[1], p[2], p[3]);
    assert!(
        b > 150 && r < 100 && g < 100,
        "right edge expected bluish, got r={r} g={g} b={b}",
    );
}

/// Phase 1 修复（2026-06-21）：`anyrender_svg/text` feature 启用后，
/// usvg 解析 `<text>` / `<tspan>` 元素并通过 system-fonts 渲染。
///
/// **注意**：必须用具体字体名（"DejaVu Sans"），不能用 generic "sans-serif"。
/// fontdb 0.23 处理 fontconfig alias 时结构理解反了：
/// `/etc/fonts/conf.d/45-latin.conf` 里 `<alias><family>DejaVu Sans</family><default><family>sans-serif</family></default></alias>`
/// 意为 "DejaVu Sans 是 sans-serif 的 default"，但 fontdb 找 `alias == "sans-serif"` 找不到。
/// 所以 Linux 下 `font-family="sans-serif"` 不解析。
/// Mac/Windows 通常有 Arial 等 fallback，命中具体名能渲染。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_svg_text_element_is_painted() {
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20">
  <rect width="60" height="20" fill="white"/>
  <text x="2" y="14" font-family="DejaVu Sans" font-size="14" fill="black">Hello</text>
</svg>"#;
    let html = html_with_inline_svg(SVG);
    let (pixels, _w, _h) = render(&html, 60, 20);

    let black_count = pixels
        .chunks_exact(4)
        .filter(|p| p[0] < 50 && p[1] < 50 && p[2] < 50 && p[3] == 255)
        .count();
    assert!(
        black_count > 20,
        "expected >20 black pixels from text glyphs, got {black_count} \
         (this means usvg text feature is not active)",
    );
}
