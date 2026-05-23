const PROFILE_CSS: &str = include_str!("../styles/profile.css");

/// Wrap osu! profile page HTML fragment with the necessary CSS.
///
/// The API returns only a BBcode HTML fragment that depends on osu! website
/// stylesheets. This function injects the relevant CSS extracted from osu-web
/// (<https://github.com/ppy/osu-web>, AGPLv3) so the fragment renders correctly
/// standalone.
pub fn wrap_osu_profile_html(html: &str, profile_hue: u16) -> String {
    let css = PROFILE_CSS
        .replace("{{PROFILE_HUE}}", &profile_hue.to_string())
        .replace(
            "{{VIEWPORT_WIDTH}}",
            &crate::PROFILE_VIEWPORT_WIDTH.to_string(),
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
        let result = wrap_osu_profile_html("", 333);
        assert!(result.starts_with("<!DOCTYPE html>"));
        assert!(result.contains("--base-hue: 333"));
        assert!(result.contains("<body>\n\n</body>"));
    }

    #[test]
    fn test_wrap_with_content() {
        let html = r#"<div class="bbcode">Hello World</div>"#;
        let result = wrap_osu_profile_html(html, 200);
        assert!(result.contains("--base-hue: 200"));
        assert!(result.contains(html));
    }

    #[test]
    fn test_wrap_includes_css_sections() {
        let result = wrap_osu_profile_html("<p>test</p>", 100);
        assert!(result.contains(".bbcode"));
        assert!(result.contains(".spoiler"));
        assert!(result.contains(".bbcode-spoilerbox"));
        assert!(result.contains(".imagemap"));
    }
}
