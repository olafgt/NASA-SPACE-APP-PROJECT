//! # telemetry-protocol
//!
//! Everything both machines must agree on:
//!
//! * the custom BLE GATT **service** and its **characteristic** UUIDs,
//! * the [`RawTelemetry`] record (also the JSON-lines payload for the TCP
//!   loopback transport and for the Rust -> Python pipe),
//! * the compact binary frames sent in BLE notifications.
//!
//! ## Two characteristics
//!
//! | characteristic | frame | content |
//! |---|---|---|
//! | `TELEMETRY_CHAR` (v1) | 18 bytes | core vitals: HR, SpO2, temp, BP, radiation dose-rate |
//! | `EXT_CHAR` (v2)       | 19 bytes | one *group* (0-5) of 6 extended values (see [`EXT_LAYOUT`]) |
//!
//! Both fit the worst-case 20-byte payload of the default 23-byte ATT MTU.
//!
//! ## Core frame (v1, little-endian, 18 bytes)
//!
//! | offset | type | field | scaling |
//! |-------:|------|-------|---------|
//! | 0  | u8  | frame version (= 1) | |
//! | 1  | u32 | sequence number | |
//! | 5  | u16 | heart rate | bpm x 10 |
//! | 7  | u16 | SpO2 | % x 10 |
//! | 9  | u16 | core temperature | C x 100 |
//! | 11 | u16 | systolic BP | mmHg |
//! | 13 | u16 | diastolic BP | mmHg |
//! | 15 | u16 | cabin radiation dose-rate | uSv/h x 10 |
//! | 17 | u8  | XOR checksum of bytes 0..17 | |
//!
//! ## Extended frame (v2, little-endian, 19 bytes)
//!
//! | offset | type | field |
//! |-------:|------|-------|
//! | 0  | u8      | frame version (= 2) |
//! | 1  | u8      | group index 0..5 |
//! | 2  | u32     | sequence number of the core sample it belongs to |
//! | 6  | 6 x i16 | the group's six values, each scaled per [`EXT_LAYOUT`] |
//! | 18 | u8      | XOR checksum of bytes 0..18 |
//!
//! Groups 0 and 1 (fast signals) are sent every second; groups 2-5
//! (daily / periodic measurements) every few seconds. The receiver keeps the
//! last known value of every field.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// 128-bit custom GATT service UUID: "Astronaut Bio-Chip Telemetry Service".
pub const SERVICE_UUID_U128: u128 = 0x7a3e_0001_5c2d_4f8b_9e61_b0c4_d5a2_e7f3;
/// Core vitals characteristic (Read + Notify).
pub const TELEMETRY_CHAR_UUID_U128: u128 = 0x7a3e_0002_5c2d_4f8b_9e61_b0c4_d5a2_e7f3;
/// Extended vitals characteristic (Notify).
pub const EXT_CHAR_UUID_U128: u128 = 0x7a3e_0003_5c2d_4f8b_9e61_b0c4_d5a2_e7f3;

pub const SERVICE_UUID: Uuid = Uuid::from_u128(SERVICE_UUID_U128);
pub const TELEMETRY_CHAR_UUID: Uuid = Uuid::from_u128(TELEMETRY_CHAR_UUID_U128);
pub const EXT_CHAR_UUID: Uuid = Uuid::from_u128(EXT_CHAR_UUID_U128);

/// Default address for the local loopback (single-PC) transport.
pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:7878";

pub const FRAME_VERSION: u8 = 1;
pub const FRAME_LEN: usize = 18;
pub const EXT_FRAME_VERSION: u8 = 2;
pub const EXT_FRAME_LEN: usize = 19;
pub const EXT_GROUPS: usize = 6;
pub const EXT_GROUP_FIELDS: usize = 6;

