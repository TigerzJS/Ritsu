//! Building and loading complete golden files (header + records) and the checks around them.

use crate::format::{Outcome, Vector};
use crate::generator::{self, GEN_VERSION};
use crate::golden::{self, ClassEntry, Header};
use crate::set::VectorSet;
use std::io::Cursor;

/// A golden file together with the regenerated vectors it refers to.
#[derive(Debug)]
pub struct Loaded {
    /// Parsed header.
    pub header: Header,
    /// Regenerated vectors (verified against the fingerprints in the header).
    pub set: VectorSet,
    /// Recorded outcomes, one per vector.
    pub outcomes: Vec<Outcome>,
}

impl Loaded {
    /// The `oracle` metadata (`hardware` or `qemu`).
    #[must_use]
    pub fn oracle(&self) -> &str {
        self.header.meta("oracle").unwrap_or("unknown")
    }

    /// Names of generator classes that this file does not cover.
    #[must_use]
    pub fn uncovered_classes(&self) -> Vec<&'static str> {
        generator::CLASSES
            .iter()
            .filter(|c| !self.header.classes.iter().any(|e| e.id == c.id))
            .map(|c| c.name)
            .collect()
    }
}

/// Serialises `outcomes` (one per vector of `set`) as a golden file with the given metadata.
///
/// # Errors
///
/// Outcomes that cannot be recorded (T0-internal signals), or a length mismatch.
pub fn pack(
    set: &VectorSet,
    outcomes: &[Outcome],
    meta: Vec<(String, String)>,
) -> std::io::Result<Vec<u8>> {
    if outcomes.len() != set.vectors.len() {
        return Err(crate::format::bad(
            "outcome count does not match the vector count",
        ));
    }
    let classes = set
        .spans
        .iter()
        .map(|s| ClassEntry {
            id: s.class.id,
            name: s.class.name.to_string(),
            count: s.count as u32,
            fingerprint: set.span_hash(s),
        })
        .collect();
    let header = Header {
        meta,
        cfg: set.cfg,
        classes,
    };
    let mut buf = Vec::new();
    golden::write_header(&mut buf, &header)?;
    for (v, o) in set.vectors.iter().zip(outcomes) {
        golden::encode_record(&mut buf, &set.cfg, v, o)?;
    }
    Ok(buf)
}

/// Parses a golden file and regenerates its vectors.
///
/// # Errors
///
/// A damaged file, a generator version or class that no longer matches (the golden file must be
/// re-recorded; it is never compared against different vectors), or trailing bytes.
pub fn unpack(bytes: &[u8]) -> Result<Loaded, String> {
    let mut cur = Cursor::new(bytes);
    let header = golden::read_header(&mut cur).map_err(|e| format!("golden header: {e}"))?;
    if header.cfg.gen_version != GEN_VERSION {
        return Err(format!(
            "golden file was recorded with generator version {}, this is version {GEN_VERSION}: \
             re-record it (make dbt-ref-record, then make dbt-ref-update)",
            header.cfg.gen_version
        ));
    }
    let mut names = Vec::new();
    for e in &header.classes {
        let class = generator::class_by_id(e.id)
            .ok_or_else(|| format!("golden file refers to unknown class id {}", e.id))?;
        if class.name != e.name {
            return Err(format!(
                "class id {} is {:?} in the golden file but {:?} in the generator",
                e.id, e.name, class.name
            ));
        }
        names.push(e.name.clone());
    }
    let set = VectorSet::generate(header.cfg, Some(&names))?;
    for (e, span) in header.classes.iter().zip(&set.spans) {
        if span.count != e.count as usize {
            return Err(format!(
                "class {}: golden has {} vectors, the generator now makes {}",
                e.name, e.count, span.count
            ));
        }
        let fp = set.span_hash(span);
        if fp != e.fingerprint {
            return Err(format!(
                "class {}: generated vectors differ from the recorded ones (fingerprint {fp:#x} vs {:#x}); \
                 the generator changed without a GEN_VERSION bump, or the golden file is damaged",
                e.name, e.fingerprint
            ));
        }
    }
    let mut outcomes = Vec::with_capacity(set.vectors.len());
    for v in &set.vectors {
        outcomes.push(
            golden::decode_record(&mut cur, &set.cfg, v)
                .map_err(|e| format!("golden record: {e}"))?,
        );
    }
    if (cur.position() as usize) != bytes.len() {
        return Err("trailing bytes after the last golden record".into());
    }
    Ok(Loaded {
        header,
        set,
        outcomes,
    })
}

