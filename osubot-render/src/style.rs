const PROFILE_CSS: &str = include_str!("../styles/profile.css");

pub(crate) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Wrap osu! profile page HTML fragment with the necessary CSS.
///
/// `avatar_data_uri` should be a `data:image/...;base64,...` URI produced by
/// [`crate::image_to_data_uri`]. Pass an empty string to omit the avatar header
/// image (e.g. when the avatar URL was empty or the download failed).
#[must_use]
pub fn wrap_osu_profile_html(
    html: &str,
    profile_hue: u16,
    avatar_data_uri: &str,
    username: &str,
) -> String {
    let css = PROFILE_CSS
        .replace("{{PROFILE_HUE}}", &profile_hue.to_string())
        .replace(
            "{{VIEWPORT_WIDTH}}",
            &crate::PROFILE_VIEWPORT_WIDTH.to_string(),
        );
    let mut ctx = tera::Context::new();
    ctx.insert("html", html);
    ctx.insert("avatar_data_uri", avatar_data_uri);
    ctx.insert("username", username);
    ctx.insert("css", &css);
    crate::template::render("profile.html", &ctx)
}

/// 构造 `render_profile_card` 喂给渲染器的最终 HTML 字符串：
/// 先用 `cache::inline_external_images` 把 BBcode 里的外链图片下载并转成 base64 data URI，
/// 再用 `wrap_osu_profile_html` 包上 avatar 头部和 CSS。
///
/// `avatar_data_uri` 由调用方预取并生成（见 [`crate::render_profile_card`]）。
/// 传空串则省略头像 `<img>`，仍能渲染其他内容。
///
/// 这是 profile 卡片"输入渲染器之前"的中间表示。`render_profile_card` 内部也走这里，
/// 集成测试 `osubot-render/tests/dump_profile_html.rs` 复用同一入口 dump 到磁盘。
#[must_use]
pub async fn build_profile_html(
    html: &str,
    profile_hue: u16,
    avatar_data_uri: &str,
    username: &str,
) -> String {
    let inlined = crate::cache::inline_external_images(html).await;
    wrap_osu_profile_html(&inlined, profile_hue, avatar_data_uri, username)
}

/// 格式化 pp 变化量的小数位(整数去掉小数点,小数保留 2 位)
fn format_pp_delta(delta: f64) -> String {
    if delta.fract().abs() < f64::EPSILON {
        format!("{}", delta as i64)
    } else {
        format!("{delta:.2}")
    }
}

/// 渲染 pp 变化 HTML（已转义，可安全用于 Tera `| safe`）。
///
/// 输出格式: `<span class="user-pp-change up|down|zero">±N</span>`
/// 所有数值通过固定格式字符串构造，不含未转义的用户输入。
///
/// `None` → 空字符串
#[must_use]
pub(crate) fn format_pp_change_html(change: Option<f64>) -> String {
    match change {
        Some(delta) if delta > 0.0 => {
            format!(
                r#"<span class="user-pp-change up">+{}</span>"#,
                format_pp_delta(delta)
            )
        }
        Some(delta) if delta < 0.0 => {
            format!(
                r#"<span class="user-pp-change down">{}</span>"#,
                format_pp_delta(delta)
            )
        }
        Some(_) => r#"<span class="user-pp-change zero">±0</span>"#.to_string(),
        _ => String::new(),
    }
}

/// 渲染 rank 变化 HTML（已转义，可安全用于 Tera `| safe`）。
///
/// 输出格式: `<span class="rank-change up|down|zero">±N</span>`
/// 所有数值通过固定格式字符串构造，不含未转义的用户输入。
///
/// `None` → 空字符串
#[must_use]
pub(crate) fn format_rank_change_html(change: Option<i64>) -> String {
    match change {
        Some(delta) if delta > 0 => format!(r#"<span class="rank-change up">+{}</span>"#, delta),
        Some(delta) if delta < 0 => format!(r#"<span class="rank-change down">{}</span>"#, delta),
        Some(_) => r#"<span class="rank-change zero">±0</span>"#.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_empty_html() {
        let result = wrap_osu_profile_html("", 333, "", "testuser");
        assert!(result.starts_with("<!DOCTYPE html>"));
        assert!(result.contains("--base-hue: 333"));
        assert!(result.contains("testuser"));
        assert!(result.contains("<hr"));
        // 空 data URI 时不输出 <img>
        assert!(!result.contains("<img"));
    }

    #[test]
    fn test_wrap_with_content() {
        let html = r#"<div class="bbcode">Hello World</div>"#;
        let result = wrap_osu_profile_html(html, 200, "data:image/jpeg;base64,FAKE", "foo");
        assert!(result.contains("--base-hue: 200"));
        assert!(result.contains(html));
        assert!(result.contains("<img"));
        assert!(result.contains("data:image/jpeg;base64,FAKE"));
    }

    #[test]
    fn test_wrap_includes_css_sections() {
        let result = wrap_osu_profile_html("<p>test</p>", 100, "", "bar");
        assert!(result.contains(".bbcode"));
        assert!(result.contains(".spoiler"));
        assert!(result.contains(".bbcode-spoilerbox"));
    }

    #[test]
    fn test_wrap_empty_avatar_omits_img_tag() {
        let result = wrap_osu_profile_html("<p>hi</p>", 333, "", "alice");
        assert!(!result.contains("<img"));
    }

    #[test]
    fn test_wrap_data_uri_avatar_includes_img_tag() {
        let data_uri = "data:image/jpeg;base64,AAAA";
        let result = wrap_osu_profile_html("<p>hi</p>", 333, data_uri, "alice");
        assert!(result.contains("<img"));
        assert!(result.contains(data_uri));
    }
}
