use crate::api::sb_api::{SbPlayerInfoFull, SbPlayerStats, SbScore, SbScoreBeatmap};
use crate::types::{GameMode, UserStats};
use osubot_types::Score;

#[allow(dead_code)]
pub fn sb_legacy_mods_to_game_mods(mods: i64, mode: GameMode) -> rosu_mods::GameMods {
    let bits = mods as u32;
    let intermode = rosu_mods::GameModsIntermode::from_bits(bits);
    let ros_mode: rosu_mods::GameMode = mode.into();
    intermode.with_mode(ros_mode)
}

pub fn sb_player_to_user_stats(info: &SbPlayerInfoFull, mode: GameMode) -> UserStats {
    let mode_int = i32::from(mode);
    let stats = info.stats.get(&mode_int).cloned().unwrap_or(SbPlayerStats {
        mode: mode_int,
        pp: 0.0,
        accuracy: 0.0,
        total_score: 0,
        ranked_score: 0,
        play_count: 0,
        play_time: 0,
        global_rank: 0,
        country_rank: 0,
        max_combo: 0,
        total_hits: 0,
        count_ssh: 0,
        count_ss: 0,
        count_sh: 0,
        count_s: 0,
        count_a: 0,
    });

    UserStats {
        user_id: info.id,
        username: info.name.clone(),
        pp: stats.pp,
        rank: stats.global_rank,
        country_rank: stats.country_rank,
        country_code: info.country.clone(),
        ranked_score: stats.ranked_score,
        accuracy: stats.accuracy,
        playcount: stats.play_count,
        hits: stats.total_hits,
        playtime: stats.play_time,
        rank_change: None,
        country_rank_change: None,
        cover_url: Some(format!("https://a.ppy.sb/{}", info.id)),
    }
}

#[allow(dead_code)]
pub fn sb_score_to_score(sb: &SbScore, beatmap: Option<&SbScoreBeatmap>, mode: GameMode) -> Score {
    let mods = sb_legacy_mods_to_game_mods(sb.mods, mode);

    Score {
        score_id: sb.id.unwrap_or(0),
        beatmap_id: beatmap.map(|b| b.id).unwrap_or(0),
        beatmapset_id: beatmap.map(|b| b.set_id).unwrap_or(0),
        artist: beatmap.map(|b| b.artist.clone()).unwrap_or_default(),
        title: beatmap.map(|b| b.title.clone()).unwrap_or_default(),
        version: beatmap.map(|b| b.version.clone()).unwrap_or_default(),
        creator: beatmap.map(|b| b.creator.clone()).unwrap_or_default(),
        star_rating: beatmap.map(|b| b.star_rating).unwrap_or(0.0),
        bpm: beatmap.map(|b| b.bpm).unwrap_or(0.0),
        ar: beatmap.map(|b| b.ar).unwrap_or(0.0),
        od: beatmap.map(|b| b.od).unwrap_or(0.0),
        cs: beatmap.map(|b| b.cs).unwrap_or(0.0),
        hp: beatmap.map(|b| b.hp).unwrap_or(0.0),
        length_seconds: beatmap.map(|b| b.total_length).unwrap_or(0),
        score_value: sb.score,
        accuracy: sb.accuracy,
        max_combo: sb.max_combo,
        beatmap_max_combo: beatmap.map(|b| b.max_combo).unwrap_or(0),
        pp: Some(sb.pp),
        pp_breakdown: None,
        pp_if_acc: None,
        perfect_pp: None,
        rank: sb.grade.clone(),
        passed: sb.status == 0,
        mods,
        is_perfect: sb.perfect,
        created_at: sb.play_time.clone(),
        is_lazer: false,
        has_replay: false,
        legacy_score_id: None,
        statistics: osubot_types::ScoreStatistics {
            count_300: sb.n300,
            count_100: sb.n100,
            count_50: sb.n50,
            count_miss: sb.nmiss,
            count_geki: sb.ngeki,
            count_katu: sb.nkatu,
            osu_large_tick_hits: 0,
            osu_small_tick_hits: 0,
            osu_slider_tail_hits: 0,
            osu_large_tick_misses: 0,
            osu_small_tick_misses: 0,
        },
        cover_url: beatmap
            .map(|b| format!("https://a.ppy.sb/beatmaps/{}/covers/fullsize.jpg", b.id))
            .unwrap_or_default(),
        user: osubot_types::ScoreUser {
            avatar_url: String::new(),
            country_code: String::new(),
            user_id: None,
            username: None,
            global_rank: None,
            country_rank: None,
            pp: 0.0,
        },
        fav_count: None,
        play_count: None,
        status: String::new(),
    }
}