/// (field name, wire scale) for every extended value, by group.
/// Wire value = round(value * scale) as i16.
pub const EXT_LAYOUT: [[(&str, f64); EXT_GROUP_FIELDS]; EXT_GROUPS] = [
    // group 0 - fast: cabin environment + autonomic
    [
        ("hrv_rmssd_ms", 10.0),
        ("resp_rate_bpm", 10.0),
        ("cabin_co2_mmhg", 100.0),
        ("cabin_o2_pct", 100.0),
        ("pm25_ugm3", 10.0),
        ("time_scale", 1.0),
    ],
    // group 1 - fast: immune / neuro / microbiology
    [
        ("lymphocytes_per_ul", 1.0),
        ("arterial_pwv_ms", 100.0),
        ("reaction_time_ms", 1.0),
        ("reaction_error_pct", 10.0),
        ("microbial_cfu_m3", 1.0),
        ("cortisol_ugdl", 10.0),
    ],
    // group 2 - periodic: sleep, mood, bone, muscle, fluids
    [
        ("sleep_hours", 100.0),
        ("sleep_efficiency_pct", 10.0),
        ("mood_score", 10.0),
        ("bone_ntx", 10.0),
        ("lean_mass_kg", 100.0),
        ("fluid_index", 1000.0),
    ],
    // group 3 - periodic: inflammation, strength, orthostatic test, clock
    [
        ("crp_mgl", 100.0),
        ("leg_strength_pct", 10.0),
        ("orthostatic_hr_rise", 10.0),
        ("exercise_min", 1.0),
        ("mission_day", 10.0),
        ("humidity_pct", 10.0),
    ],
    // group 4 - periodic: nutrition, mechanical loading, workload
    [
        ("vitamin_d_ngml", 10.0),
        ("calcium_intake_mg", 1.0),
        ("energy_intake_kcal", 1.0),
        ("mech_load_pct", 10.0),
        ("workload_index", 10.0),
        ("work_hours", 10.0),
    ],
    // group 5 - periodic: confinement / social, self-reporting, sodium
    [
        ("social_contact_min", 1.0),
        ("earth_comm_min", 1.0),
        ("selfcheck_pct", 10.0),
        ("report_delay_h", 10.0),
        ("peer_check_pct", 10.0),
        ("sodium_intake_mg", 1.0),
    ],
];

/// Which group carries `name`, if it is an extended field.
pub fn ext_group_of(name: &str) -> Option<usize> {
    EXT_LAYOUT
        .iter()
        .position(|g| g.iter().any(|(n, _)| *n == name))
}

macro_rules! ext_accessors {
    ($($f:ident),* $(,)?) => {
        /// Read an extended field by name (0.0 if unknown).
        pub fn get(&self, name: &str) -> f64 {
            match name { $(stringify!($f) => self.$f,)* _ => 0.0 }
        }
        /// Write an extended field by name (ignored if unknown).
        pub fn set(&mut self, name: &str, v: f64) {
            match name { $(stringify!($f) => self.$f = v,)* _ => {} }
        }
    };
}

/// Extended vitals covering radiation, isolation, altered gravity and the
/// closed-cabin environment. In BLE mode a field is only meaningful once its
/// group bit is set in `groups_mask`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtendedVitals {
    /// Bit g set = group g has been received at least once.
    pub groups_mask: u8,
    // group 0
    pub hrv_rmssd_ms: f64,
    pub resp_rate_bpm: f64,
    pub cabin_co2_mmhg: f64,
    pub cabin_o2_pct: f64,
    pub pm25_ugm3: f64,
    /// Mission seconds per real second (simulator time acceleration).
    pub time_scale: f64,
    // group 1
    pub lymphocytes_per_ul: f64,
    pub arterial_pwv_ms: f64,
    pub reaction_time_ms: f64,
    pub reaction_error_pct: f64,
    pub microbial_cfu_m3: f64,
    pub cortisol_ugdl: f64,
    // group 2
    pub sleep_hours: f64,
    pub sleep_efficiency_pct: f64,
    pub mood_score: f64,
    pub bone_ntx: f64,
    pub lean_mass_kg: f64,
    pub fluid_index: f64,
    // group 3
    pub crp_mgl: f64,
    pub leg_strength_pct: f64,
    pub orthostatic_hr_rise: f64,
    pub exercise_min: f64,
    pub mission_day: f64,
    pub humidity_pct: f64,
    // group 4
    pub vitamin_d_ngml: f64,
    pub calcium_intake_mg: f64,
    pub energy_intake_kcal: f64,
    pub mech_load_pct: f64,
    pub workload_index: f64,
    pub work_hours: f64,
    // group 5
    pub social_contact_min: f64,
    pub earth_comm_min: f64,
    pub selfcheck_pct: f64,
    pub report_delay_h: f64,
    pub peer_check_pct: f64,
    pub sodium_intake_mg: f64,
}

