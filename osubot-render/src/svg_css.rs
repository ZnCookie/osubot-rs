//! 在 usvg/resvg parse 之前预处理内联 SVG：把 `var(--name)` 替换成立面值，
//! 加 `--circ-opacity` 兜底。
//!
//! ## 为什么需要
//!
//! `usvg 0.45.1` 用 `simplecss 0.2.2` 解析 SVG 的 `<style>` 块。`simplecss`
//! **不支持 CSS 变量**（自定义属性）。当 SVG 用了：
//!
//! ```xml
//! <style>.club-60 { --accent1: 255, 89, 111; }</style>
//! <rect fill="rgb(var(--accent1))"/>
//! ```
//!
//! usvg 解析 `stop-color="rgb(var(--accent1))"` 时，调用
//! `svgtypes::Color::from_str`，**不认识 `var()`**，fallback 到 `Color::black()`
//! （`usvg/src/parser/paint_server.rs`）。整条 linearGradient 渲成黑色。
//!
//! 同样地，`<g fill="rgba(var(--accent3), var(--circ-opacity))">` 里的 `var(--circ-opacity)`
//! usvg 也解析不出来，alpha 通道 fallback 到 1.0，circle 变实色。
//!
//! ## 我们的方案
//!
//! 在 `inline_external_images` 把 SVG 嵌进 data URI **之前**调用本模块的
//! `resolve_svg_css_vars`，把所有 `var(--name)` 替换成对应作用域下的字面值。
//! usvg 拿到的是已经"扁平化"的 SVG，不需要解析 CSS 变量。
//!
//! resvg 0.45.1 同样不解析 CSS 变量（底层用同一个 usvg），所以 rasterize 路径也
//! 需要 preprocessor。
//!
//! ## 只取 `*` / `:root` 规则
//!
//! CSS 变量级联：直接应用到元素的规则（即使 specificity 只有 0,0,0）压过继承值。
//! osu! profile SVG（`<svg class="club-60">`）的可见子元素（`<stop>`/`<rect>`/`<circle>` 等）
//! **无 class**，只命中 `*` (0,0,0) → 拿到 `*` 块的值。`.club-60` 的红值被 .class 隔离，
//! 不会作用到子元素上。这跟 osekai.net 浏览器渲染结果一致。
//!
//! 因此我们**忽略 class 规则**，只取 `*` 和 `:root` 块。
//!
//! ## `--circ-opacity` 兜底
//!
//! osu! SVG 里 `--circ-opacity: 0.25` 定义在 `.club-XX` 块里（不在 `*` 块里）。
//! preprocessor 忽略 class，所以这个 var 解析不出来。兜底 0.25 保持原视觉
//! （usvg fallback 1.0 会让 circle 变实色，突兀）。

use std::collections::HashMap;

/// 预处理 SVG 文本：
/// 1. 把 `var(--name)` 替换成 `*` / `:root` 规则里的字面值
/// 2. 给 `--circ-opacity` 兜底 0.25
///
/// resvg 0.45.1 通过 rasterize 路径接管 `<pattern>`/`<filter>`/mix-blend-mode，
/// preprocessor 不需要再剥除这些。
pub fn resolve_svg_css_vars(svg: &str) -> String {
    let collected = collect_css_vars(svg);
    let mut effective = effective_vars_for_root(&collected);
    // 兜底：usvg/resvg 不解析 var()，会 fallback（color→黑 / alpha→1.0）
    // osu! SVG 里所有 .club-XX 都是 0.25，先用这个保守值
    effective
        .entry("--circ-opacity".to_string())
        .or_insert_with(|| "0.25".to_string());

    substitute_vars(svg, &effective)
}

/// 收集所有 `<style>` 块里的 CSS 变量声明。
struct CollectedVars {
    /// 选择器 → (变量名 → 值)
    rules: Vec<(String, HashMap<String, String>)>,
}

fn collect_css_vars(svg: &str) -> CollectedVars {
    let mut rules = Vec::new();
    let mut search_from = 0;
    while let Some(rel_start) = svg[search_from..].find("<style") {
        let abs_start = search_from + rel_start;
        let after_tag_open = svg[abs_start..].find('>').map(|i| abs_start + i + 1);
        let Some(after_tag_open) = after_tag_open else {
            break;
        };
        let close_rel = svg[after_tag_open..].find("</style>");
        let Some(close_rel) = close_rel else {
            break;
        };
        let style_body = &svg[after_tag_open..after_tag_open + close_rel];
        for (selector, decls) in parse_style_block(style_body) {
            if !decls.is_empty() {
                rules.push((selector, decls));
            }
        }
        search_from = after_tag_open + close_rel + "</style>".len();
    }
    CollectedVars { rules }
}

