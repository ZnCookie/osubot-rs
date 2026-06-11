use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;
use tokio::sync::{Mutex, Semaphore};

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_CONCURRENT_FETCHES: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("HTTP client error: status {status}")]
    ClientError { status: u16 },
    #[error("image exceeds maximum size")]
    TooLarge,
    #[error("retries exhausted")]
    RetriesExhausted,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SVG rasterization failed: {0}")]
    SvgRasterizationFailed(String),
}

pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client")
    })
}

// Per-URL mutex to prevent thundering-herd cache writes.
// The lock is removed from the map immediately after the fetch completes,
// so the map only holds entries for URLs currently being fetched.
fn fetch_locks() -> &'static std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

static FETCH_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn fetch_semaphore() -> &'static Semaphore {
    FETCH_SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_FETCHES))
}

fn cache_dir() -> PathBuf {
    static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
    CACHE_DIR
        .get_or_init(|| {
            dirs_proj()
                .unwrap_or_else(std::env::temp_dir)
                .join("osubot")
                .join("resources")
        })
        .clone()
}

pub async fn ensure_cache_dir() {
    if let Err(e) = tokio::fs::create_dir_all(cache_dir()).await {
        tracing::error!("failed to create cache directory: {}", e);
    }
}

fn dirs_proj() -> Option<PathBuf> {
    dirs::cache_dir()
}

fn sha256_hex(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
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
    } else if (bytes.len() >= 4 && bytes.starts_with(b"<svg"))
        || bytes
            .strip_prefix(b"\xEF\xBB\xBF")
            .is_some_and(|b| b.starts_with(b"<svg"))
    {
        ("image/svg+xml", ".svg")
    } else if bytes.starts_with(b"<?xml") {
        let search_window = &bytes[..bytes.len().min(512)];
        if search_window.windows(4).any(|w| w == b"<svg") {
            ("image/svg+xml", ".svg")
        } else {
            detect_mime_and_ext_fallback(url)
        }
    } else {
        detect_mime_and_ext_fallback(url)
    }
}

fn detect_mime_and_ext_fallback(url: &str) -> (&'static str, &'static str) {
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

/// Rasterize SVG bytes to PNG using librsvg.
/// Returns PNG-encoded bytes on success.
fn rasterize_svg_to_png(svg_bytes: &[u8]) -> Result<Vec<u8>, CacheError> {
    // Load SVG using rsvg::Loader with a MemoryInputStream
    let memory_stream =
        gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg_bytes.to_vec()));
    let handle = rsvg::Loader::new()
        .read_stream::<gio::MemoryInputStream, gio::File, gio::Cancellable>(
            &memory_stream,
            None,
            None,
        )
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    // Get intrinsic dimensions
    let dims = rsvg::CairoRenderer::new(&handle)
        .intrinsic_size_in_pixels()
        .unwrap_or((100.0, 100.0));
    let width = dims.0.ceil() as i32;
    let height = dims.1.ceil() as i32;

    const MAX_SVG_DIMENSION: i32 = 4096;
    if width <= 0 || height <= 0 || width > MAX_SVG_DIMENSION || height > MAX_SVG_DIMENSION {
        return Err(CacheError::SvgRasterizationFailed(format!(
            "SVG dimensions {}x{} exceed maximum {}x{}",
            width, height, MAX_SVG_DIMENSION, MAX_SVG_DIMENSION
        )));
    }

    // Create Cairo surface
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;
    let cr = cairo::Context::new(&surface)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    // Render SVG
    rsvg::CairoRenderer::new(&handle)
        .render_document(
            &cr,
            &cairo::Rectangle::new(0.0, 0.0, f64::from(width), f64::from(height)),
        )
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    // Encode as PNG
    let mut png_bytes = Vec::new();
    surface
        .write_to_png(&mut png_bytes)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;
    Ok(png_bytes)
}

/// Rasterize SVG to PNG with caching.
/// Cache key is SHA256 of SVG bytes. Same SVG always maps to same PNG.
/// Uses per-URL locks + double-check to avoid concurrent rasterization of the same SVG.
/// Writes via temp file + atomic rename to avoid partial cache files.
async fn rasterize_svg_to_png_cached(svg_bytes: &[u8], url: &str) -> Result<Vec<u8>, CacheError> {
    let cache_key = sha256_hex_bytes(svg_bytes);
    let cache_file = cache_dir().join(format!("{}.png", cache_key));

    // Fast path: check cache without lock
    if let Ok(cached) = tokio::fs::read(&cache_file).await {
        return Ok(cached);
    }

    // Slow path: acquire per-URL lock
    let url_lock = {
        let mut locks = fetch_locks().lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(url.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _fetch_guard = FetchLockGuard {
        url: url.to_string(),
    };
    let _guard = url_lock.lock().await;

    // Double-check
    if let Ok(cached) = tokio::fs::read(&cache_file).await {
        return Ok(cached);
    }

    // Rasterize in blocking task
    let svg_bytes = svg_bytes.to_vec();
    let png_bytes = tokio::task::spawn_blocking(move || rasterize_svg_to_png(&svg_bytes))
        .await
        .map_err(|e| {
            CacheError::SvgRasterizationFailed(format!("spawn_blocking failed: {}", e))
        })??;

    // Write to temp file
    let temp_file = cache_dir().join(format!("{}.tmp", cache_key));
    tokio::fs::write(&temp_file, &png_bytes)
        .await
        .map_err(|e| {
            CacheError::SvgRasterizationFailed(format!("failed to write temp file: {}", e))
        })?;

    // Atomic rename
    match tokio::fs::rename(&temp_file, &cache_file).await {
        Ok(()) => {}
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_file).await;
            return Err(CacheError::Io(e));
        }
    }

    Ok(png_bytes)
}