impl ExtendedVitals {
    ext_accessors!(
        hrv_rmssd_ms,
        resp_rate_bpm,
        cabin_co2_mmhg,
        cabin_o2_pct,
        pm25_ugm3,
        time_scale,
        lymphocytes_per_ul,
        arterial_pwv_ms,
        reaction_time_ms,
        reaction_error_pct,
        microbial_cfu_m3,
        cortisol_ugdl,
        sleep_hours,
        sleep_efficiency_pct,
        mood_score,
        bone_ntx,
        lean_mass_kg,
        fluid_index,
        crp_mgl,
        leg_strength_pct,
        orthostatic_hr_rise,
        exercise_min,
        mission_day,
        humidity_pct,
        vitamin_d_ngml,
        calcium_intake_mg,
        energy_intake_kcal,
        mech_load_pct,
        workload_index,
        work_hours,
        social_contact_min,
        earth_comm_min,
        selfcheck_pct,
        report_delay_h,
        peer_check_pct,
        sodium_intake_mg,
    );

    /// The six values of group `g` (already rounded to wire precision on decode).
    pub fn group_values(&self, g: usize) -> [f64; EXT_GROUP_FIELDS] {
        let mut out = [0.0; EXT_GROUP_FIELDS];
        for (i, (name, _)) in EXT_LAYOUT[g].iter().enumerate() {
            out[i] = self.get(name);
        }
        out
    }

    /// Store a received group and mark it as present.
    pub fn apply_group(&mut self, g: usize, vals: [f64; EXT_GROUP_FIELDS]) {
        for (i, (name, _)) in EXT_LAYOUT[g].iter().enumerate() {
            self.set(name, vals[i]);
        }
        self.groups_mask |= 1 << g;
    }

    pub fn has_group(&self, g: usize) -> bool {
        self.groups_mask & (1 << g) != 0
    }

    /// All (name, value) pairs in layout order (used for CSV export).
    pub fn pairs(&self) -> Vec<(&'static str, f64)> {
        EXT_LAYOUT
            .iter()
            .flat_map(|g| g.iter().map(|(n, _)| (*n, self.get(n))))
            .collect()
    }
}

/// One telemetry sample produced by the implant once per second.
///
/// Serialised as JSON for the TCP loopback transport and for the Rust -> Python
/// analysis pipe (one object per line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawTelemetry {
    /// Monotonic sample counter (wraps at u32::MAX). Gaps = lost packets.
    pub seq: u32,
    pub heart_rate_bpm: f64,
    pub spo2_pct: f64,
    pub core_temp_c: f64,
    pub bp_systolic_mmhg: f64,
    pub bp_diastolic_mmhg: f64,
    /// Cabin radiation dose-rate in micro-Sievert per hour (ISS nominal ~ 15-25).
    pub radiation_usv_h: f64,
    /// Unix epoch milliseconds stamped by the *receiver* (Ground Control).
    /// The implant has no reliable wall clock, so it is 0 on the wire.
    #[serde(default)]
    pub received_at_ms: u64,
    /// Extended vitals (see [`ExtendedVitals`]). Absent on old simulators.
    #[serde(default)]
    pub ext: ExtendedVitals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    WrongLength(usize),
    UnsupportedVersion(u8),
    BadChecksum { expected: u8, got: u8 },
    BadGroup(u8),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::WrongLength(n) => write!(
                f,
                "frame has {n} bytes, expected {FRAME_LEN} (core) or {EXT_FRAME_LEN} (extended)"
            ),
            FrameError::UnsupportedVersion(v) => write!(f, "unsupported frame version {v}"),
            FrameError::BadChecksum { expected, got } => {
                write!(
                    f,
                    "checksum mismatch: expected 0x{expected:02x}, got 0x{got:02x}"
                )
            }
            FrameError::BadGroup(g) => write!(f, "invalid extended group {g}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Quantise into a u16 with the given scale, saturating at the edges.
fn q(v: f64, scale: f64) -> u16 {
    let x = (v * scale).round();
    if x.is_nan() {
        0
    } else {
        x.clamp(0.0, u16::MAX as f64) as u16
    }
}

/// Quantise into an i16 with the given scale, saturating at the edges.
fn qi(v: f64, scale: f64) -> i16 {
    let x = (v * scale).round();
    if x.is_nan() {
        0
    } else {
        x.clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }
}

fn xor(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc ^ b)
}

/// Encode a sample into the 18-byte core BLE notification frame.
pub fn encode_frame(t: &RawTelemetry) -> [u8; FRAME_LEN] {
    let mut f = [0u8; FRAME_LEN];
    f[0] = FRAME_VERSION;
    f[1..5].copy_from_slice(&t.seq.to_le_bytes());
    f[5..7].copy_from_slice(&q(t.heart_rate_bpm, 10.0).to_le_bytes());
    f[7..9].copy_from_slice(&q(t.spo2_pct, 10.0).to_le_bytes());
    f[9..11].copy_from_slice(&q(t.core_temp_c, 100.0).to_le_bytes());
    f[11..13].copy_from_slice(&q(t.bp_systolic_mmhg, 1.0).to_le_bytes());
    f[13..15].copy_from_slice(&q(t.bp_diastolic_mmhg, 1.0).to_le_bytes());
    f[15..17].copy_from_slice(&q(t.radiation_usv_h, 10.0).to_le_bytes());
    f[17] = xor(&f[..17]);
    f
}

