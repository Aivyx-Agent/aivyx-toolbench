//! Small dependency-free helpers: UTC timestamps, a deterministic PRNG,
//! run ids, sha256, build info.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// (year, month, day, hour, minute, second) for Unix seconds, UTC.
/// Howard Hinnant's civil-from-days.
pub fn utc_parts(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

/// (`2026-10-19T08:15:02Z`, `20261019T081502Z`).
pub fn utc_strings(secs: u64) -> (String, String) {
    let (y, mo, d, h, mi, s) = utc_parts(secs);
    (
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"),
        format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z"),
    )
}

pub fn utc_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_strings(secs)
}

/// splitmix64: tiny, deterministic, good enough for resampling.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform-ish index in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// `YYYYMMDDTHHMMSSZ-xxxx`.
pub fn new_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seed = now.as_nanos() as u64 ^ (u64::from(std::process::id()) << 32);
    let hex = SplitMix64::new(seed).next_u64() & 0xffff;
    format!("{}-{hex:04x}", utc_strings(now.as_secs()).1)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: env!("TOOLBENCH_COMMIT").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_conversion_matches_known_instants() {
        assert_eq!(utc_strings(0).0, "1970-01-01T00:00:00Z");
        assert_eq!(utc_strings(951_782_400).0, "2000-02-29T00:00:00Z");
        assert_eq!(utc_strings(1_792_313_735).1, "20261018T085535Z");
    }

    #[test]
    fn splitmix_is_deterministic() {
        let a: Vec<u64> = (0..3)
            .map({
                let mut r = SplitMix64::new(7);
                move |_| r.next_u64()
            })
            .collect();
        let b: Vec<u64> = (0..3)
            .map({
                let mut r = SplitMix64::new(7);
                move |_| r.next_u64()
            })
            .collect();
        assert_eq!(a, b);
        assert_ne!(a[0], a[1]);
    }

    #[test]
    fn run_ids_have_the_documented_shape() {
        let id = new_run_id();
        let re = regex::Regex::new(r"^\d{8}T\d{6}Z-[0-9a-f]{4}$").unwrap();
        assert!(re.is_match(&id), "{id}");
    }

    #[test]
    fn sha256_of_empty_is_the_known_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