async fn try_fetch_image(url: &str, client: &reqwest::Client) -> Result<Vec<u8>, CacheError> {
    use tokio::time::Duration;

    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
        }
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Some(len) = resp.content_length() {
                    if len > MAX_IMAGE_BYTES {
                        return Err(CacheError::TooLarge);
                    }
                }
                let mut bytes = Vec::new();
                let mut stream = resp.bytes_stream();
                use futures_util::StreamExt;
                let mut body_failed = false;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => {
                            if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES as usize {
                                return Err(CacheError::TooLarge);
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, url = %url, attempt, "body download failed, will retry");
                            body_failed = true;
                            break;
                        }
                    }
                }
                if body_failed {
                    continue;
                }
                return Ok(bytes);
            }
            Ok(resp) if resp.status().is_server_error() => {
                tracing::warn!(status = %resp.status(), url = %url, "server error, will retry");
                continue;
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                tracing::warn!(status = %status, url = %url, "client error, aborting");
                return Err(CacheError::ClientError { status });
            }
            Err(e) => {
                tracing::warn!(error = %e, url = %url, "request failed, will retry");
                continue;
            }
        }
    }
    Err(CacheError::RetriesExhausted)
}

struct FetchLockGuard {
    url: String,
}

impl Drop for FetchLockGuard {
    fn drop(&mut self) {
        let mut locks = fetch_locks().lock().unwrap_or_else(|e| e.into_inner());
        locks.remove(&self.url);
    }
}

