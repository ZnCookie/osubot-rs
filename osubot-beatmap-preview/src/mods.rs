// Copyright (c) 2026 xuan_yuan (from osu-beatmap-preview, MIT licensed)
// Copyright (c) 2026 ZnCookie
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use crate::errors::{PreviewError, Result};

#[derive(Debug, Clone, Default)]
pub struct ModSettings {
    pub speed_multiplier: f64,
    pub double_time: bool,
    pub half_time: bool,

    pub da_cs: Option<f64>,
    pub da_ar: Option<f64>,
    pub da_od: Option<f64>,
    pub da_hp: Option<f64>,

    pub easy: bool,
    pub hard_rock: bool,
    pub hidden: bool,

    pub swap: bool,
    pub cs_override: bool,

    pub mania_keys: Option<i32>,
    pub mania_key_mods: Vec<i32>,
    pub dual_stage: bool,
    pub inverse: bool,
    pub hold_off: bool,

    pub tokens: Vec<String>,
}

impl ModSettings {
    pub fn new() -> Self {
        ModSettings {
            speed_multiplier: 1.0,
            ..Default::default()
        }
    }

    pub fn has_da(&self) -> bool {
        self.da_cs.is_some() || self.da_ar.is_some() || self.da_od.is_some() || self.da_hp.is_some()
    }

    pub fn has_any_mod(&self) -> bool {
        self.speed_multiplier != 1.0
            || self.has_da()
            || self.easy
            || self.hard_rock
            || self.hidden
            || self.swap
            || self.cs_override
            || self.mania_keys.is_some()
            || self.dual_stage
            || self.inverse
            || self.hold_off
    }
}

pub fn parse_mods(mod_str: &str) -> Result<ModSettings> {
    let mut settings = ModSettings::new();
    if mod_str.trim().is_empty() {
        return Ok(settings);
    }
    let tokens: Vec<String> = mod_str
        .split('+')
        .map(|t| t.trim().to_uppercase())
        .filter(|t| !t.is_empty())
        .collect();
    settings.tokens = tokens.clone();
    for token in &tokens {
        parse_one_token(token, &mut settings)?;
    }
    Ok(settings)
}

fn parse_one_token(token: &str, s: &mut ModSettings) -> Result<()> {
    if let Some(tail) = token.strip_prefix("DA") {
        return parse_da_token(tail, s);
    }

    // DT/HT with optional speed value
    if token.starts_with("DT") || token.starts_with("HT") {
        let (kind, rest) = token.split_at(2);
        if rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit() || c == '.') {
            let raw_val = if rest.is_empty() { None } else { Some(rest) };
            if kind == "DT" {
                let val = match raw_val {
                    Some(r) => parse_float(r, token)?,
                    None => 1.5,
                };
                if !(1.01..=2.00).contains(&val) {
                    return Err(PreviewError::new(format!(
                        "DT speed must be in [1.01, 2.0], got {}",
                        fmt_float(val)
                    )));
                }
                s.speed_multiplier = val;
                s.double_time = true;
            } else {
                let val = match raw_val {
                    Some(r) => parse_float(r, token)?,
                    None => 0.75,
                };
                if !(0.5..=0.99).contains(&val) {
                    return Err(PreviewError::new(format!(
                        "HT speed must be in [0.5, 0.99], got {}",
                        fmt_float(val)
                    )));
                }
                s.speed_multiplier = val;
                s.half_time = true;
            }
            return Ok(());
        }
    }

    // <n>K
    if let Some(num) = token.strip_suffix('K') {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            let keys: i32 = num
                .parse()
                .map_err(|_| PreviewError::new(format!("mania keys must be 1-10, got {num}")))?;
            if !(1..=10).contains(&keys) {
                return Err(PreviewError::new(format!(
                    "mania keys must be 1-10, got {keys}"
                )));
            }
            if s.mania_keys.is_none() {
                s.mania_keys = Some(keys);
            }
            s.mania_key_mods.push(keys);
            return Ok(());
        }
    }

    match token {
        "EZ" => s.easy = true,
        "HR" => s.hard_rock = true,
        "HD" => s.hidden = true,
        "SW" => s.swap = true,
        "CS" => s.cs_override = true,
        "DS" => s.dual_stage = true,
        "IN" => s.inverse = true,
        "HO" => s.hold_off = true,
        _ => {
            // Skip unsupported mod tokens (e.g. NF, FL, BT)
        }
    }
    Ok(())
}

fn parse_da_token(tail: &str, s: &mut ModSettings) -> Result<()> {
    let bytes = tail.as_bytes();
    let mut pos = 0;
    let mut matched = false;
    while pos < bytes.len() {
        let rest = &tail[pos..];
        let lower = rest.to_lowercase();
        let param = if lower.starts_with("ar") {
            "AR"
        } else if lower.starts_with("cs") {
            "CS"
        } else if lower.starts_with("od") {
            "OD"
        } else if lower.starts_with("hp") {
            "HP"
        } else {
            break;
        };
        // numeric part: -?[\d.]+
        let num_start = pos + 2;
        let mut num_end = num_start;
        let b = tail.as_bytes();
        if num_end < b.len() && b[num_end] == b'-' {
            num_end += 1;
        }
        let digits_start = num_end;
        while num_end < b.len() && (b[num_end].is_ascii_digit() || b[num_end] == b'.') {
            num_end += 1;
        }
        if num_end == digits_start {
            break;
        }
        matched = true;
        let val = parse_float(&tail[num_start..num_end], &format!("DA{tail}"))?;
        set_da_param(param, val, s)?;
        pos = num_end;
    }

    if !matched {
        return Err(PreviewError::new(format!(
            "DA mod requires at least one parameter (ar/cs/od/hp), got: '{tail}'"
        )));
    }
    if pos < tail.len() {
        return Err(PreviewError::new(format!(
            "unexpected content after DA params: '{}'",
            &tail[pos..]
        )));
    }
    Ok(())
}

