use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;

fn cache_dir() -> PathBuf {
    static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
    CACHE_DIR
        .get_or_init(|| {
            let base = dirs_proj()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("osubot")
                .join("resources");
            let _ = std::fs::create_dir_all(&base);
            base
        })
        .clone()
}

fn dirs_proj() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join("Library").join("Caches"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            return Some(PathBuf::from(localappdata));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let base = std::env::var("XDG_CACHE_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(home).join(".cache"));
            return Some(base);
        }
    }
    None
}

fn sha256_hex(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn detect_mime_and_ext(url: &str, bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.len() >= 8 && &bytes[0..4] == b"\x89PNG" {
        ("image/png", ".png")
    } else if bytes.len() >= 3 && &bytes[0..3] == b"\xFF\xD8\xFF" {
        ("image/jpeg", ".jpg")
    } else if bytes.len() >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        ("image/gif", ".gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ("image/webp", ".webp")
    } else if bytes.starts_with(b"<svg")
        || bytes
            .strip_prefix(b"\xEF\xBB\xBF")
            .map_or(false, |b| b.starts_with(b"<svg"))
        || bytes.starts_with(b"<?xml")
    {
        ("image/svg+xml", ".svg")
    } else {
        let ext = url.split('?').next().and_then(|u| {
            let lower = u.to_lowercase();
            if lower.ends_with(".png") {
                Some(".png")
            } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                Some(".jpg")
            } else if lower.ends_with(".gif") {
                Some(".gif")
            } else if lower.ends_with(".webp") {
                Some(".webp")
            } else if lower.ends_with(".svg") {
                Some(".svg")
            } else {
                None
            }
        });
        match ext {
            Some(".png") => ("image/png", ".png"),
            Some(".jpg") => ("image/jpeg", ".jpg"),
            Some(".gif") => ("image/gif", ".gif"),
            Some(".webp") => ("image/webp", ".webp"),
            Some(".svg") => ("image/svg+xml", ".svg"),
            _ => ("application/octet-stream", ".bin"),
        }
    }
}

async fn try_fetch_image(url: &str, client: &reqwest::Client) -> Result<Vec<u8>, ()> {
    use tokio::time::Duration;

    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
        }
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                return resp.bytes().await.map(|b| b.to_vec()).map_err(|_| ());
            }
            Ok(resp) if resp.status().is_server_error() => {
                continue;
            }
            Ok(_) => return Err(()),
            Err(_) => continue,
        }
    }
    Err(())
}

async fn fetch_and_cache(
    url: &str,
    client: &reqwest::Client,
) -> Result<(Vec<u8>, String, String), ()> {
    let hash = sha256_hex(url);
    let cache_file = cache_dir().join(&hash);

    if let Ok(cached) = std::fs::read(&cache_file) {
        let (mime, _) = detect_mime_and_ext(url, &cached);
        return Ok((cached, mime.to_string(), hash));
    }

    let bytes = try_fetch_image(url, client).await?;

    let (mime, _) = detect_mime_and_ext(url, &bytes);
    let cache_file = cache_dir().join(&hash);

    if let Err(e) = std::fs::write(&cache_file, &bytes) {
        eprintln!("[osubot-render] failed to cache {}: {}", url, e);
    }

    Ok((bytes, mime.to_string(), hash))
}

struct ImageRef {
    url: String,
    start: usize,
    end: usize,
}

fn extract_image_refs(html: &str) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    let bytes = html.as_bytes();
    let mut pos = 0;

    while pos < bytes.len() {
        if let Some(rest) = find_next_tag(bytes, pos) {
            pos = rest;
            if !is_img_tag(bytes, pos) {
                continue;
            }
            if let Some((url_start, url_end)) = find_src_url(bytes, pos) {
                let url = std::str::from_utf8(&bytes[url_start..url_end]).unwrap_or("");
                if url.starts_with("https://") || url.starts_with("http://") {
                    refs.push(ImageRef {
                        url: url.to_string(),
                        start: url_start,
                        end: url_end,
                    });
                }
            }
        } else {
            break;
        }
    }

    refs
}

