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

//! osu! FastRandom（xorshift128）与无状态 MurmurHash3-finalizer 随机数。
//! 调用顺序必须与 lazer / stable 的实现完全一致（决定香蕉/水滴的随机位置）。

pub struct LegacyRandom {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
}

impl LegacyRandom {
    pub fn new(seed: u32) -> Self {
        LegacyRandom {
            x: seed,
            y: 842502087,
            z: 3579807591,
            w: 273326509,
        }
    }

    /// xorshift128 核心步进。
    pub fn next_uint(&mut self) -> u32 {
        let t = self.x ^ (self.x << 11);
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        self.w = self.w ^ (self.w >> 19) ^ t ^ (t >> 8);
        self.w
    }

    /// 非负 31 位整数（与 .NET Random.Next() 语义对应）。
    pub fn next(&mut self) -> i32 {
        (self.next_uint() & 0x7FFF_FFFF) as i32
    }

    /// [lower, upper) 区间整数。
    pub fn next_range(&mut self, lower: i32, upper: i32) -> i32 {
        (lower as f64 + self.next_double() * (upper - lower) as f64) as i32
    }

    /// [0, 1) 双精度。
    pub fn next_double(&mut self) -> f64 {
        (self.next_uint() & 0x7FFF_FFFF) as f64 / 2147483648.0
    }
}

/// MurmurHash3 finalizer 混淆。
fn stateless_mix(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    value ^= value >> 33;
    value = value.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    value ^= value >> 33;
    value
}

/// 无状态随机：由（seed, series）直接得到 u64（lazer StatelessRNG）。
fn stateless_next_ulong(seed: i64, series: i64) -> u64 {
    let combined = (((series as u64) & 0xFFFF_FFFF) << 32) | ((seed as u64) & 0xFFFF_FFFF);
    stateless_mix(combined ^ 0x1234_5678)
}

