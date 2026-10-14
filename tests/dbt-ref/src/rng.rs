//! Deterministic randomness (testing.md section 4.5).
//!
//! The per-vector seed is `hash(master seed, class name, index)`, so adding a class or changing
//! the number of vectors of one class never shifts the vectors of the others.

/// Default master seed of the checked-in golden files (`make dbt-ref-record`).
pub const DEFAULT_SEED: u64 = 0x5249_5453_5530_3031; // "RITSU001"

/// splitmix64 generator. Also the specification of the data window contents
/// ([`fill_window`]); the C runner implements the same function.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform value in `0..n` (`n > 0`); the tiny modulo bias is irrelevant for tests.
    pub fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        self.next_u64() % n
    }

    /// Uniform value in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// `true` with probability `percent` / 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// A random element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// FNV-1a, 64 bit. Used for class-name hashing and content fingerprints (not cryptographic).
#[derive(Clone, Copy, Debug)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    /// Feeds bytes.
    pub fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// FNV-1a of a byte string.
#[must_use]
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = Fnv::default();
    h.update(bytes);
    h.0
}

/// Seed of vector `index` of class `class`.
#[must_use]
pub fn vector_seed(master: u64, class: &str, index: u64) -> u64 {
    let mut r = Rng(master ^ fnv(class.as_bytes()).rotate_left(17));
    r.0 = r.next_u64() ^ index.wrapping_mul(0xd6e8_feb8_6659_fd93);
    r.next_u64()
}

/// Contents of the data window: little-endian splitmix64 words from `mem_seed`.
#[must_use]
pub fn fill_window(len: usize, mem_seed: u64) -> Vec<u8> {
    let mut r = Rng(mem_seed);
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&r.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Parses `123` or `0x7b` (underscores allowed).
#[must_use]
pub fn parse_seed(s: &str) -> Option<u64> {
    let s: String = s.trim().chars().filter(|c| *c != '_').collect();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// The master seed: `RITSU_TEST_SEED` if set, otherwise `default`. Prints the value so that a
/// failing run can be repeated.
///
/// # Panics
///
/// If `RITSU_TEST_SEED` is set but not a number (a typo must not silently fall back).
#[must_use]
pub fn seed_from_env(default: u64) -> u64 {
    match std::env::var("RITSU_TEST_SEED") {
        Ok(v) => {
            let seed = parse_seed(&v).unwrap_or_else(|| {
                panic!("RITSU_TEST_SEED={v:?} is not a number (decimal or 0x..)")
            });
            eprintln!("RITSU_TEST_SEED={seed:#x} (from environment)");
            seed
        }
        Err(_) => {
            eprintln!("RITSU_TEST_SEED={default:#x} (default; set RITSU_TEST_SEED to override)");
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_reference_values() {
        // First outputs of splitmix64 with state 0 (published reference values).
        let mut r = Rng(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next_u64(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(r.next_u64(), 0x06c4_5d18_8009_454f);
    }

    #[test]
    fn fnv_reference_values() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn seeds_parse() {
        assert_eq!(parse_seed("42"), Some(42));
        assert_eq!(parse_seed("0x2a"), Some(42));
        assert_eq!(parse_seed(" 0X2A "), Some(42));
        assert_eq!(parse_seed("0x_dead_beef"), Some(0xdead_beef));
        assert_eq!(parse_seed("nope"), None);
    }

    #[test]
    fn vector_seeds_are_independent_per_class_and_index() {
        let a = vector_seed(1, "movz", 0);
        assert_eq!(a, vector_seed(1, "movz", 0));
        assert_ne!(a, vector_seed(1, "movz", 1));
        assert_ne!(a, vector_seed(1, "movn", 0));
        assert_ne!(a, vector_seed(2, "movz", 0));
    }

    #[test]
    fn window_is_prefix_stable() {
        let long = fill_window(64, 7);
        assert_eq!(&long[..13], &fill_window(13, 7)[..]);
        assert_eq!(fill_window(0, 7).len(), 0);
    }
}
