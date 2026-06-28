use osubot_render::{render_match_result_card, MatchResultParams, MatchResultPlayerParams};

#[derive(Debug)]
struct MatchCardPixelStats {
    width: u32,
    height: u32,
    dark_pixels: usize,
    bright_pixels: usize,
    foreground_edge_pixels: usize,
    max_luminance: i16,
}

fn make_test_cover() -> image::DynamicImage {
    let mut img = image::RgbImage::new(640, 360);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = ((x * 255) / 639) as u8;
        let g = ((y * 180) / 359) as u8;
        let b = (120 + ((x + y) % 120)) as u8;
        *pixel = image::Rgb([r, g, b]);
    }
    image::DynamicImage::ImageRgb8(img)
}

fn make_test_avatar(seed: u8) -> image::DynamicImage {
    let mut img = image::RgbImage::new(96, 96);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let edge = x < 10 || y < 10 || x > 85 || y > 85;
        let r = if edge { 240 } else { seed };
        let g = if edge { 240 } else { 120 };
        let b = if edge {
            240
        } else {
            220_u8.saturating_sub(seed / 2)
        };
        *pixel = image::Rgb([r, g, b]);
    }
    image::DynamicImage::ImageRgb8(img)
}

fn luminance(pixel: &image::Rgb<u8>) -> i16 {
    ((u32::from(pixel[0]) * 299 + u32::from(pixel[1]) * 587 + u32::from(pixel[2]) * 114) / 1000)
        as i16
}

fn decoded_match_card_stats(jpeg: &[u8]) -> MatchCardPixelStats {
    let rgb = image::load_from_memory(jpeg)
        .expect("rendered match card should be a decodable image")
        .to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut dark_pixels = 0;
    let mut bright_pixels = 0;
    let mut foreground_edge_pixels = 0;
    let mut max_luminance = 0;

    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let current = luminance(rgb.get_pixel(x, y));
            max_luminance = max_luminance.max(current);
            if current < 80 {
                dark_pixels += 1;
            }
            if current > 155 {
                bright_pixels += 1;
            }

            let right = luminance(rgb.get_pixel(x + 1, y));
            let below = luminance(rgb.get_pixel(x, y + 1));
            if current > 110 && ((current - right).abs() > 45 || (current - below).abs() > 45) {
                foreground_edge_pixels += 1;
            }
        }
    }

    MatchCardPixelStats {
        width,
        height,
        dark_pixels,
        bright_pixels,
        foreground_edge_pixels,
        max_luminance,
    }
}

