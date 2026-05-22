/// Wrap osu! profile page HTML fragment with the necessary CSS.
///
/// The API returns only a BBcode HTML fragment that depends on osu! website
/// stylesheets. This function injects the relevant CSS extracted from osu-web
/// (<https://github.com/ppy/osu-web>, AGPLv3) so the fragment renders correctly
/// standalone.
pub fn wrap_osu_profile_html(html: &str, profile_hue: u16) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<style>
/* osu! profile page CSS - extracted from osu-web (AGPLv3)
   https://github.com/ppy/osu-web */

:root {{
  --base-hue: {profile_hue};
  --font-content: 'Inter', 'Torus', 'Helvetica Neue', Tahoma, Arial, sans-serif;
  --font-default: 'Torus', 'Inter', 'Helvetica Neue', Tahoma, Arial, sans-serif;
}}

body {{
  --hsl-p: var(--base-hue), 100%, 50%;
  --hsl-h1: var(--base-hue), 100%, 70%;
  --hsl-h2: var(--base-hue), 50%, 45%;
  --hsl-c1: var(--base-hue), 40%, 100%;
  --hsl-c2: var(--base-hue), 40%, 90%;
  --hsl-l1: var(--base-hue), 40%, 80%;
  --hsl-l2: var(--base-hue), 40%, 75%;
  --hsl-l3: var(--base-hue), 40%, 70%;
  --hsl-l4: var(--base-hue), 40%, 50%;
  --hsl-d1: var(--base-hue), 20%, 35%;
  --hsl-d2: var(--base-hue), 20%, 30%;
  --hsl-d3: var(--base-hue), 20%, 25%;
  --hsl-d4: var(--base-hue), 20%, 20%;
  --hsl-d5: var(--base-hue), 20%, 15%;
  --hsl-d6: var(--base-hue), 20%, 10%;
  --hsl-f1: var(--base-hue), 10%, 60%;
  --hsl-b1: var(--base-hue), 10%, 40%;
  --hsl-b2: var(--base-hue), 10%, 30%;
  --hsl-b3: var(--base-hue), 10%, 25%;
  --hsl-b4: var(--base-hue), 10%, 20%;
  --hsl-b5: var(--base-hue), 10%, 15%;
  --hsl-b6: var(--base-hue), 10%, 10%;

  margin: 0;
  padding: 10px;
  background: hsl(var(--hsl-b6));
  color: hsl(var(--hsl-c1));
  font-family: var(--font-content);
  line-height: 1.35;
}}

/* ---- base elements ---- */

a {{
  color: hsl(var(--hsl-l2));
  text-decoration: none;
}}
a:hover {{
  color: hsl(var(--hsl-l1));
  text-decoration: underline;
}}

h2 {{
  font-size: 1.5em;
  font-style: normal;
  font-weight: bold;
  color: hsl(var(--hsl-l1));
  margin: 10px 0 0 0;
}}

blockquote {{
  font-size: inherit;
  padding: 0 0 0 20px;
  border: none;
  color: hsl(var(--hsl-c2));
  position: relative;
  margin: 0;
}}
blockquote::before {{
  content: "";
  position: absolute;
  left: 0;
  top: 0;
  width: 2px;
  height: 100%;
  border-radius: 10000px;
  background-color: hsl(var(--hsl-c2));
}}

img {{
  max-width: 100%;
}}

audio {{
  max-width: 100%;
}}

/* ---- bbcode ---- */

.bbcode {{
  font-family: var(--font-content);
  line-height: 1.5;
  overflow-wrap: anywhere;
  text-align: left;
}}

.bbcode code {{
  border-radius: 4px;
  background-color: hsl(var(--hsl-b5));
  padding: 1px 4px;
}}

.bbcode h2 {{
  font-size: 1.5em;
  font-style: normal;
  font-weight: bold;
  color: hsl(var(--hsl-l1));
}}

.bbcode blockquote {{
  font-size: inherit;
  padding: 0 0 0 20px;
  border: none;
  color: hsl(var(--hsl-c2));
  position: relative;
}}
.bbcode blockquote::before {{
  content: "";
  position: absolute;
  left: 0;
  top: 0;
  width: 2px;
  height: 100%;
  border-radius: 10000px;
  background-color: hsl(var(--hsl-c2));
}}

.bbcode pre {{
  border-radius: 4px;
  white-space: pre-wrap;
  background-color: hsl(var(--hsl-b5));
  color: inherit;
  padding: 10px;
  border: none;
  font-size: inherit;
}}

.bbcode .spoiler {{
  background-color: #000 !important;
  color: #000 !important;
}}

.bbcode .well {{
  margin: 0;
  background: hsl(var(--hsl-b5));
  border: 2px solid hsl(var(--hsl-b1));
  padding: 9px;
  border-radius: 4px;
}}

.bbcode__align-centre {{
  text-align: center;
}}

.bbcode__align-left {{
  text-align: left;
}}

.bbcode__align-right {{
  text-align: right;
}}

/* ---- spoilerbox ---- */

.bbcode-spoilerbox {{
  margin: 10px 0;
}}

.bbcode-spoilerbox__link {{
  display: flex;
  align-items: center;
  gap: 4px;
  color: hsl(var(--hsl-l2));
  text-decoration: none;
  font-weight: 600;
  font-size: 14px;
  padding: 4px 0;
}}
.bbcode-spoilerbox__link:hover {{
  color: hsl(var(--hsl-l1));
}}

.bbcode-spoilerbox__body {{
  display: block;
  padding: 8px 0 0 0;
}}

/* ---- imagemap ---- */

.imagemap {{
  position: relative;
  display: inline-block;
  max-width: 100%;
}}

.imagemap__image {{
  max-width: 100%;
  height: auto;
}}

.imagemap__link {{
  position: absolute;
  border: 2px solid rgba(255,255,255,0.7);
  border-radius: 2px;
}}

/* ---- js-dependent (visible by default since no JS) ---- */

.js-spoilerbox__body {{
  display: block;
}}

/* ---- usercard ---- */

.js-usercard {{
  font-weight: 600;
}}

.user-name {{
  font-weight: 600;
  color: hsl(var(--hsl-l2));
}}

/* ---- gallery (visible as-is, no JS lightbox) ---- */

.js-gallery {{
  display: block;
}}
</style>
</head>
<body>
{html}
</body>
</html>"#,
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