/// 无状态随机：[0, max_value) 区间整数（用于香蕉颜色选择）。
pub fn stateless_next_int(max_value: u64, seed: i64, series: i64) -> u64 {
    if max_value == 0 {
        return 0;
    }
    stateless_next_ulong(seed, series) % max_value
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED_0_EXPECTED: [u32; 100] = [
        273327012, 2660065245, 3082852308, 4072804168, 3083912503, 1129781159, 4036646446,
        314621754, 587741480, 3625580737, 4195413706, 3980306397, 2409431595, 2555722985,
        3847157655, 4250313900, 2596105591, 2873014461, 962465794, 1902485569, 12758479,
        1583810106, 2540753908, 3410693875, 3740429952, 3099784626, 2839313965, 982761839,
        2002267859, 3633851119, 2451565255, 908319058, 2159757299, 2574109273, 4098441825,
        3699965171, 2253831246, 1906217439, 3480915911, 1418632639, 1728704428, 3801914107,
        4186125784, 3653250098, 4053584937, 4157725603, 387718516, 3479503385, 3633275480,
        2717562409, 1459284191, 2976406995, 307655684, 1718207385, 3881067419, 337722471,
        3044449459, 2636903567, 3643216906, 3304638797, 3312076389, 120020427, 3863060065,
        3841036613, 1933556432, 1321957497, 2761119063, 3430974265, 1122453827, 1476065750,
        1778198419, 2712467678, 3648590073, 1506637385, 3649831772, 522222267, 248471422,
        1015160784, 2242735615, 2571381331, 3976503978, 3243531333, 796398341, 2547429899,
        1594835881, 1020955257, 3553973980, 4054966343, 3565610963, 1021785720, 1174267022,
        993036206, 3630909284, 3711631079, 2001315930, 3371916522, 1244230390, 1089368210,
        2061437869, 1758532624,
    ];

    const SEED_12345_EXPECTED: [u32; 100] = [
        298506853, 2668439084, 3057708565, 4081219257, 3021767494, 1121382319, 4082602406,
        323012299, 1125544278, 3058189580, 4192020674, 3971932076, 483565484, 3896957215,
        4174211053, 3801384167, 1781745894, 3026447006, 654442992, 1643579979, 2440235755,
        988755265, 232111725, 3567864014, 3716424930, 2659338030, 1040491589, 2692052083,
        1546319787, 3491590199, 3407921405, 3237864198, 3412181789, 4039544462, 1026334531,
        317107763, 3613064793, 321693890, 1252260707, 1829983026, 1841021183, 463937330,
        1317701574, 3120023526, 184046623, 696616189, 858489532, 887405512, 4235479527, 4191394040,
        2525158172, 2276002047, 3654046364, 3166504197, 1013412789, 4081478198, 1218813552,
        476327756, 463015342, 3679372081, 3212611638, 2187319002, 1366839106, 4236400919,
        2804194193, 3685834921, 1210066626, 2793702448, 637500258, 1964761948, 1011141899,
        3194186679, 1620652914, 3392290110, 3489963494, 1973381762, 3694446048, 2276771989,
        1940626984, 4260241829, 2274353213, 2791202873, 2301358084, 88345485, 4247948560,
        2822268199, 2118454260, 1534623698, 843785120, 1496410515, 259904490, 2539895608,
        4234075223, 787566526, 3488754441, 1170404276, 1333011263, 3948681680, 3047738833,
        3886405507,
    ];

    const SEED_U32MAX_EXPECTED: [u32; 100] = [
        273327196, 2660063269, 3082850348, 4072802480, 3085908720, 1129779288, 4037388782,
        314620098, 3694541008, 655894238, 4197388554, 3980304421, 2158228970, 1718296054,
        3830380400, 4234763348, 1677388438, 1564528562, 3458614813, 2015045745, 3422671806,
        2540153043, 1804403340, 3396663084, 136991231, 2728476953, 2705043107, 3432767551,
        2523198970, 1046857638, 1113012351, 1373549074, 3768866550, 4042057139, 193179409,
        2942943825, 1840784044, 4209939413, 3959640254, 228162597, 2711130744, 795266469,
        3642158679, 531159300, 2059830235, 1674863299, 232676536, 1424475390, 443633122,
        3665032725, 632769151, 1327348227, 3751014495, 2585097958, 91108201, 2801884261,
        3880800722, 3603147160, 2722250066, 252559115, 1790353913, 2717010526, 317283231,
        1887324974, 2937623985, 2626394806, 3303415579, 1178803090, 765311990, 3976448729,
        434704037, 1193810305, 2276087961, 1946951607, 633449397, 572676765, 4154040528,
        3816613939, 3418909674, 4212012732, 3249247789, 3314729819, 1301116728, 3350991843,
        1513877447, 165705664, 804451699, 150394981, 2369145707, 2271376250, 1065572232,
        2164823533, 3193633716, 708294310, 254647167, 3394224266, 2745238828, 884217208,
        1461815026, 3822999308,
    ];

    #[test]
    fn test_seed_0_first_100() {
        let mut rng = LegacyRandom::new(0);
        for &exp in SEED_0_EXPECTED.iter() {
            assert_eq!(rng.next_uint(), exp);
        }
    }

    #[test]
    fn test_seed_12345_first_100() {
        let mut rng = LegacyRandom::new(12345);
        for &exp in SEED_12345_EXPECTED.iter() {
            assert_eq!(rng.next_uint(), exp);
        }
    }

    #[test]
    fn test_seed_u32max_first_100() {
        let mut rng = LegacyRandom::new(u32::MAX);
        for &exp in SEED_U32MAX_EXPECTED.iter() {
            assert_eq!(rng.next_uint(), exp);
        }
    }

    #[test]
    fn test_next_double_range() {
        let mut rng = LegacyRandom::new(42);
        for _ in 0..1000 {
            let d = rng.next_double();
            assert!(d >= 0.0 && d < 1.0, "next_double() out of range: {d}");
        }
    }

    #[test]
    fn test_next_range_bounds() {
        let mut rng = LegacyRandom::new(9999);
        for _ in 0..1000 {
            let v = rng.next_range(5, 20);
            assert!(v >= 5 && v < 20, "next_range(5, 20) out of bounds: {v}");
        }
    }

    #[test]
    fn test_reproducibility() {
        let seed = 0xDEAD_BEEF;
        let mut rng1 = LegacyRandom::new(seed);
        let mut rng2 = LegacyRandom::new(seed);
        for _ in 0..100 {
            assert_eq!(rng1.next_uint(), rng2.next_uint());
        }
        for _ in 0..100 {
            assert_eq!(rng1.next(), rng2.next());
        }
        for _ in 0..100 {
            assert!((rng1.next_double() - rng2.next_double()).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_next_is_non_negative() {
        let mut rng = LegacyRandom::new(7);
        for _ in 0..1000 {
            let v = rng.next();
            assert!(v >= 0, "next() returned negative: {v}");
        }
    }
}