fn parse_style_block(css: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut out = Vec::new();
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            if let Some(end) = css[i + 2..].find("*/") {
                i += 2 + end + 2;
                continue;
            } else {
                break;
            }
        }
        let brace_rel = css[i..].find('{');
        let Some(brace_rel) = brace_rel else {
            break;
        };
        let selector = css[i..i + brace_rel].trim();
        let decl_start = i + brace_rel + 1;
        let Some(close_rel) = find_matching_brace(&css[decl_start..]) else {
            break;
        };
        let decls_str = &css[decl_start..decl_start + close_rel];
        let mut decls = HashMap::new();
        for decl in decls_str.split(';') {
            let decl = decl.trim();
            if decl.is_empty() {
                continue;
            }
            if let Some((name, value)) = decl.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if name.starts_with("--") && !name.is_empty() && !value.is_empty() {
                    decls.insert(name.to_string(), value.to_string());
                }
            }
        }
        if !decls.is_empty() {
            out.push((selector.to_string(), decls));
        }
        i = decl_start + close_rel + 1;
    }
    out
}

fn find_matching_brace(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn effective_vars_for_root(collected: &CollectedVars) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (selector, decls) in &collected.rules {
        if matches!(selector.as_str(), ":root" | "*") {
            for (k, v) in decls {
                result.insert(k.clone(), v.clone());
            }
        }
    }
    result
}

fn substitute_vars(svg: &str, vars: &HashMap<String, String>) -> String {
    if vars.is_empty() {
        return svg.to_string();
    }
    let mut result = String::with_capacity(svg.len());
    let bytes = svg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((attr_start, attr_end)) = find_next_attr_value(svg, i) {
            result.push_str(&svg[i..attr_start]);
            let attr_value = &svg[attr_start + 1..attr_end];
            let new_value = substitute_var_refs(attr_value, vars);
            result.push(svg.as_bytes()[attr_start] as char);
            result.push_str(&new_value);
            result.push(svg.as_bytes()[attr_end] as char);
            i = attr_end + 1;
        } else {
            result.push_str(&svg[i..]);
            break;
        }
    }
    result
}

fn find_next_attr_value(s: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'=' && i > from {
            if bytes[i - 1] == b'/' {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            let quote = bytes[j];
            if quote != b'"' && quote != b'\'' {
                i += 1;
                continue;
            }
            let value_start = j;
            let mut k = value_start + 1;
            while k < bytes.len() && bytes[k] != quote {
                k += 1;
            }
            if k >= bytes.len() {
                return None;
            }
            return Some((value_start, k));
        }
        i += 1;
    }
    None
}