pub async fn fetch_and_cache(
    url: &str,
    client: &reqwest::Client,
) -> Result<(Vec<u8>, String, String), CacheError> {
    let hash = sha256_hex(url);
    let cache_file = cache_dir().join(&hash);

    if let Ok(cached) = tokio::fs::read(&cache_file).await {
        let (mime, _) = detect_mime_and_ext(url, &cached);
        return Ok((cached, mime.to_string(), hash));
    }

    let url_lock = {
        let mut locks = fetch_locks().lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(url.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };

    let _fetch_guard = FetchLockGuard {
        url: url.to_string(),
    };
    let _guard = url_lock.lock().await;

    if let Ok(cached) = tokio::fs::read(&cache_file).await {
        let (mime, _) = detect_mime_and_ext(url, &cached);
        return Ok((cached, mime.to_string(), hash));
    }

    let _fetch_permit = fetch_semaphore()
        .acquire()
        .await
        .expect("fetch semaphore never closed");

    let bytes = match try_fetch_image(url, client).await {
        Ok(b) => b,
        Err(e) => return Err(e),
    };

    let (mime, _) = detect_mime_and_ext(url, &bytes);

    if let Err(e) = tokio::fs::write(&cache_file, &bytes).await {
        tracing::warn!("failed to cache {}: {}", url, e);
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

    // Try double quote first
    if let Some(src_pos) = search_window
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"src=\""))
    {
        let value_start = tag_start + src_pos + 5;
        let quote_pos = bytes[value_start..].iter().position(|&b| b == b'"')?;
        return Some((value_start, value_start + quote_pos));
    }

    // Try single quote
    if let Some(src_pos) = search_window
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"src='"))
    {
        let value_start = tag_start + src_pos + 5;
        let quote_pos = bytes[value_start..].iter().position(|&b| b == b'\'')?;
        return Some((value_start, value_start + quote_pos));
    }

    None
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

    let client = (*http_client()).clone();

    let mut url_to_data: HashMap<String, String> = HashMap::new();

    let mut set = tokio::task::JoinSet::new();
    for url in unique_urls {
        let client = client.clone();
        set.spawn(async move {
            let res = fetch_and_cache(&url, &client).await;
            (url, res)
        });
    }

    while let Some(res) = set.join_next().await {
        match res {
            Ok((url, Ok((bytes, mime, _hash)))) => {
                let (final_bytes, final_mime) = if mime == "image/svg+xml" {
                    match rasterize_svg_to_png_cached(&bytes, &url).await {
                        Ok(png_bytes) => (png_bytes, "image/png".to_string()),
                        Err(e) => {
                            tracing::warn!(url = %url, error = %e, "SVG rasterization failed, falling back to original SVG");
                            (bytes, mime)
                        }
                    }
                } else {
                    (bytes, mime)
                };
                let data_uri = build_data_uri(&final_bytes, &final_mime);
                url_to_data.insert(url, data_uri);
            }
            Ok((url, Err(e))) => {
                tracing::warn!(url = %url, error = %e, "failed to fetch image in parallel batch");
            }
            Err(e) => {
                tracing::warn!(error = %e, "join error in parallel image fetch");
            }
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

    String::from_utf8(result).unwrap_or_else(|e| {
        tracing::warn!("image inlining produced invalid UTF-8: {}", e);
        html.to_string()
    })
}

pub async fn cleanup_expired(retention_days: u64) {
    if retention_days == 0 {
        return;
    }

    let dir = cache_dir();
    match tokio::fs::try_exists(&dir).await {
        Ok(false) | Err(_) => return,
        Ok(true) => {}
    }

    let cutoff =
        SystemTime::now().checked_sub(std::time::Duration::from_secs(retention_days * 86400));

    let cutoff = match cutoff {
        Some(t) => t,
        None => {
            tracing::error!("failed to compute cutoff time for cache cleanup");
            return;
        }
    };

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("failed to read cache dir for cleanup: {}", e);
            return;
        }
    };

    let mut deleted = 0u64;
    let mut errors = 0u64;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();

        let modified = match entry.metadata().await.and_then(|m| m.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };

        if modified < cutoff {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!("failed to delete cache file {}: {e}", path.display());
                    errors += 1;
                }
            }
        }
    }

    tracing::info!(
        "cache cleanup: {} deleted, {} errors (retention: {} days)",
        deleted,
        errors,
        retention_days
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
    fn test_sha256_hex_bytes_deterministic() {
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let hash1 = sha256_hex_bytes(bytes);
        let hash2 = sha256_hex_bytes(bytes);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_sha256_hex_bytes_different_for_different_content() {
        let bytes1 = b"<svg></svg>";
        let bytes2 = b"<svg viewBox=\"0 0 100 100\"></svg>";
        let hash1 = sha256_hex_bytes(bytes1);
        let hash2 = sha256_hex_bytes(bytes2);
        assert_ne!(hash1, hash2);
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

    #[tokio::test]
    async fn test_cleanup_expired_noop_when_disabled() {
        cleanup_expired(0).await;
    }

    #[test]
    fn test_extract_image_refs_single_quote() {
        let html = r#"<img src='https://a.ppy.sh/1.png'>"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].url, "https://a.ppy.sh/1.png");
    }

    #[test]
    fn test_detect_mime_svg_xml_decl() {
        let svg =
            b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"><circle/></svg>";
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x", svg);
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(ext, ".svg");
    }

    #[test]
    fn test_detect_mime_svg_bom() {
        let svg = b"\xEF\xBB\xBF<svg><circle/></svg>";
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x", svg);
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(ext, ".svg");
    }

    #[test]
    fn test_detect_mime_xml_not_svg() {
        let xml = b"<?xml version=\"1.0\"?>\n<html><body>not svg</body></html>";
        let (mime, ext) = detect_mime_and_ext("https://a.ppy.sh/x.foo", xml);
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(ext, ".bin");
    }

    #[test]
    fn test_extract_image_refs_mixed_quotes() {
        let html = r#"<img src="https://a.ppy.sh/1.png"><img src='https://a.ppy.sh/2.jpg'>"#;
        let refs = extract_image_refs(html);
        assert_eq!(refs.len(), 2);
        let urls: Vec<&str> = refs.iter().map(|r| r.url.as_str()).collect();
        assert!(urls.contains(&"https://a.ppy.sh/1.png"));
        assert!(urls.contains(&"https://a.ppy.sh/2.jpg"));
    }

    #[test]
    fn test_fetch_locks_sync_drop_no_panic() {
        let mut locks = fetch_locks().lock().unwrap_or_else(|e| e.into_inner());
        locks.insert("test-url".to_string(), Arc::new(Mutex::new(())));
        drop(locks);
        let guard = FetchLockGuard {
            url: "test-url".to_string(),
        };
        drop(guard);
        let locks = fetch_locks().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!locks.contains_key("test-url"));
    }

    #[test]
    fn test_fetch_locks_poisoned_mutex_no_panic() {
        let poisoned = std::thread::spawn(|| {
            let _lock = fetch_locks().lock().unwrap();
            panic!("intentional poison");
        });
        assert!(poisoned.join().is_err());
        let guard = FetchLockGuard {
            url: "poison-url".to_string(),
        };
        drop(guard);
    }

    #[test]
    fn test_rasterize_svg_to_png_valid_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/></svg>"#;
        let result = rasterize_svg_to_png(svg);
        assert!(result.is_ok());
        let png = result.unwrap();
        assert!(!png.is_empty());
        // PNG magic bytes
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn test_rasterize_svg_to_png_malformed_svg() {
        let svg = b"not an svg at all";
        let result = rasterize_svg_to_png(svg);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CacheError::SvgRasterizationFailed(_)));
    }
}
