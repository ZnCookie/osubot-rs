use crate::cache::CacheError;
use tracing::warn;
use osubot_core::types::ScoreCard;

pub struct ScoreCardData {
    pub title: String,
    pub artist: String,
    pub mapper: String,
    pub bid: u64,
    pub difficulty_name: String,
    pub stars: f64,
    pub cs: f64,
    pub ar: f64,
    pub od: f64,
    pub hp: f64,
    pub bpm: u32,
    pub length_secs: u32,
    pub username: String,
    pub timestamp: String,
    pub mods: Vec<String>,
    pub pp: Option<f64>,
    pub score: u64,
    pub grade: char,
    pub great: u32,
    pub ok: u32,
    pub meh: u32,
    pub miss: u32,
    pub accuracy: f64,
    pub max_combo: u32,
    pub avatar_url: String,
    pub cover_url: String,
}

fn grade_color(grade: char) -> &'static str {
    match grade {
        'X' => "#FFD700",
        'S' => "#C0C0C0",
        'A' => "#7BDC35",
        'B' => "#4169E1",
        'C' => "#00CED1",
        'D' => "#808080",
        'F' => "#DC3535",
        _ => "#808080",
    }
}

fn mods_display(mods: &[String]) -> String {
    if mods.is_empty() {
        String::from("NM")
    } else {
        mods.join("")
    }
}

fn format_length(secs: u32) -> String {
    let mins = secs / 60;
    let secs = secs % 60;
    format!("{}:{:02}", mins, secs)
}

