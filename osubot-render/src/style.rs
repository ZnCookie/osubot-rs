const PROFILE_CSS: &str = include_str!("../styles/profile.css");

/// Wrap osu! profile page HTML fragment with the necessary CSS.
///
/// The API returns only a BBcode HTML fragment that depends on osu! website
/// stylesheets. This function injects the relevant CSS extracted from osu-web
/// (<https://github.com/ppy/osu-web>, AGPLv3) so the fragment renders correctly
/// standalone.
pub fn wrap_osu_profile_html(
    html: &str,
    profile_hue: u16,
    avatar_url: &str,
    username: &str,
) -> String {
    let header = format!(
        r#"<div style="display:flex;justify-content:center;align-items:center;margin-bottom:20px">
<img src="{}" style="width:200px;height:200px;border-radius:50%">
</div>
<div class="user-name" style="text-align:center;margin-bottom:20px">{}</div>
<hr style="border:0;height:1px;background:#ffffff;margin:20px 0">"#,
        avatar_url, username
    );
    let css = PROFILE_CSS
        .replacen("{{PROFILE_HUE}}", &profile_hue.to_string(), 1)
        .replacen(
            "{{VIEWPORT_WIDTH}}",
            &crate::PROFILE_VIEWPORT_WIDTH.to_string(),
            2,
        );
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<style>
{css}
</style>
</head>
<body>
{header}
{html}
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_empty_html() {
        let result = wrap_osu_profile_html("", 333, "https://a.ppy.sh/1", "testuser");
        assert!(result.starts_with("<!DOCTYPE html>"));
        assert!(result.contains("--base-hue: 333"));
        assert!(result.contains("testuser"));
        assert!(result.contains("a.ppy.sh/1"));
        assert!(result.contains("<hr"));
    }

    #[test]
    fn test_wrap_with_content() {
        let html = r#"<div class="bbcode">Hello World</div>"#;
        let result = wrap_osu_profile_html(html, 200, "https://a.ppy.sh/2", "foo");
        assert!(result.contains("--base-hue: 200"));
        assert!(result.contains(html));
    }

    #[test]
    fn test_wrap_includes_css_sections() {
        let result = wrap_osu_profile_html("<p>test</p>", 100, "https://a.ppy.sh/3", "bar");
        assert!(result.contains(".bbcode"));
        assert!(result.contains(".spoiler"));
        assert!(result.contains(".bbcode-spoilerbox"));
    }
}