fn set_da_param(param: &str, val: f64, s: &mut ModSettings) -> Result<()> {
    let check = |min: f64, max: f64, name: &str| -> Result<()> {
        if val < min || val > max {
            Err(PreviewError::new(format!(
                "DA {name} must be in [{}, {}], got {}",
                fmt_float(min),
                fmt_float(max),
                fmt_float(val)
            )))
        } else {
            Ok(())
        }
    };
    match param {
        "CS" => {
            check(0.0, 11.0, "CS")?;
            s.da_cs = Some(val);
        }
        "AR" => {
            check(-10.0, 11.0, "AR")?;
            s.da_ar = Some(val);
        }
        "OD" => {
            check(0.0, 11.0, "OD")?;
            s.da_od = Some(val);
        }
        "HP" => {
            check(0.0, 11.0, "HP")?;
            s.da_hp = Some(val);
        }
        _ => {
            return Err(PreviewError::new(format!("unknown DA param: {param}")));
        }
    }
    Ok(())
}

fn parse_float(raw: &str, token: &str) -> Result<f64> {
    raw.parse::<f64>()
        .map_err(|_| PreviewError::new(format!("invalid numeric value in mod token: '{token}'")))
}

fn fmt_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e16 {
        format!("{:.1}", v)
    } else {
        format!("{}", v)
    }
}

/// Check inter-mod conflicts and mode-specific compatibility.
/// Returns a list of human-readable error messages (empty if valid).
/// Does not validate per-format (GIF/PNG) mod support — that's left to the
/// renderer to decide.
#[must_use]
pub fn validate_mods(settings: &ModSettings, mode: Option<i32>) -> Vec<String> {
    let mut errors = Vec::new();

    if settings.double_time && settings.half_time {
        errors.push("DT and HT cannot be used together".to_string());
    }
    if settings.easy && settings.hard_rock {
        errors.push("EZ and HR cannot be used together".to_string());
    }
    if settings.mania_key_mods.len() > 1 {
        let keys: Vec<String> = settings
            .mania_key_mods
            .iter()
            .map(|k| format!("{k}K"))
            .collect();
        errors.push(format!(
            "mania key mods cannot be used together: {}",
            keys.join(", ")
        ));
    }

    if mode == Some(0) {
        if settings.has_da() && settings.easy {
            errors.push("DA and EZ cannot be used together".to_string());
        }
        if settings.has_da() && settings.hard_rock {
            errors.push("DA and HR cannot be used together".to_string());
        }
    }
    if mode == Some(3) && settings.inverse && settings.hold_off {
        errors.push("IN and HO cannot be used together".to_string());
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> ModSettings {
        ModSettings::new()
    }

    #[test]
    fn valid_settings_no_errors() {
        assert!(validate_mods(&s(), None).is_empty());
    }

    #[test]
    fn dt_and_ht_conflict() {
        let mut m = s();
        m.double_time = true;
        m.half_time = true;
        let errs = validate_mods(&m, None);
        assert!(errs.iter().any(|e| e.contains("DT and HT")));
    }

    #[test]
    fn ez_and_hr_conflict() {
        let mut m = s();
        m.easy = true;
        m.hard_rock = true;
        let errs = validate_mods(&m, None);
        assert!(errs.iter().any(|e| e.contains("EZ and HR")));
    }

    #[test]
    fn multiple_mania_keys_conflict() {
        let mut m = s();
        m.mania_key_mods = vec![4, 7];
        let errs = validate_mods(&m, None);
        assert!(errs.iter().any(|e| e.contains("mania key mods")));
    }

    #[test]
    fn da_and_ez_conflict_in_std() {
        let mut m = s();
        m.da_cs = Some(4.0);
        m.easy = true;
        let errs = validate_mods(&m, Some(0));
        assert!(errs.iter().any(|e| e.contains("DA and EZ")));
    }

    #[test]
    fn da_and_ez_no_conflict_in_taiko() {
        let mut m = s();
        m.da_cs = Some(4.0);
        m.easy = true;
        assert!(validate_mods(&m, Some(1)).is_empty());
    }

    #[test]
    fn da_and_hr_conflict_in_std() {
        let mut m = s();
        m.da_ar = Some(9.0);
        m.hard_rock = true;
        let errs = validate_mods(&m, Some(0));
        assert!(errs.iter().any(|e| e.contains("DA and HR")));
    }

    #[test]
    fn in_and_ho_conflict_in_mania() {
        let mut m = s();
        m.inverse = true;
        m.hold_off = true;
        let errs = validate_mods(&m, Some(3));
        assert!(errs.iter().any(|e| e.contains("IN and HO")));
    }

    #[test]
    fn in_and_ho_no_conflict_in_std() {
        let mut m = s();
        m.inverse = true;
        m.hold_off = true;
        assert!(validate_mods(&m, Some(0)).is_empty());
    }

    #[test]
    fn single_mania_key_ok() {
        let mut m = s();
        m.mania_key_mods = vec![4];
        assert!(validate_mods(&m, None).is_empty());
    }
}