fn find_next_tag(bytes: &[u8], start: usize) -> Option<usize> {
    let tag_start = bytes[start..].iter().position(|&b| b == b'<')?;
    Some(start + tag_start + 1)
}

fn is_img_tag(bytes: &[u8], pos: usize) -> bool {
    let rest = &bytes[pos..];
    if rest.len() < 3 {
        return false;
    }
    if !(rest[0].eq_ignore_ascii_case(&b'i')
        && rest[1].eq_ignore_ascii_case(&b'm')
        && rest[2].eq_ignore_ascii_case(&b'g'))
    {
        return false;
    }
    if rest.len() > 3 && rest[3].is_ascii_alphanumeric() {
        return false;
    }
    true
}

fn find_src_url(bytes: &[u8], tag_start: usize) -> Option<(usize, usize)> {
    let tag_bytes = &bytes[tag_start..];
    let tag_len = tag_bytes
        .iter()
        .position(|&b| b == b'>')
        .unwrap_or(tag_bytes.len());
    let search_window = &tag_bytes[..tag_len.min(4096)];
    let src_pos = search_window
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"src=\""))?;
    let value_start = tag_start + src_pos + 5;
    let quote_pos = bytes[value_start..].iter().position(|&b| b == b'"')?;
    Some((value_start, value_start + quote_pos))
}

fn build_data_uri(bytes: &[u8], mime: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{};base64,{}", mime, b64)
}

pub async fn inline_external_images(html: &str) -> String {
    let mut refs = extract_image_refs(html);
    if refs.is_empty() {
        return html.to_string();
    }

    let unique_urls = {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        let mut urls = Vec::new();
        for r in &refs {
            if seen.insert(r.url.clone()) {
                urls.push(r.url.clone());
            }
        }
        urls
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[osubot-render] failed to build reqwest client: {}", e);
            return html.to_string();
        }
    };

    use std::collections::HashMap;
    let mut url_to_data: HashMap<String, String> = HashMap::new();

    for url in &unique_urls {
        match fetch_and_cache(url, &client).await {
            Ok((bytes, mime, _hash)) => {
                let data_uri = build_data_uri(&bytes, &mime);
                url_to_data.insert(url.clone(), data_uri);
            }
            Err(_) => {}
        }
    }

    if url_to_data.is_empty() {
        return html.to_string();
    }

    refs.sort_by_key(|r| r.start);
    let mut result = html.as_bytes().to_vec();

    for r in refs.iter().rev() {
        if let Some(data_uri) = url_to_data.get(&r.url) {
            result.splice(r.start..r.end, data_uri.bytes());
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| html.to_string())
}