pub fn generate_score_card_svg(data: &ScoreCardData) -> String {
    let gc = grade_color(data.grade);
    let mods_str = mods_display(&data.mods);
    let length_str = format_length(data.length_secs);
    let pp_str = data
        .pp
        .map(|p| format!("{:.0}", p))
        .unwrap_or_else(|| "—".to_string());
    let accuracy_str = format!("{:.2}%", data.accuracy);

    let avatar_circle = if data.avatar_url.is_empty() {
        r##"<circle cx="60" cy="340" r="50" fill="#3a3a4a"/>"##.to_string()
    } else {
        format!(
            r##"<image href="{}" x="10" y="290" width="100" height="100" clip-path="url(#avatar-clip)"/>"##,
            data.avatar_url
        )
    };

    let cover_rect = if data.cover_url.is_empty() {
        r##"<rect x="0" y="0" width="120" height="120" fill="#2a2a3a"/>"##.to_string()
    } else {
        format!(
            r##"<image href="{}" width="120" height="120"/>"##,
            data.cover_url
        )
    };

    let grade_badge = format!(
        r##"<g transform="translate(700, 270)">
            <circle cx="60" cy="60" r="55" fill="{}" stroke="#1a1a2a" stroke-width="3"/>
            <text x="60" y="78" text-anchor="middle" font-family="Arial Black, sans-serif" font-size="60" font-weight="bold" fill="white">{}</text>
        </g>"##,
        gc, data.grade
    );

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="500" viewBox="0 0 900 500">
    <defs>
        <clipPath id="avatar-clip">
            <circle cx="60" cy="340" r="50"/>
        </clipPath>
        <linearGradient id="bg-grad" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" style="stop-color:#1a1a2e"/>
            <stop offset="100%" style="stop-color:#16213e"/>
        </linearGradient>
    </defs>
    <rect width="900" height="500" fill="url(#bg-grad)"/>

    <g transform="translate(20, 20)">
        {cover_rect}
        <rect x="130" y="10" width="400" height="30" fill="#2a2a4a"/>
        <text x="140" y="32" font-family="Arial, sans-serif" font-size="18" fill="#e0e0ff" font-weight="bold">{title}</text>
        <text x="140" y="55" font-family="Arial, sans-serif" font-size="14" fill="#b0b0d0">{artist} // mapped by {mapper}</text>
        <text x="140" y="75" font-family="Arial, sans-serif" font-size="12" fill="#8080a0">BID: {bid}</text>
    </g>

    <g transform="translate(20, 140)">
        <rect x="0" y="0" width="100" height="30" fill="#3a3a5a" rx="4"/>
        <text x="50" y="20" text-anchor="middle" font-family="Arial, sans-serif" font-size="14" fill="#FFD700" font-weight="bold">{stars:.1} {difficulty_name}</text>
    </g>

    <g transform="translate(140, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">CS</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{cs}</text>
    </g>
    <g transform="translate(200, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">AR</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{ar}</text>
    </g>
    <g transform="translate(260, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">OD</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{od}</text>
    </g>
    <g transform="translate(320, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">HP</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{hp}</text>
    </g>
    <g transform="translate(380, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">BPM</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{bpm}</text>
    </g>
    <g transform="translate(450, 140)">
        <text x="0" y="12" font-family="Arial, sans-serif" font-size="11" fill="#808090">Length</text>
        <text x="0" y="28" font-family="Arial, sans-serif" font-size="14" fill="white" font-weight="bold">{length}</text>
    </g>

    <g transform="translate(30, 290)">
        {avatar_circle}
        <text x="130" y="330" font-family="Arial, sans-serif" font-size="20" fill="white" font-weight="bold">{username}</text>
        <text x="130" y="350" font-family="Arial, sans-serif" font-size="12" fill="#808090">{timestamp}</text>
        <text x="130" y="370" font-family="Arial, sans-serif" font-size="14" fill="#8080c0">Mods: {mods}</text>
    </g>

    <g transform="translate(30, 400)">
        <text x="0" y="20" font-family="Arial, sans-serif" font-size="12" fill="#808090">PP</text>
        <text x="0" y="50" font-family="Arial Black, sans-serif" font-size="28" fill="#FF69B4" font-weight="bold">{pp}</text>
    </g>

    <g transform="translate(580, 200)">
        <text x="0" y="20" font-family="Arial, sans-serif" font-size="12" fill="#808090">Score</text>
        <text x="0" y="50" font-family="Arial Black, sans-serif" font-size="28" fill="white" font-weight="bold">{score}</text>
    </g>

    {grade_badge}

    <g transform="translate(520, 400)">
        <text x="0" y="15" font-family="Arial, sans-serif" font-size="11" fill="#7BDC35">Great: {great}</text>
        <text x="0" y="35" font-family="Arial, sans-serif" font-size="11" fill="#9CDC35">Ok: {ok}</text>
        <text x="0" y="55" font-family="Arial, sans-serif" font-size="11" fill="#CDC035">Meh: {meh}</text>
        <text x="0" y="75" font-family="Arial, sans-serif" font-size="11" fill="#DC3535">Miss: {miss}</text>
    </g>

    <g transform="translate(680, 400)">
        <text x="0" y="15" font-family="Arial, sans-serif" font-size="12" fill="#808090">Accuracy</text>
        <text x="0" y="40" font-family="Arial Black, sans-serif" font-size="22" fill="white">{accuracy}</text>
        <text x="0" y="65" font-family="Arial, sans-serif" font-size="12" fill="#808090">Combo: {combo}x</text>
    </g>
    <text x="850" y="480" font-family="Arial, sans-serif" font-size="10" fill="#404060">Grade: {grade}</text>
</svg>"##,
        title = data.title,
        artist = data.artist,
        mapper = data.mapper,
        bid = data.bid,
        stars = data.stars,
        difficulty_name = data.difficulty_name,
        cs = data.cs,
        ar = data.ar,
        od = data.od,
        hp = data.hp,
        bpm = data.bpm,
        length = length_str,
        username = data.username,
        timestamp = data.timestamp,
        mods = mods_str,
        pp = pp_str,
        score = data.score,
        grade = data.grade,
        great = data.great,
        ok = data.ok,
        meh = data.meh,
        miss = data.miss,
        accuracy = accuracy_str,
        combo = data.max_combo,
        cover_rect = cover_rect,
        avatar_circle = avatar_circle,
        grade_badge = grade_badge,
    )
}

pub fn rasterize_svg_to_png(svg_bytes: &[u8]) -> Result<Vec<u8>, CacheError> {
    let memory_stream =
        gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg_bytes.to_vec()));
    let handle = rsvg::Loader::new()
        .read_stream::<gio::MemoryInputStream, gio::File, gio::Cancellable>(
            &memory_stream,
            None,
            None,
        )
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    let dims = rsvg::CairoRenderer::new(&handle)
        .intrinsic_size_in_pixels()
        .unwrap_or((900.0, 500.0));
    let width = dims.0.ceil() as i32;
    let height = dims.1.ceil() as i32;

    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;
    let cr = cairo::Context::new(&surface)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    rsvg::CairoRenderer::new(&handle)
        .render_document(
            &cr,
            &cairo::Rectangle::new(0.0, 0.0, f64::from(width), f64::from(height)),
        )
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;

    let mut png_bytes = Vec::new();
    surface
        .write_to_png(&mut png_bytes)
        .map_err(|e| CacheError::SvgRasterizationFailed(e.to_string()))?;
    Ok(png_bytes)
}

