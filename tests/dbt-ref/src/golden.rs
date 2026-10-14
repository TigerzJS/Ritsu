//! Golden files: recorded reference results, stored as a delta against the initial state.
//!
//! The initial states and instruction words are *not* stored: they are regenerated from the seed
//! (the generator is deterministic) and protected by a per-class fingerprint. A golden file
//! therefore only holds what the reference produced, in a compact form (about 15 bytes per vector).
//!
//! ```text
//! file    = "RITSUDG1" u32 version(=1) u32 meta_len meta[meta_len]
//!           u64 seed  u32 gen_version  u64 code_base  u64 mem_base  u32 mem_len  u32 units
//!           u32 nclasses  { u16 id  u32 count  u64 vector_fingerprint  u8 name_len name }*
//!           record*
//! meta    = UTF-8 lines "key=value\n" (oracle, cpu, kernel, ...)
//! record  = u8 flags  varint zigzag(pc - code_base)  u8 nchanged { u8 reg  varint value }*
//!           [u8 nzcv_nibble] [mem_len bytes]
//! flags   = signal (bits 0-4) | 0x20 window changed | 0x40 NZCV changed
//! reg     = 0..=30 for X0-X30, 31 for SP
//! varint  = LEB128, little endian base 128
//! ```

use crate::format::{Config, Outcome, State, Vector, bad, read_u16, read_u32, read_u64};
use std::io::{self, Read, Write};

/// Magic of a golden file.
pub const MAGIC: &[u8; 8] = b"RITSUDG1";

/// Per-class entry of the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassEntry {
    /// Class id.
    pub id: u16,
    /// Class name (diagnostics; the id is authoritative).
    pub name: String,
    /// Number of vectors.
    pub count: u32,
    /// Fingerprint of the class's vector records.
    pub fingerprint: u64,
}

/// Parsed header of a golden file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// `key=value` metadata (oracle, cpu, kernel, ...), in file order.
    pub meta: Vec<(String, String)>,
    /// Generation parameters.
    pub cfg: Config,
    /// Classes in record order.
    pub classes: Vec<ClassEntry>,
}

impl Header {
    /// Value of a metadata key.
    #[must_use]
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Total number of vectors.
    #[must_use]
    pub fn total(&self) -> usize {
        self.classes.iter().map(|c| c.count as usize).sum()
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(r: &mut impl Read) -> io::Result<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        if shift == 63 && b[0] > 1 {
            return Err(bad("varint overflows 64 bits"));
        }
        v |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(bad("varint too long"))
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Writes the header.
pub fn write_header(w: &mut impl Write, h: &Header) -> io::Result<()> {
    let mut meta = String::new();
    for (k, v) in &h.meta {
        if k.contains(['=', '\n']) || v.contains('\n') {
            return Err(bad(
                "metadata keys must not contain '=' or newlines, values no newlines",
            ));
        }
        meta.push_str(&format!("{k}={v}\n"));
    }
    w.write_all(MAGIC)?;
    w.write_all(&1u32.to_le_bytes())?;
    w.write_all(&(meta.len() as u32).to_le_bytes())?;
    w.write_all(meta.as_bytes())?;
    w.write_all(&h.cfg.seed.to_le_bytes())?;
    w.write_all(&h.cfg.gen_version.to_le_bytes())?;
    w.write_all(&h.cfg.code_base.to_le_bytes())?;
    w.write_all(&h.cfg.mem_base.to_le_bytes())?;
    w.write_all(&h.cfg.mem_len.to_le_bytes())?;
    w.write_all(&h.cfg.units.to_le_bytes())?;
    w.write_all(&(h.classes.len() as u32).to_le_bytes())?;
    for c in &h.classes {
        w.write_all(&c.id.to_le_bytes())?;
        w.write_all(&c.count.to_le_bytes())?;
        w.write_all(&c.fingerprint.to_le_bytes())?;
        let name = c.name.as_bytes();
        w.write_all(&[u8::try_from(name.len()).map_err(|_| bad("class name too long"))?])?;
        w.write_all(name)?;
    }
    Ok(())
}

/// Reads the header.
pub fn read_header(r: &mut impl Read) -> io::Result<Header> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not a golden file (bad magic)"));
    }
    if read_u32(r)? != 1 {
        return Err(bad("unsupported golden file version"));
    }
    let meta_len = read_u32(r)? as usize;
    if meta_len > 1 << 20 {
        return Err(bad("metadata too large"));
    }
    let mut meta_bytes = vec![0u8; meta_len];
    r.read_exact(&mut meta_bytes)?;
    let meta_text = String::from_utf8(meta_bytes).map_err(|_| bad("metadata is not UTF-8"))?;
    let meta = meta_text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let seed = read_u64(r)?;
    let gen_version = read_u32(r)?;
    let code_base = read_u64(r)?;
    let mem_base = read_u64(r)?;
    let mem_len = read_u32(r)?;
    let units = read_u32(r)?;
    let nclasses = read_u32(r)?;
    let mut classes = Vec::new();
    for _ in 0..nclasses {
        let id = read_u16(r)?;
        let count = read_u32(r)?;
        let fingerprint = read_u64(r)?;
        let mut len = [0u8; 1];
        r.read_exact(&mut len)?;
        let mut name = vec![0u8; usize::from(len[0])];
        r.read_exact(&mut name)?;
        classes.push(ClassEntry {
            id,
            name: String::from_utf8(name).map_err(|_| bad("class name is not UTF-8"))?,
            count,
            fingerprint,
        });
    }
    Ok(Header {
        meta,
        cfg: Config {
            seed,
            gen_version,
            code_base,
            mem_base,
            mem_len,
            units,
        },
        classes,
    })
}

