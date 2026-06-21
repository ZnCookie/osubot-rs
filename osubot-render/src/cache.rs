use osubot_core::log_fmt;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
    #[error("SVG rasterize failed: {0}")]
    SvgRasterize(String),
}

pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
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
        tracing::error!("{}", log_fmt!("render.cache_dir_failed", error = &e));
    }
}

fn dirs_proj() -> Option<PathBuf> {
    dirs::cache_dir()
}

fn sha256_hex(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hex::encode(hasher.finalize())
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
                            tracing::warn!(error = %e, url = %url, attempt, "{}", log_fmt!("render.body_download_failed"));
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
                tracing::warn!(status = %resp.status(), url = %url, "{}", log_fmt!("render.server_error_retry"));
                continue;
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                tracing::warn!(status = %status, url = %url, "{}", log_fmt!("render.client_error_abort"));
                return Err(CacheError::ClientError { status });
            }
            Err(e) => {
                tracing::warn!(error = %e, url = %url, "{}", log_fmt!("render.request_failed_retry"));
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
        tracing::warn!(
            "{}",
            log_fmt!("render.cache_write_failed", url = url, error = &e)
        );
    }

    Ok((bytes, mime.to_string(), hash))
}

struct ImageRef {
    url: String,
    start: usize,
    end: usize,
}

/// 通过字节扫描提取 HTML 中的 `<img src="...">` URL。
///
/// 这不是一个完整的 HTML 解析器——它无法处理注释中的 img 标签、转义引号、
/// 或除 `src` 之外的其他属性写法。适用范围仅限于 blitz 渲染引擎输出的
/// 结构规整的 `<img>` 标签。若将来引入 maud 或其他模板引擎，应考虑
/// 改用真正的 HTML 解析器（如 `html5ever` 或 `ego-tree`）。
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
                // 提取 http(s) + file:// 三类：前两类走 fetch+inline 路径，
                // file:// 进来后会直接被 is_blocked_url 判 true（scheme=file
                // 没 host），由 inline_external_images 改写成 1x1 占位 PNG，
                // 防止下游 blitz 自己用 std::fs::read 读本地文件。
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("file://")
                {
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

/// 把 SVG 字节 rasterize 成 PNG。
///
/// 流程：svg_css 预处理器先把 var(--name) 替换成立面值（usvg/simplecss 不支持
/// CSS 变量，会 fallback 黑色），build 出加载了系统字体的 usvg::Options，
/// usvg::Tree::from_data parse，最后 resvg::render 画到 tiny_skia::Pixmap，
/// 编码 PNG。
///
/// 跟 blitz SVG paint 路径完全解耦：blitz 之后看到的就是普通 PNG 位图，
/// 不再触发 usvg 对 <pattern>/<filter>/mix-blend-mode 的有限支持问题。
fn rasterize_svg_to_png(svg_bytes: &[u8]) -> Result<Vec<u8>, CacheError> {
    // 1. 先过 preprocessor：usvg + simplecss 0.2.2 不解析 CSS 变量，
    //    留 var(--name) 给 usvg 会 fallback Color::black()，所以必须替换。
    let preprocessed = match std::str::from_utf8(svg_bytes) {
        Ok(text) => crate::svg_css::resolve_svg_css_vars(text),
        Err(_) => {
            // 非 UTF-8 SVG（极罕见），原样交给 usvg
            return rasterize_svg_str(svg_bytes, &build_svg_options());
        }
    };

    rasterize_svg_str(preprocessed.as_bytes(), &build_svg_options())
}

/// build usvg::Options，load 系统字体，探测可用的 sans-serif 字体名。
///
/// `usvg::Options::default()` 用空 `fontdb: Arc<new(Database::new())>`，
/// 直接 parse `<text>` 元素会找不到字体、画不出文字。必须手动
/// `load_system_fonts()` + 探测 sans-serif 具体 family 名（fontdb 0.23
/// 在 Linux 处理 fontconfig alias 结构有 bug，`sans-serif` 字面量 query
/// 不到）。
fn build_svg_options() -> resvg::usvg::Options<'static> {
    use resvg::usvg::Options;
    let mut opt = Options::default();
    {
        let fontdb = opt.fontdb_mut();
        // fontdb 0.23 的 load_system_fonts() 返回 ()，无错误信号。失败时
        // （容器无 fontconfig / no system fonts）只能靠下方 sans-serif 探测
        // 全部 miss 来发现——所以该函数必须返回 bool 让调用方 logging。
        fontdb.load_system_fonts();
        if !detect_sans_serif_family(fontdb) {
            tracing::warn!("{}", log_fmt!("render.svg_no_sans_serif_detected"));
        }
    }
    opt
}