/// Decode and validate an 18-byte core frame. `ext` is left at its default.
pub fn decode_frame(bytes: &[u8]) -> Result<RawTelemetry, FrameError> {
    if bytes.len() != FRAME_LEN {
        return Err(FrameError::WrongLength(bytes.len()));
    }
    if bytes[0] != FRAME_VERSION {
        return Err(FrameError::UnsupportedVersion(bytes[0]));
    }
    let expected = xor(&bytes[..17]);
    if expected != bytes[17] {
        return Err(FrameError::BadChecksum {
            expected,
            got: bytes[17],
        });
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]) as f64;
    Ok(RawTelemetry {
        seq: u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
        heart_rate_bpm: u16_at(5) / 10.0,
        spo2_pct: u16_at(7) / 10.0,
        core_temp_c: u16_at(9) / 100.0,
        bp_systolic_mmhg: u16_at(11),
        bp_diastolic_mmhg: u16_at(13),
        radiation_usv_h: u16_at(15) / 10.0,
        received_at_ms: 0,
        ext: ExtendedVitals::default(),
    })
}

/// Encode group `group` (0..5) of extended values into a 19-byte frame.
pub fn encode_ext_frame(
    seq: u32,
    group: usize,
    vals: &[f64; EXT_GROUP_FIELDS],
) -> [u8; EXT_FRAME_LEN] {
    let mut f = [0u8; EXT_FRAME_LEN];
    f[0] = EXT_FRAME_VERSION;
    f[1] = group as u8;
    f[2..6].copy_from_slice(&seq.to_le_bytes());
    for (i, v) in vals.iter().enumerate() {
        let scale = EXT_LAYOUT[group][i].1;
        let o = 6 + 2 * i;
        f[o..o + 2].copy_from_slice(&qi(*v, scale).to_le_bytes());
    }
    f[18] = xor(&f[..18]);
    f
}

/// Decoded extended frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtFrame {
    pub seq: u32,
    pub group: usize,
    pub values: [f64; EXT_GROUP_FIELDS],
}

pub fn decode_ext_frame(bytes: &[u8]) -> Result<ExtFrame, FrameError> {
    if bytes.len() != EXT_FRAME_LEN {
        return Err(FrameError::WrongLength(bytes.len()));
    }
    if bytes[0] != EXT_FRAME_VERSION {
        return Err(FrameError::UnsupportedVersion(bytes[0]));
    }
    let expected = xor(&bytes[..18]);
    if expected != bytes[18] {
        return Err(FrameError::BadChecksum {
            expected,
            got: bytes[18],
        });
    }
    let group = bytes[1] as usize;
    if group >= EXT_GROUPS {
        return Err(FrameError::BadGroup(bytes[1]));
    }
    let mut values = [0.0; EXT_GROUP_FIELDS];
    for (i, v) in values.iter_mut().enumerate() {
        let o = 6 + 2 * i;
        *v = i16::from_le_bytes([bytes[o], bytes[o + 1]]) as f64 / EXT_LAYOUT[group][i].1;
    }
    Ok(ExtFrame {
        seq: u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
        group,
        values,
    })
}