fn substitute_var_refs(s: &str, vars: &HashMap<String, String>) -> String {
    if !s.contains("var(") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 4 <= bytes.len() && &bytes[i..i + 4] == b"var(" {
            let Some(close_rel) = s[i + 4..].find(')') else {
                out.push(s.as_bytes()[i] as char);
                i += 1;
                continue;
            };
            let inner = &s[i + 4..i + 4 + close_rel];
            let var_name = inner.split(',').next().unwrap_or("").trim();
            if let Some(value) = vars.get(var_name) {
                out.push_str(value);
            } else {
                out.push_str(&s[i..i + 4 + close_rel + 1]);
            }
            i += 4 + close_rel + 1;
        } else {
            let ch_end = next_char_boundary(s, i);
            out.push_str(&s[i..ch_end]);
            i = ch_end;
        }
    }
    out
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    let bytes = s.as_bytes();
    let mut j = i + 1;
    while j < bytes.len() && (bytes[j] & 0b1100_0000) == 0b1000_0000 {
        j += 1;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_class_var_declarations() {
        let css = ".club-60 { --accent1: 255, 89, 111; --accent2: 0, 0, 0; }";
        let rules = parse_style_block(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].0, ".club-60");
        assert_eq!(rules[0].1.get("--accent1").unwrap(), "255, 89, 111");
    }

    #[test]
    fn parses_global_var_declarations() {
        let css = "* { --angle: 0; font-family: sans-serif; }";
        let rules = parse_style_block(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].0, "*");
        assert_eq!(rules[0].1.get("--angle").unwrap(), "0");
    }

    #[test]
    fn resolves_var_in_stop_color() {
        let svg = r#"<svg class="test" xmlns="http://www.w3.org/2000/svg">
<style>* { --c1: 255, 0, 0; }</style>
<defs>
  <linearGradient id="g">
    <stop offset="0%" stop-color="rgb(var(--c1))"/>
  </linearGradient>
</defs>
</svg>"#;
        let result = resolve_svg_css_vars(svg);
        assert!(result.contains("rgb(255, 0, 0)"), "result: {result}");
        assert!(
            !result.contains("var(--c1)"),
            "var() not substituted: {result}"
        );
    }

    #[test]
    fn resolves_var_with_global_and_class() {
        const SVG: &str = r#"<svg class="c60" xmlns="http://www.w3.org/2000/svg">
<style>
* { --accent: 0, 128, 255; }
.c60 { --accent: 255, 89, 111; }
</style>
<rect fill="rgb(var(--accent))"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains("rgb(0, 128, 255)"), "got: {result}");
        assert!(!result.contains("rgb(255, 89, 111)"), "got: {result}");
    }

    #[test]
    fn class_on_root_is_ignored_for_child_vars() {
        const SVG: &str = r#"<svg class="c60" xmlns="http://www.w3.org/2000/svg">
<style>.c60 { --c1: 0, 0, 255; }</style>
<style>* { --c1: 255, 89, 111; }</style>
<rect fill="rgb(var(--c1))"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains("rgb(255, 89, 111)"), "got: {result}");
        assert!(!result.contains("rgb(0, 0, 255)"), "got: {result}");
    }

    #[test]
    fn no_substitution_when_no_vars() {
        let svg = r#"<svg><rect fill="red"/></svg>"#;
        let result = resolve_svg_css_vars(svg);
        assert_eq!(result, svg);
    }

    #[test]
    fn handles_var_in_rgba() {
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">
<style>* { --c: 97, 76, 227; --a: 0.25; }</style>
<circle fill="rgba(var(--c), var(--a))"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(
            result.contains("rgba(97, 76, 227, 0.25)"),
            "result: {result}"
        );
    }

    #[test]
    fn preserves_unknown_var() {
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">
<style>* { --defined: red; }</style>
<rect fill="var(--undefined)"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains("var(--undefined)"), "result: {result}");
    }

    #[test]
    fn ignores_style_block_var_in_css() {
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">
<style>* { --c: red; color: var(--c); }</style>
<rect fill="var(--c)"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains(r#"fill="red""#), "result: {result}");
    }

    #[test]
    fn osu_profile_svg_pattern() {
        const SVG: &str = r#"<svg width="100" height="40" class="club-60" xmlns="http://www.w3.org/2000/svg">
<style>
.club-60 { --accent1: 255, 89, 111; --accent2: 244, 139, 150; }
* { --angle: 0; font-family: sans-serif; }
</style>
<style>* { --accent1: 135, 206, 235; --accent2: 0, 0, 128; --angle: 119; }</style>
<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="100" y2="40">
  <stop offset="0%" stop-color="rgb(var(--accent1))"/>
  <stop offset="100%" stop-color="rgb(var(--accent2))"/>
</linearGradient>
<rect width="100" height="40" fill="url(#g)"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains("rgb(135, 206, 235)"), "got: {result}");
        assert!(result.contains("rgb(0, 0, 128)"), "got: {result}");
        assert!(!result.contains("rgb(255, 89, 111)"), "got: {result}");
    }

    // --circ-opacity 兜底 — usvg/resvg 不解析 var()，alpha fallback 1.0

    #[test]
    fn circ_opacity_fallback_when_only_in_class() {
        // .club-60 定义 --circ-opacity: 0.25，但 preprocessor 忽略 class
        // 兜底：必须用 0.25，否则 usvg/resvg 拿 var() 默认 1.0 → circle 实色
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">
<style>.club-60 { --circ-opacity: 0.25; }</style>
<circle fill="rgba(255, 0, 0, var(--circ-opacity))"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(
            result.contains("rgba(255, 0, 0, 0.25)"),
            "circ-opacity should be 0.25: {result}"
        );
    }

    #[test]
    fn circ_opacity_in_star_rule_wins() {
        // 如果 * 块里定义了 --circ-opacity，用 * 的值
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg">
<style>* { --circ-opacity: 0.5; }</style>
<circle fill="rgba(255, 0, 0, var(--circ-opacity))"/>
</svg>"#;
        let result = resolve_svg_css_vars(SVG);
        assert!(result.contains("rgba(255, 0, 0, 0.5)"), "result: {result}");
    }
}