pub fn cleanup_expired(retention_days: u64) {
    if retention_days == 0 {
        return;
    }

    let dir = cache_dir();
    if !dir.exists() {
        return;
    }

    let cutoff =
        SystemTime::now().checked_sub(std::time::Duration::from_secs(retention_days * 86400));

    let cutoff = match cutoff {
        Some(t) => t,
        None => {
            eprintln!("[osubot-render] failed to compute cutoff time for cache cleanup");
            return;
        }
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!(
                "[osubot-render] failed to read cache dir for cleanup: {}",
                e
            );
            return;
        }
    };

    let mut deleted = 0u64;
    let mut errors = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();

        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };

        if modified < cutoff {
            match std::fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => errors += 1,
            }
        }
    }

    eprintln!(
        "[osubot-render] cache cleanup: {} deleted, {} errors (retention: {} days)",
        deleted, errors, retention_days
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hex_deterministic() {
        let a = sha256_hex("https://a.ppy.sh/1234");
        let b = sha256_hex("https://a.ppy.sh/1234");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn test_detect_mime_png() {
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.png", b"\x89PNG\r\n\x1a\n");
        assert_eq!(mime, "image/png");
        assert_eq!(ext, ".png");
    }

    #[test]
    fn test_detect_mime_jpeg() {
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.jpg", b"\xFF\xD8\xFF");
        assert_eq!(mime, "image/jpeg");
        assert_eq!(ext, ".jpg");
    }

    #[test]
    fn test_detect_mime_gif() {
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.gif", b"GIF89a");
        assert_eq!(mime, "image/gif");
        assert_eq!(ext, ".gif");
    }

    #[test]
    fn test_detect_mime_webp() {
        let (mime, ext) =
            detect_mime_and_ext("https://a.ppy.sh/x.webp", b"RIFF\x00\x00\x00\x00WEBP");
        assert_eq!(mime, "image/webp");
        assert_eq!(ext, ".webp");
    }

    #[test]
    fn test_detect_mime_fallback_by_ext() {
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.PNG", b"");
        assert_eq!(mime, "image/png");
        assert_eq!(ext, ".png");
    }

    #[test]
    fn test_detect_mime_unknown() {
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.foo", b"");
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(ext, ".bin");
    }

    #[test]
    fn test_extract_image_refs_single() {
        let html = r#"<img src="https://a.ppy.sh/1.png">"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].url, "https://a.ppy.sh/1.png");
        assert_eq!(&html[refs[0].start..refs[0].end], "https://a.ppy.sh/1.png");
    }

    #[test]
    fn test_extract_image_refs_multiple() {
        let html =
            r#"<div><img src="https://a.ppy.sh/1.png"><img src="https://a.ppy.sh/2.jpg"></div>"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 2);
        let urls: Vec<&str> = refs.iter().map(|r| r.url.as_str()).collect();
        assert!(urls.contains(&"https://a.ppy.sh/1.png"));
        assert!(urls.contains(&"https://a.ppy.sh/2.jpg"));
        for r in &refs {
            assert_eq!(&html[r.start..r.end], r.url);
        }
    }

    #[test]
    fn test_extract_image_refs_preserves_dup_positions() {
        let html = r#"<img src="https://a.ppy.sh/1.png"><img src="https://a.ppy.sh/1.png">"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].url, "https://a.ppy.sh/1.png");
        assert_eq!(refs[1].url, "https://a.ppy.sh/1.png");
        assert_ne!(refs[0].start, refs[1].start);
        assert_eq!(&html[refs[0].start..refs[0].end], "https://a.ppy.sh/1.png");
        assert_eq!(&html[refs[1].start..refs[1].end], "https://a.ppy.sh/1.png");
    }

    #[test]
    fn test_extract_image_refs_no_images() {
        let html = r#"<div>hello</div>"#;
        let refs = extract_image_refs(html);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_image_refs_relative_ignored() {
        let html = r#"<img src="/images/foo.png">"#;
        let refs = extract_image_refs(html);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_image_refs_prefix_urls_separate_positions() {
        let html = r#"<img src="https://a.ppy.sh/1"><img src="https://a.ppy.sh/10">"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].url, "https://a.ppy.sh/1");
        assert_eq!(refs[1].url, "https://a.ppy.sh/10");
        assert_eq!(&html[refs[0].start..refs[0].end], "https://a.ppy.sh/1");
        assert_eq!(&html[refs[1].start..refs[1].end], "https://a.ppy.sh/10");
        assert_ne!(refs[0].start, refs[1].start);
        assert_ne!(refs[0].end, refs[1].end);
    }

    #[test]
    fn test_extract_image_refs_ignores_non_img_tags() {
        let html = r#"<source src="https://a.ppy.sh/1.webp"><script src="https://a.ppy.sh/2.js"><img src="https://a.ppy.sh/3.png">"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].url, "https://a.ppy.sh/3.png");
    }

    #[test]
    fn test_extract_image_refs_matches_img_case_variants() {
        let html = r#"<IMG src="https://a.ppy.sh/a.png"><Img src="https://a.ppy.sh/b.png"><img src="https://a.ppy.sh/c.png">"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_build_data_uri() {
        let uri = build_data_uri(b"\x89PNG\r\n\x1a\nhello", "image/png");
        assert!(uri.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_inline_external_images_no_images() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = "<div>no images here</div>";
        let result = rt.block_on(inline_external_images(html));
        assert_eq!(result, html);
    }

    #[test]
    fn test_cleanup_expired_noop_when_disabled() {
        cleanup_expired(0);
    }
}
