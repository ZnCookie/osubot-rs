use scraper::ElementRef;
use scraper::Html;
use takumi::layout::node::{ImageData, Node};

use crate::error::RenderError;

pub fn html_to_node(html: &str) -> Result<Node, RenderError> {
    let document = Html::parse_fragment(html);

    let children: Vec<Node> = convert_children(&document.root_element());

    if children.is_empty() {
        return Err(RenderError::Convert("empty HTML after conversion".into()));
    }

    Ok(Node::container(children))
}

fn convert_children(element: &ElementRef) -> Vec<Node> {
    element
        .children()
        .filter_map(|child| {
            ElementRef::wrap(child)
                .map(|el| convert_element(el))
                .or_else(|| {
                    child.value().as_text().and_then(|t| {
                        let s = t.trim().to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(Node::text(s))
                        }
                    })
                })
        })
        .collect()
}

fn convert_element(el: ElementRef) -> Node {
    let tag = el.value().name().to_lowercase();
    let class = el.value().attr("class").unwrap_or("").to_string();
    let id = el.value().attr("id").map(|s| s.to_string());

    if tag == "img" {
        let src = el.value().attr("src").unwrap_or("").to_string();
        let mut node = Node::image(ImageData::from(src.as_str()));
        if !class.is_empty() {
            node = node.with_class_name(class);
        }
        if let Some(rid) = id {
            node = node.with_id(rid);
        }
        return node;
    }

    let children: Vec<Node> = if class
        .split_whitespace()
        .any(|c| c == "bbcode-spoilerbox__link")
    {
        std::iter::once(Node::text("↴ "))
            .chain(convert_children(&el))
            .collect()
    } else {
        convert_children(&el)
    };

    let mut node = Node::container(children).with_tag_name(tag);
    if !class.is_empty() {
        node = node.with_class_name(class);
    }
    if let Some(rid) = id {
        node = node.with_id(rid);
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple_text() {
        let node = html_to_node("<div class=\"bbcode\">Hello World</div>").unwrap();
        let dbg = format!("{:?}", node);
        assert!(dbg.contains("Hello"), "should contain text: {}", dbg);
    }

    #[test]
    fn test_convert_image_with_data_uri() {
        let html = r#"<div class="bbcode"><img class="badge" src="data:image/png;base64,iVBORw0KGgo="></div>"#;
        let node = html_to_node(html).unwrap();
        let dbg = format!("{:?}", node);
        assert!(
            dbg.contains("data:image"),
            "should contain image src: {}",
            dbg
        );
    }

    #[test]
    fn test_convert_nested_containers() {
        let html = r#"<div class="bbcode"><p>Hello <strong>World</strong></p></div>"#;
        let node = html_to_node(html).unwrap();
        let dbg = format!("{:?}", node);
        assert!(dbg.contains("Hello"), "should contain text: {}", dbg);
    }

    #[test]
    fn test_convert_empty_fragment_returns_err() {
        let result = html_to_node("");
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_preserves_tag_names() {
        let html = r#"<div class="bbcode"><h2>Title</h2><p>Text</p></div>"#;
        let node = html_to_node(html).unwrap();
        let dbg = format!("{:?}", node);
        assert!(dbg.contains("h2"), "should contain h2 tag: {}", dbg);
        assert!(dbg.contains("p"), "should contain p tag: {}", dbg);
    }

    #[test]
    fn test_convert_preserves_class_names() {
        let html = r#"<div class="bbcode"><div class="spoiler">hidden</div></div>"#;
        let node = html_to_node(html).unwrap();
        let dbg = format!("{:?}", node);
        assert!(
            dbg.contains("spoiler"),
            "should contain spoiler class: {}",
            dbg
        );
    }

    #[test]
    fn test_convert_spoilerbox_link_has_arrow() {
        let html = r#"<a class="bbcode-spoilerbox__link">Click</a>"#;
        let node = html_to_node(html).unwrap();
        let dbg = format!("{:?}", node);
        assert!(dbg.contains("\u{21B4}"), "should contain arrow: {}", dbg);
    }
}