fn make_player(index: usize) -> MatchResultPlayerParams {
    MatchResultPlayerParams {
        placement: index,
        username: format!("Player{index:02}WithLongName"),
        avatar_url: None,
        avatar_image: Some(make_test_avatar(index as u8 * 11)),
        team: Some(
            if index.is_multiple_of(2) {
                "Blue"
            } else {
                "Red"
            }
            .to_string(),
        ),
        score: 10_000_000_u64.saturating_sub(index as u64 * 321_987),
        accuracy: 0.99 - (index as f64 * 0.003),
        max_combo: 1800_u32.saturating_sub(index as u32 * 41),
        mods: if index.is_multiple_of(3) {
            vec!["HD".to_string(), "HR".to_string()]
        } else {
            Vec::new()
        },
        rank: if index.is_multiple_of(5) {
            "F".to_string()
        } else if index == 1 {
            "S".to_string()
        } else {
            "A".to_string()
        },
        passed: !index.is_multiple_of(5),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_match_result_card_produces_jpeg() {
    let params = MatchResultParams {
        match_id: 12345678,
        match_name: "Test Match".to_string(),
        event_label: "Game finished".to_string(),
        played_at: "2026/06/26 20:00:00".to_string(),
        beatmap_id: 987654,
        beatmap_artist: "xi".to_string(),
        beatmap_title: "Blue Zenith".to_string(),
        beatmap_version: "FOUR DIMENSIONS".to_string(),
        beatmap_mapper: "Asphyxia".to_string(),
        beatmap_mode: "osu!".to_string(),
        star_rating: Some(7.85),
        beatmap_bpm: Some(190.0),
        beatmap_length_seconds: Some(260),
        beatmap_max_combo: Some(2429),
        beatmap_ar: Some(9.7),
        beatmap_od: Some(9.8),
        beatmap_cs: Some(4.0),
        beatmap_hp: Some(6.0),
        cover_image: Some(make_test_cover()),
        is_started: false,
        selected_mods: vec!["HD".to_string()],
        team_type: Some("team-vs".to_string()),
        scoring_type: Some("score".to_string()),
        team_results: Vec::new(),
        players: vec![
            MatchResultPlayerParams {
                placement: 1,
                username: "Alice".to_string(),
                avatar_url: None,
                avatar_image: Some(make_test_avatar(40)),
                team: Some("Red".to_string()),
                score: 1_234_567,
                accuracy: 0.9876,
                max_combo: 1234,
                mods: vec!["HD".to_string(), "HR".to_string()],
                rank: "A".to_string(),
                passed: true,
            },
            MatchResultPlayerParams {
                placement: 2,
                username: "Bob".to_string(),
                avatar_url: None,
                avatar_image: None,
                team: Some("Blue".to_string()),
                score: 987_654,
                accuracy: 0.9654,
                max_combo: 876,
                mods: Vec::new(),
                rank: "B".to_string(),
                passed: false,
            },
        ],
    };

    let jpeg = render_match_result_card(params).await.expect("render ok");
    assert!(!jpeg.is_empty());
    assert!(jpeg.len() > 1024);
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);

    let stats = decoded_match_card_stats(&jpeg);
    assert_eq!(stats.width, 1920);
    assert!(
        stats.height >= 1080,
        "match card should be at least the base card height, got {stats:?}"
    );
    assert!(
        stats.dark_pixels > 1_000_000,
        "match card should contain the dark card background, got {stats:?}"
    );
    assert!(
        stats.bright_pixels > 5_000,
        "match card should contain visible foreground/text pixels, got {stats:?}"
    );
    assert!(
        stats.max_luminance > 155,
        "match card should contain bright foreground content, got {stats:?}"
    );
    assert!(
        stats.foreground_edge_pixels > 1_000,
        "match card should contain readable high-contrast content edges, got {stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_match_result_card_handles_sixteen_players() {
    let params = MatchResultParams {
        match_id: 12345678,
        match_name: "Sixteen Player Test Match With Long Name".to_string(),
        event_label: "Game finished".to_string(),
        played_at: "2026/06/26 20:00:00".to_string(),
        beatmap_id: 987654,
        beatmap_artist: "xi".to_string(),
        beatmap_title: "Blue Zenith".to_string(),
        beatmap_version: "FOUR DIMENSIONS".to_string(),
        beatmap_mapper: "MapperWithAnExcessivelyLongNameForOverflowTesting".to_string(),
        beatmap_mode: "osu!".to_string(),
        star_rating: Some(7.85),
        beatmap_bpm: Some(190.0),
        beatmap_length_seconds: Some(260),
        beatmap_max_combo: Some(2429),
        beatmap_ar: Some(9.7),
        beatmap_od: Some(9.8),
        beatmap_cs: Some(4.0),
        beatmap_hp: Some(6.0),
        cover_image: Some(make_test_cover()),
        is_started: false,
        selected_mods: Vec::new(),
        team_type: Some("team-vs".to_string()),
        scoring_type: Some("score".to_string()),
        team_results: Vec::new(),
        players: (1..=16).map(make_player).collect(),
    };

    let jpeg = render_match_result_card(params).await.expect("render ok");
    assert!(!jpeg.is_empty());
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);

    let stats = decoded_match_card_stats(&jpeg);
    assert_eq!(stats.width, 1920);
    assert_eq!(stats.height, 1080);
    assert!(
        stats.bright_pixels > 8_000,
        "16-player match card should retain visible text/content, got {stats:?}"
    );
    assert!(
        stats.foreground_edge_pixels > 1_500,
        "16-player match card should keep readable high-contrast content edges, got {stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_started_match_card_shows_participants() {
    let params = MatchResultParams {
        match_id: 12345678,
        match_name: "Started Match".to_string(),
        event_label: "场次开始".to_string(),
        played_at: "2026/06/26 20:00:00".to_string(),
        beatmap_id: 987654,
        beatmap_artist: "xi".to_string(),
        beatmap_title: "Blue Zenith".to_string(),
        beatmap_version: "FOUR DIMENSIONS".to_string(),
        beatmap_mapper: "Asphyxia".to_string(),
        beatmap_mode: "osu!".to_string(),
        star_rating: Some(7.85),
        beatmap_bpm: Some(190.0),
        beatmap_length_seconds: Some(260),
        beatmap_max_combo: Some(2429),
        beatmap_ar: Some(9.7),
        beatmap_od: Some(9.8),
        beatmap_cs: Some(4.0),
        beatmap_hp: Some(6.0),
        cover_image: Some(make_test_cover()),
        is_started: true,
        selected_mods: vec!["DT".to_string(), "NF".to_string()],
        team_type: Some("team-vs".to_string()),
        scoring_type: Some("score".to_string()),
        team_results: Vec::new(),
        players: (1..=4)
            .map(|idx| MatchResultPlayerParams {
                score: 0,
                accuracy: 0.0,
                max_combo: 0,
                mods: Vec::new(),
                passed: true,
                ..make_player(idx)
            })
            .collect(),
    };

    let jpeg = render_match_result_card(params).await.expect("render ok");
    assert!(!jpeg.is_empty());
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);

    let stats = decoded_match_card_stats(&jpeg);
    assert_eq!(stats.width, 1920);
    assert_eq!(stats.height, 1080);
    assert!(
        stats.bright_pixels > 8_000,
        "started match card should retain visible participant content, got {stats:?}"
    );
    assert!(
        stats.foreground_edge_pixels > 1_500,
        "started match card should keep readable participant edges, got {stats:?}"
    );
}