/// Appends the record of `out` (delta against `v`'s initial state).
///
/// # Errors
///
/// A signal that cannot occur in a recording (T0-internal codes).
pub fn encode_record(buf: &mut Vec<u8>, cfg: &Config, v: &Vector, out: &Outcome) -> io::Result<()> {
    if out.signal >= 32 {
        return Err(bad("signal >= 32 cannot be recorded"));
    }
    let mut flags = out.signal as u8;
    if out.mem.is_some() {
        flags |= 0x20;
    }
    let nzcv_changed = out.state.nzcv != v.nzcv;
    if nzcv_changed {
        flags |= 0x40;
    }
    buf.push(flags);
    put_varint(buf, zigzag(out.state.pc.wrapping_sub(cfg.code_base) as i64));
    let mut changed: Vec<(u8, u64)> = Vec::new();
    for i in 0..31 {
        if out.state.x[i] != v.x[i] {
            changed.push((i as u8, out.state.x[i]));
        }
    }
    if out.state.sp != v.sp {
        changed.push((31, out.state.sp));
    }
    buf.push(changed.len() as u8);
    for (r, val) in changed {
        buf.push(r);
        put_varint(buf, val);
    }
    if nzcv_changed {
        buf.push((out.state.nzcv >> 28) as u8 & 0xf);
    }
    if let Some(m) = &out.mem {
        buf.extend_from_slice(m);
    }
    Ok(())
}