/// Current Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RawTelemetry {
        RawTelemetry {
            seq: 4242,
            heart_rate_bpm: 72.3,
            spo2_pct: 98.1,
            core_temp_c: 36.92,
            bp_systolic_mmhg: 118.0,
            bp_diastolic_mmhg: 76.0,
            radiation_usv_h: 18.4,
            received_at_ms: 0,
            ext: ExtendedVitals::default(),
        }
    }

    #[test]
    fn frames_fit_default_mtu() {
        assert!(
            FRAME_LEN <= 20 && EXT_FRAME_LEN <= 20,
            "must fit a 23-byte ATT MTU"
        );
    }

    #[test]
    fn roundtrip() {
        let s = sample();
        let d = decode_frame(&encode_frame(&s)).unwrap();
        assert_eq!(d.seq, s.seq);
        assert!((d.heart_rate_bpm - s.heart_rate_bpm).abs() < 0.051);
        assert!((d.spo2_pct - s.spo2_pct).abs() < 0.051);
        assert!((d.core_temp_c - s.core_temp_c).abs() < 0.0051);
        assert_eq!(d.bp_systolic_mmhg, 118.0);
        assert_eq!(d.bp_diastolic_mmhg, 76.0);
        assert!((d.radiation_usv_h - s.radiation_usv_h).abs() < 0.051);
    }

    #[test]
    fn detects_corruption() {
        let mut f = encode_frame(&sample());
        f[6] ^= 0x40;
        assert!(matches!(
            decode_frame(&f),
            Err(FrameError::BadChecksum { .. })
        ));
        assert!(matches!(
            decode_frame(&f[..10]),
            Err(FrameError::WrongLength(10))
        ));
    }

    #[test]
    fn saturates_out_of_range() {
        let mut s = sample();
        s.radiation_usv_h = 1.0e9;
        s.heart_rate_bpm = -5.0;
        let d = decode_frame(&encode_frame(&s)).unwrap();
        assert_eq!(d.radiation_usv_h, 6553.5);
        assert_eq!(d.heart_rate_bpm, 0.0);
    }

    #[test]
    fn every_ext_field_is_accessible_and_layout_is_unique() {
        let mut e = ExtendedVitals::default();
        let mut n = 0.0;
        for g in EXT_LAYOUT.iter() {
            for (name, _) in g.iter() {
                n += 1.0;
                e.set(name, n);
                assert_eq!(e.get(name), n, "field {name} not wired in ext_accessors!");
            }
        }
        assert_eq!(n as usize, EXT_GROUPS * EXT_GROUP_FIELDS);
        assert_eq!(ext_group_of("lean_mass_kg"), Some(2));
        assert_eq!(ext_group_of("nope"), None);
    }

    #[test]
    fn ext_roundtrip_all_groups() {
        let mut src = ExtendedVitals::default();
        src.set("hrv_rmssd_ms", 44.7);
        src.set("cabin_co2_mmhg", 2.23);
        src.set("time_scale", 3600.0);
        src.set("lymphocytes_per_ul", 2143.0);
        src.set("arterial_pwv_ms", 7.04);
        src.set("sleep_hours", 6.85);
        src.set("lean_mass_kg", 57.62);
        src.set("fluid_index", 1.083);
        src.set("crp_mgl", 1.07);
        src.set("mission_day", 123.4);
        src.set("vitamin_d_ngml", 31.7);
        src.set("calcium_intake_mg", 1010.0);
        src.set("workload_index", 58.3);
        src.set("selfcheck_pct", 94.4);
        src.set("sodium_intake_mg", 3180.0);
        let mut dst = ExtendedVitals::default();
        for g in 0..EXT_GROUPS {
            let frame = encode_ext_frame(77, g, &src.group_values(g));
            let d = decode_ext_frame(&frame).unwrap();
            assert_eq!((d.seq, d.group), (77, g));
            dst.apply_group(g, d.values);
        }
        assert_eq!(dst.groups_mask, 0b111111);
        assert!((dst.vitamin_d_ngml - 31.7).abs() < 0.051);
        assert_eq!(dst.calcium_intake_mg, 1010.0);
        assert!((dst.workload_index - 58.3).abs() < 0.051);
        assert!((dst.selfcheck_pct - 94.4).abs() < 0.051);
        assert_eq!(dst.sodium_intake_mg, 3180.0);
        assert!((dst.hrv_rmssd_ms - 44.7).abs() < 0.051);
        assert!((dst.cabin_co2_mmhg - 2.23).abs() < 0.0051);
        assert_eq!(dst.time_scale, 3600.0);
        assert_eq!(dst.lymphocytes_per_ul, 2143.0);
        assert!((dst.lean_mass_kg - 57.62).abs() < 0.0051);
        assert!((dst.fluid_index - 1.083).abs() < 0.00051);
        assert!((dst.mission_day - 123.4).abs() < 0.051);
    }

    #[test]
    fn ext_frame_rejects_bad_input() {
        let mut f = encode_ext_frame(1, 0, &[0.0; 6]);
        f[8] ^= 1;
        assert!(matches!(
            decode_ext_frame(&f),
            Err(FrameError::BadChecksum { .. })
        ));
        let mut g = encode_ext_frame(1, 0, &[0.0; 6]);
        g[1] = 9;
        g[18] = xor(&g[..18]);
        assert!(matches!(decode_ext_frame(&g), Err(FrameError::BadGroup(9))));
        assert!(matches!(
            decode_ext_frame(&g[..5]),
            Err(FrameError::WrongLength(5))
        ));
    }
}