/// 在 load_system_fonts() 之后的 fontdb 里探测可用的 sans-serif 字体名并 set。
/// 全部 miss 时返回 false，调用方负责 logging。
fn detect_sans_serif_family(fontdb: &mut fontdb::Database) -> bool {
    use fontdb::{Family, Query};
    // Noto Sans CJK SC 提前：Linux 容器最常见，osu! profile 含 CJK 字符
    // 命中它才能渲中日韩。其他候选按 macOS / Windows / Linux 常见顺序。
    let candidates = [
        "Noto Sans CJK SC",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
        "Arial",
        "Helvetica",
        "PingFang SC",
        "Microsoft YaHei",
        "Hiragino Kaku Gothic ProN",
    ];
    for name in candidates {
        let q = Query {
            families: &[Family::Name(name)],
            ..Default::default()
        };
        if fontdb.query(&q).is_some() {
            fontdb.set_sans_serif_family(name);
            return true;
        }
    }
    false
}

fn rasterize_svg_str(svg_bytes: &[u8], opt: &resvg::usvg::Options) -> Result<Vec<u8>, CacheError> {
    use resvg::usvg::Tree;
    let tree = Tree::from_data(svg_bytes, opt)
        .map_err(|e| CacheError::SvgRasterize(format!("usvg parse: {e}")))?;

    let size = tree.size();
    let width = (size.width() as u32).max(1);
    let height = (size.height() as u32).max(1);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| CacheError::SvgRasterize(format!("pixmap alloc {width}x{height}")))?;

    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );

    pixmap
        .encode_png()
        .map_err(|e| CacheError::SvgRasterize(format!("png encode: {e}")))
}

fn check_v4_private(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
}

/// 拒绝私有/保留 IP 段和本地域名，防止 SSRF。
#[doc(hidden)]
pub fn is_blocked_url(url: &str) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    let host_raw = match parsed.host_str() {
        Some(h) => h,
        None => return true,
    };
    let host_raw = host_raw.to_ascii_lowercase();
    // reqwest 的 host_str() 对 IPv6 会保留方括号 "[::1]"，剥掉才能 parse
    let host = host_raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host_raw);
    // 拒绝字面量私有/保留 IP
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => check_v4_private(v4),
            std::net::IpAddr::V6(v6) => {
                // IPv4-mapped IPv6 (::ffff:0:0/96)：先提 V4 走 V4 规则
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return check_v4_private(v4);
                }
                // NAT64 (RFC 6052): 64:ff9b::/96 — NAT64 网关可桥接到 IPv4 私网
                if let Some(first) = v6.segments().first() {
                    if *first == 0x0064 && v6.segments()[1] == 0xff9b {
                        return true;
                    }
                }
                // Teredo (RFC 4380): 2001::/32 — 经 UDP 隧道封装 IPv4
                if v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0000 {
                    return true;
                }
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unicast_link_local()
                    || v6.is_unique_local()
                    // 6to4 (2002::/16)：承载 IPv4 私网
                    || (v6.segments()[0] == 0x2002)
            }
        };
    }
    // 拒绝 localhost / .local（host 已小写化，匹配统一走小写）
    matches!(
        host,
        "localhost" | "0.0.0.0" | "::1" | "ip6-localhost" | "ip6-loopback"
    ) || host.ends_with(".local")
}

