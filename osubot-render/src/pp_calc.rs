use crate::error::RenderError;
use rosu_pp::{any::PerformanceAttributes, Beatmap, Performance};

pub struct PpBreakdown {
    pub aim: f64,
    pub speed: f64,
    pub accuracy: f64,
    pub flashlight: f64,
    pub effective_miss_count: i64,
    pub full_pp: f64,
    pub perfect_pp: f64,
}

fn mod_bitmask(mods: &[String]) -> u32 {
    let mut bits = 0u32;
    for m in mods {
        bits |= match m.as_str() {
            "NF" => 1,
            "EZ" => 2,
            "HD" => 8,
            "HR" => 16,
            "SD" | "PF" => 32,
            "DT" => 64,
            "HT" => 256,
            "NC" => 64 | 512,
            "FL" => 1024,
            "SO" => 4096,
            _ => 0,
        };
    }
    bits
}

fn extract_attrs(attrs: &PerformanceAttributes) -> (f64, f64, f64, f64, i64) {
    match attrs {
        PerformanceAttributes::Osu(a) => (
            a.pp_aim,
            a.pp_speed,
            a.pp_acc,
            a.pp_flashlight,
            a.effective_miss_count as i64,
        ),
        _ => (0.0, 0.0, 0.0, 0.0, 0),
    }
}

pub fn calculate_pp_breakdown(
    beatmap_bytes: &[u8],
    n300: i64,
    n100: i64,
    n50: i64,
    misses: i64,
    combo: i64,
    max_combo: Option<i64>,
    mods: &[String],
) -> Result<PpBreakdown, RenderError> {
    let beatmap = Beatmap::from_bytes(beatmap_bytes)
        .map_err(|e| RenderError::Render(format!("beatmap parse: {}", e)))?;

    let mods_bits = mod_bitmask(mods);
    let fc_combo = max_combo.unwrap_or(combo).max(combo);

    let attrs = Performance::new(&beatmap)
        .mods(mods_bits)
        .n300(n300 as u32)
        .n100(n100 as u32)
        .n50(n50 as u32)
        .misses(misses as u32)
        .combo(combo as u32)
        .calculate();

    let fc_attrs = Performance::new(&beatmap)
        .mods(mods_bits)
        .n300(n300 as u32)
        .n100(n100 as u32)
        .n50(n50 as u32)
        .misses(0)
        .combo(fc_combo as u32)
        .calculate();

    let total_hits = n300 + n100 + n50 + misses;
    let pf_attrs = Performance::new(&beatmap)
        .mods(mods_bits)
        .n300(total_hits as u32)
        .n100(0)
        .n50(0)
        .misses(0)
        .combo(fc_combo as u32)
        .calculate();

    let (aim, speed, accuracy, flashlight, effective_miss_count) = extract_attrs(&attrs);

    Ok(PpBreakdown {
        aim,
        speed,
        accuracy,
        flashlight,
        effective_miss_count,
        full_pp: fc_attrs.pp(),
        perfect_pp: pf_attrs.pp(),
    })
}