/// Reads one record; `v` is the vector it belongs to.
pub fn decode_record(r: &mut impl Read, cfg: &Config, v: &Vector) -> io::Result<Outcome> {
    let mut f = [0u8; 1];
    r.read_exact(&mut f)?;
    let flags = f[0];
    if flags & 0x80 != 0 {
        return Err(bad("reserved flag bit set"));
    }
    let pc = cfg.code_base.wrapping_add(unzigzag(get_varint(r)?) as u64);
    let mut n = [0u8; 1];
    r.read_exact(&mut n)?;
    let mut state = State {
        x: v.x,
        sp: v.sp,
        nzcv: v.nzcv,
        pc,
    };
    for _ in 0..n[0] {
        let mut reg = [0u8; 1];
        r.read_exact(&mut reg)?;
        let val = get_varint(r)?;
        match reg[0] {
            0..=30 => state.x[usize::from(reg[0])] = val,
            31 => state.sp = val,
            _ => return Err(bad("register index out of range")),
        }
    }
    if flags & 0x40 != 0 {
        let mut nz = [0u8; 1];
        r.read_exact(&mut nz)?;
        state.nzcv = u64::from(nz[0] & 0xf) << 28;
    }
    let mem = if flags & 0x20 != 0 {
        let mut m = vec![0u8; cfg.mem_len as usize];
        r.read_exact(&mut m)?;
        Some(m)
    } else {
        None
    };
    Ok(Outcome {
        signal: u16::from(flags & 0x1f),
        si_code: 0,
        state,
        mem,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> Vector {
        let mut x = [0u64; 31];
        for (i, v) in x.iter_mut().enumerate() {
            *v = 0x1111 * (i as u64 + 1);
        }
        Vector {
            class_id: 1,
            mem_seed: 5,
            x,
            sp: 0x1000,
            nzcv: 0xa000_0000,
            words: vec![0xd503_201f],
        }
    }

    fn cfg() -> Config {
        crate::generator::default_config(1)
    }

    #[test]
    fn varint_and_zigzag_roundtrip() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            1 << 35,
            u64::MAX,
            u64::MAX - 1,
            1 << 63,
        ] {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            assert_eq!(get_varint(&mut b.as_slice()).unwrap(), v);
        }
        for v in [0i64, 1, -1, 4, -4, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
        assert!(get_varint(&mut &[0xffu8; 11][..]).is_err());
        assert!(
            get_varint(&mut &[0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02][..])
                .is_err()
        );
    }

    #[test]
    fn record_roundtrip_unchanged_and_changed() {
        let cfg = cfg();
        let v = vector();
        let unchanged = Outcome {
            signal: 5,
            si_code: 0,
            state: State {
                x: v.x,
                sp: v.sp,
                nzcv: v.nzcv,
                pc: cfg.code_base + 4,
            },
            mem: None,
        };
        let mut buf = Vec::new();
        encode_record(&mut buf, &cfg, &v, &unchanged).unwrap();
        assert_eq!(buf.len(), 1 + 1 + 1, "an unchanged state costs three bytes");
        assert_eq!(
            decode_record(&mut buf.as_slice(), &cfg, &v).unwrap(),
            unchanged
        );

        let mut changed = unchanged.clone();
        changed.state.x[3] = u64::MAX;
        changed.state.x[30] = 0;
        changed.state.sp = 7;
        changed.state.nzcv = 0x4000_0000;
        changed.state.pc = cfg.code_base.wrapping_sub(4);
        changed.signal = 4;
        changed.mem = Some(vec![0xab; cfg.mem_len as usize]);
        let mut buf = Vec::new();
        encode_record(&mut buf, &cfg, &v, &changed).unwrap();
        assert_eq!(
            decode_record(&mut buf.as_slice(), &cfg, &v).unwrap(),
            changed
        );
    }

    #[test]
    fn t0_only_signals_are_rejected() {
        let cfg = cfg();
        let v = vector();
        let o = Outcome {
            signal: crate::format::sig::T0_UNIMPLEMENTED,
            si_code: 0,
            state: State {
                x: v.x,
                sp: v.sp,
                nzcv: v.nzcv,
                pc: 0,
            },
            mem: None,
        };
        assert!(encode_record(&mut Vec::new(), &cfg, &v, &o).is_err());
    }

    #[test]
    fn header_roundtrip() {
        let h = Header {
            meta: vec![
                ("oracle".into(), "qemu".into()),
                ("cpu".into(), "a = b".into()),
            ],
            cfg: cfg(),
            classes: vec![ClassEntry {
                id: 4,
                name: "movz".into(),
                count: 18,
                fingerprint: 0xfeed,
            }],
        };
        let mut buf = Vec::new();
        write_header(&mut buf, &h).unwrap();
        let back = read_header(&mut buf.as_slice()).unwrap();
        assert_eq!(back, h);
        assert_eq!(back.meta("cpu"), Some("a = b"));
        assert_eq!(back.total(), 18);
    }
}