/// Regenerates the vector set described by a vector file that was produced by `gen`: class runs
/// in the file give the class list, the header gives seed and units.
///
/// # Errors
///
/// The file does not equal what the generator produces for its own header.
pub fn set_for_vectors(
    cfg: crate::format::Config,
    vectors: &[Vector],
) -> Result<VectorSet, String> {
    let mut names: Vec<String> = Vec::new();
    let mut last = None;
    for v in vectors {
        if last != Some(v.class_id) {
            let c = generator::class_by_id(v.class_id)
                .ok_or_else(|| format!("unknown class id {}", v.class_id))?;
            names.push(c.name.to_string());
            last = Some(v.class_id);
        }
    }
    let set = VectorSet::generate(cfg, Some(&names))?;
    if set.vectors != vectors {
        return Err(
            "the vector file is not what this generator produces for its header (seed, units, gen_version, \
             classes): refusing to continue"
                .into(),
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Config;

    fn small_cfg() -> Config {
        Config {
            units: 2,
            ..generator::default_config(0xabc)
        }
    }

    /// A fake "reference": T0 itself. Enough to test the container logic.
    fn outcomes(set: &VectorSet) -> Vec<Outcome> {
        crate::t0::run_all(&set.cfg, &set.vectors)
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let names: Vec<String> = [
            "handmade", "movz", "movk", "add_imm", "b", "br", "cbz", "b_cond", "nop", "udf",
            "mix_flow",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let set = VectorSet::generate(small_cfg(), Some(&names)).unwrap();
        let outs = outcomes(&set);
        assert!(
            outs.iter().all(|o| o.signal < 32),
            "T0 must handle these classes"
        );
        let bytes = pack(&set, &outs, vec![("oracle".into(), "test".into())]).unwrap();
        let back = unpack(&bytes).unwrap();
        assert_eq!(back.outcomes, outs);
        assert_eq!(back.oracle(), "test");
        assert!(back.uncovered_classes().contains(&"sub_imm"));
        assert!(!back.uncovered_classes().contains(&"movz"));
    }

    #[test]
    fn unpack_rejects_damage_and_drift() {
        let names = vec!["movz".to_string(), "nop".to_string()];
        let set = VectorSet::generate(small_cfg(), Some(&names)).unwrap();
        let outs = outcomes(&set);
        assert!(outs.iter().all(|o| o.signal < 32));
        let bytes = pack(&set, &outs, vec![]).unwrap();
        assert!(unpack(&bytes).is_ok());
        // Trailing byte.
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(unpack(&longer).unwrap_err().contains("trailing"));
        // Truncated.
        assert!(unpack(&bytes[..bytes.len() - 1]).is_err());
        // Generator version drift.
        let mut other = set.cfg;
        other.gen_version += 1;
        let drift = VectorSet {
            cfg: other,
            spans: set.spans.clone(),
            vectors: set.vectors.clone(),
        };
        let bytes = pack(&drift, &outs, vec![]).unwrap();
        assert!(unpack(&bytes).unwrap_err().contains("generator version"));
        // Changed seed with the same version: the fingerprints catch it.
        let mut other = set.cfg;
        other.seed ^= 1;
        let drift = VectorSet {
            cfg: other,
            spans: set.spans.clone(),
            vectors: set.vectors.clone(),
        };
        let bytes = pack(&drift, &outs, vec![]).unwrap();
        assert!(unpack(&bytes).unwrap_err().contains("fingerprint"));
    }

    #[test]
    fn vector_file_must_match_the_generator() {
        let set =
            VectorSet::generate(small_cfg(), Some(&["b".to_string(), "movk".to_string()])).unwrap();
        assert!(set_for_vectors(set.cfg, &set.vectors).is_ok());
        let mut tampered = set.vectors.clone();
        tampered[3].x[0] ^= 1;
        assert!(set_for_vectors(set.cfg, &tampered).is_err());
    }
}