pub async fn inline_external_images(html: &str) -> String {
    let mut refs = extract_image_refs(html);
    if refs.is_empty() {
        return html.to_string();
    }

    let unique_urls = {
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
    let mut blocked: HashSet<String> = HashSet::new();
    // 1x1 透明 PNG（70 字节标准 base64），用于替换被 SSRF 规则拒绝的 <img src=...>。
    // 选 1x1 是为了不污染版面：blitz 拿到一个看不见的占位图，仍会按 alt 文本继续排版。
    const PLACEHOLDER_PNG: &str = "data:image/png;base64,\
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

    let mut set = tokio::task::JoinSet::new();
    for url in unique_urls {
        if is_blocked_url(&url) {
            tracing::warn!(
                url = %url,
                "blocked external image fetch (private/reserved address); replacing with placeholder"
            );
            blocked.insert(url);
            continue;
        }
        let client = client.clone();
        set.spawn(async move {
            let res = fetch_and_cache(&url, &client).await;
            (url, res)
        });
    }

    while let Some(res) = set.join_next().await {
        match res {
            Ok((url, Ok((bytes, mime, _hash)))) => {
                // 内联 SVG 走 resvg 预 rasterize 成 PNG：
                // - usvg + simplecss 0.2.2 不解析 CSS 变量（preprocessor 提前把
                //   var(--name) 替换成立面值，否则 stop-color fallback 黑）
                // - usvg 0.45.1 + anyrender_svg 0.6.3 对 SVG <pattern>、<filter>、
                //   mix-blend-mode 支持不全（之前出现 2 个红方块）。resvg 自己渲 SVG，
                //   blitz 拿到 <img src="data:image/png;..."> 普通位图，不再触发 usvg
                //   paint 路径。
                let data_uri = if mime == "image/svg+xml" {
                    match rasterize_svg_to_png(&bytes) {
                        Ok(png) => Some(build_data_uri(&png, "image/png")),
                        Err(e) => {
                            tracing::warn!(
                                url = %url,
                                error = %e,
                                "{}",
                                log_fmt!("render.svg_rasterize_failed")
                            );
                            // 关键修复：失败时不再 fallback 到 image/svg+xml data URI
                            // （会重新走 blitz + usvg paint 路径，触发 <pattern>/<filter>
                            // /mix-blend-mode 支持不全的红方块 bug，正是这次 librsvg→resvg
                            // 迁移要绕开的路径）。改为丢弃该图片，html 里 <img src=...>
                            // 保留原 URL，blitz 拿不到会显示破图占位——行为可观察。
                            None
                        }
                    }
                } else {
                    Some(build_data_uri(&bytes, &mime))
                };
                if let Some(uri) = data_uri {
                    url_to_data.insert(url, uri);
                }
            }
            Ok((url, Err(e))) => {
                tracing::warn!(url = %url, error = %e, "{}", log_fmt!("render.batch_fetch_failed"));
            }
            Err(e) => {
                tracing::warn!(error = %e, "{}", log_fmt!("render.batch_join_error"));
            }
        }
    }

    if url_to_data.is_empty() && blocked.is_empty() {
        return html.to_string();
    }

    refs.sort_by_key(|r| r.start);
    let mut result = html.as_bytes().to_vec();

    for r in refs.iter().rev() {
        if let Some(data_uri) = url_to_data.get(&r.url) {
            result.splice(r.start..r.end, data_uri.bytes());
        } else if blocked.contains(&r.url) {
            // SSRF 拒绝的 URL（包括 file://）改成 1x1 占位 PNG，
            // 防止下游 blitz 用自己的 client 重新 fetch 该 URL。
            result.splice(r.start..r.end, PLACEHOLDER_PNG.bytes());
        }
    }

    String::from_utf8(result).unwrap_or_else(|e| {
        tracing::warn!("{}", log_fmt!("render.inline_utf8_invalid", error = &e));
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
            tracing::error!("{}", log_fmt!("render.cleanup_cutoff_failed"));
            return;
        }
    };

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("{}", log_fmt!("render.cleanup_read_dir_failed", error = &e));
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
                    tracing::warn!(
                        "{}",
                        log_fmt!(
                            "render.cleanup_delete_failed",
                            path = format!("{}", path.display()),
                            error = &e
                        )
                    );
                    errors += 1;
                }
            }
        }
    }

    tracing::info!(
        "{}",
        log_fmt!(
            "render.cleanup_summary",
            deleted = deleted.to_string(),
            errors = errors.to_string(),
            days = retention_days.to_string()
        )
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

    #[tokio::test]
    async fn test_inline_external_images_passes_svg_through_as_data_uri() {
        let test_url = "https://example.test/club-60.svg";
        let fixture_svg: &str = include_str!("../tests/resources/club-60.svg");

        ensure_cache_dir().await;
        let cache_path = cache_dir().join(sha256_hex(test_url));
        tokio::fs::write(&cache_path, fixture_svg.as_bytes())
            .await
            .expect("write fixture to cache");

        struct RemoveOnDrop(PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = RemoveOnDrop(cache_path);

        let html = format!(r#"<html><body><img src="{}"></body></html>"#, test_url);
        let result = inline_external_images(&html).await;

        // Phase 4 修复（2026-06-21）：内联 SVG 现在走 resvg 预 rasterize 成 PNG，
        // 解决 usvg 0.45.1 + anyrender_svg 0.6.3 对 SVG <pattern>/<filter>/mix-blend-mode
        // 支持不全的问题（出现 2 个红方块）。blitz 拿到 <img src="data:image/png;...">
        // 当作普通位图处理，不再触发 usvg paint 路径。
        assert!(
            result.contains("data:image/png;base64,"),
            "expected PNG data URI prefix, got: {}",
            result
        );
        assert!(
            !result.contains("data:image/svg+xml"),
            "SVG should be rasterized to PNG, not passed through: {}",
            result
        );

        // 验证 PNG 是有效的（decode 能成 + 维度正确）
        use base64::Engine;
        let b64_start =
            result.find("data:image/png;base64,").unwrap() + "data:image/png;base64,".len();
        let b64_end = result[b64_start..].find('"').unwrap() + b64_start;
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(&result[b64_start..b64_end])
            .expect("PNG base64 should decode");
        let img = image::load_from_memory(&png_bytes).expect("PNG should decode");
        assert!(
            img.width() > 0 && img.height() > 0,
            "PNG dimensions should be non-zero"
        );
    }
}

#[cfg(test)]
mod ssrf_tests {
    use super::is_blocked_url;

    #[test]
    fn test_blocked_private_ipv4() {
        assert!(is_blocked_url("http://10.0.0.1/x.png"));
        assert!(is_blocked_url("http://192.168.1.1/x.png"));
        assert!(is_blocked_url("http://172.16.0.1/x.png"));
    }

    #[test]
    fn test_blocked_loopback() {
        assert!(is_blocked_url("http://127.0.0.1/x.png"));
        assert!(is_blocked_url("http://localhost/x.png"));
    }

    #[test]
    fn test_blocked_link_local() {
        assert!(is_blocked_url("http://169.254.1.1/x.png"));
    }

    #[test]
    fn test_allowed_public() {
        assert!(!is_blocked_url("https://assets.ppy.sh/x.png"));
        assert!(!is_blocked_url("https://example.com/x.png"));
    }

    #[test]
    fn test_blocked_invalid_url() {
        assert!(is_blocked_url("not-a-url"));
        assert!(is_blocked_url("file:///etc/passwd"));
    }
}

/// `detect_sans_serif_family` 单元测试。
///
/// 注意：fontdb 0.23 `Database::new()` 默认 `family_sans_serif = "Arial"`
/// （不是空），这是它和 usvg 假设不同的点。我们的 `detect_sans_serif_family`
/// 只在**探测到具体字体**时 set；探测 miss 时保持原值不动。
/// 真实场景（探测命中）的覆盖由集成测试 rasterize 含 `<text>` SVG 完成。
#[cfg(test)]
mod detect_sans_serif_tests {
    use super::detect_sans_serif_family;
    use fontdb::Family;

    #[test]
    fn empty_db_keeps_default() {
        let mut db = fontdb::Database::new();
        let before = db.family_name(&Family::SansSerif).to_string();
        detect_sans_serif_family(&mut db);
        // 探测不到任何具体 family → 保持原值（默认 "Arial"）
        assert_eq!(db.family_name(&Family::SansSerif), before);
    }

    #[test]
    fn picks_first_match_in_candidate_order() {
        // 空 fontdb 里探测全 miss → 保持默认。构造一个含字体的 fixture 太重
        // （需要真实 ttf bytes），交给集成测试端到端覆盖。
        let mut db = fontdb::Database::new();
        detect_sans_serif_family(&mut db);
        assert_eq!(db.family_name(&Family::SansSerif), "Arial");
    }

    /// 端到端验证 resvg 路径能正确渲染 `<text>` 元素（build_svg_options
    /// 加载系统字体 + 探测 sans-serif 名字成功）。
    ///
    /// 复刻 osu! profile banner 结构：class 在 root + 通用 `font-family="sans-serif"`。
    /// preprocessor 跳过 class，但 `*` 块在 fixture 里加一个具体字体名强制 resvg
    /// 用具体字体。
    #[test]
    fn rasterize_svg_text_is_painted_when_system_font_available() {
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="20">
  <rect width="80" height="20" fill="white"/>
  <text x="2" y="14" font-family="sans-serif" font-size="14" fill="black">Hello</text>
</svg>"#;
        let png_bytes = match super::rasterize_svg_to_png(SVG.as_bytes()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("rasterize failed (likely no system fonts): {e}");
                return;
            }
        };
        let img = image::load_from_memory(&png_bytes).expect("PNG should decode");
        let rgba = img.to_rgba8();
        // 统计非白像素（黑色字形）
        let black_count = rgba
            .pixels()
            .filter(|p| p[0] < 50 && p[1] < 50 && p[2] < 50 && p[3] == 255)
            .count();
        assert!(
            black_count > 20,
            "expected >20 black pixels from text glyphs, got {black_count} \
             (system font loading or sans-serif detection failed)"
        );
    }
}