pub fn convert_png_to_jpeg(png_bytes: &[u8], quality: u8) -> Result<Vec<u8>, CacheError> {
    let img = image::load_from_memory(png_bytes)
        .map_err(|e| CacheError::SvgRasterizationFailed(format!("failed to load PNG: {}", e)))?;
    let rgb_img = img.to_rgb8();
    let mut jpeg_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut jpeg_bytes);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
    rgb_img
        .write_with_encoder(encoder)
        .map_err(|e| CacheError::SvgRasterizationFailed(format!("failed to encode JPEG: {}", e)))?;
    Ok(jpeg_bytes)
}

pub fn render_score_card_svg(data: &ScoreCardData) -> Result<Vec<u8>, CacheError> {
    let svg = generate_score_card_svg(data);
    let svg_bytes = svg.into_bytes();
    let png_bytes = rasterize_svg_to_png(&svg_bytes)?;
    convert_png_to_jpeg(&png_bytes, 80)
}

impl From<&ScoreCard> for ScoreCardData {
    fn from(sc: &ScoreCard) -> Self {
        let beatmap = &sc.beatmap;
        let score = &sc.score;
        let player = &sc.player;

        let cover_url = beatmap
            .beatmapset
            .as_ref()
            .map(|bs| bs.covers.cover.clone())
            .unwrap_or_default();

        let grade_char = if score.passed {
            score.grade.as_str().chars().next().unwrap_or('D')
        } else {
            'F'
        };

        Self {
            title: beatmap
                .beatmapset
                .as_ref()
                .map(|bs| bs.title.clone())
                .unwrap_or_default(),
            artist: beatmap
                .beatmapset
                .as_ref()
                .map(|bs| bs.artist.clone())
                .unwrap_or_default(),
            mapper: beatmap
                .beatmapset
                .as_ref()
                .map(|bs| bs.creator.clone())
                .unwrap_or_default(),
            bid: beatmap.id as u64,
            difficulty_name: beatmap.version.clone(),
            stars: beatmap.difficulty_rating,
            cs: beatmap.cs as f64,
            ar: beatmap.ar as f64,
            od: beatmap.od as f64,
            hp: beatmap.hp as f64,
            bpm: beatmap.bpm as u32,
            length_secs: beatmap.total_length as u32,
            username: player.username.clone(),
            timestamp: score.ended_at.clone(),
            mods: score.mods.clone(),
            pp: Some(score.pp),
            score: score.score as u64,
            grade: grade_char,
            great: score.statistics.great as u32,
            ok: score.statistics.ok as u32,
            meh: score.statistics.meh as u32,
            miss: score.statistics.miss as u32,
            accuracy: score.accuracy,
            max_combo: score.max_combo as u32,
            avatar_url: player.avatar_url.clone(),
            cover_url,
        }
    }
}

impl From<&Box<ScoreCard>> for ScoreCardData {
    fn from(sc: &Box<ScoreCard>) -> Self {
        ScoreCardData::from(sc.as_ref())
    }
}

impl ScoreCardData {
    pub async fn with_inlined_images(mut self) -> Self {
        let avatar_url = self.avatar_url.clone();
        let cover_url = self.cover_url.clone();

        let avatar_fut = async {
            if avatar_url.is_empty() {
                None
            } else {
                match crate::fetch_url_as_data_uri(&avatar_url).await {
                    Ok(data_uri) => Some(data_uri),
                    Err(e) => {
                        warn!(url = %avatar_url, error = %e, "Failed to inline avatar image for score card");
                        None
                    }
                }
            }
        };
        let cover_fut = async {
            if cover_url.is_empty() {
                None
            } else {
                match crate::fetch_url_as_data_uri(&cover_url).await {
                    Ok(data_uri) => Some(data_uri),
                    Err(e) => {
                        warn!(url = %cover_url, error = %e, "Failed to inline cover image for score card");
                        None
                    }
                }
            }
        };

        let (avatar_result, cover_result) = tokio::join!(avatar_fut, cover_fut);

        if let Some(data_uri) = avatar_result {
            self.avatar_url = data_uri;
        }
        if let Some(data_uri) = cover_result {
            self.cover_url = data_uri;
        }
        self
    }
}
