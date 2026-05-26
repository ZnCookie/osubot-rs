const SCORE_PANEL_CSS: &str = include_str!("../../styles/score_panel.css");

pub fn wrap_score_panel_css(html: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<style>
{style}
</style>
</head>
<body>
{content}
</body>
</html>"#,
        style = SCORE_PANEL_CSS,
        content = html
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_empty() {
        let result = wrap_score_panel_css("");
        assert!(result.starts_with("<!DOCTYPE html>"));
        assert!(result.contains(".score-panel"));
    }

    #[test]
    fn test_wrap_with_content() {
        let result = wrap_score_panel_css("<div>test</div>");
        assert!(result.contains("<div>test</div>"));
    }
}
