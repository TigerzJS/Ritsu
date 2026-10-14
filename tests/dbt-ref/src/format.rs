//! Vector and raw-result file formats (all little endian). Documented in `tests/dbt-ref/README.md`;
//! the C runner (`runner/ritsu-ref-runner.c`) implements the same layouts.

use std::io::{self, Read, Write};

/// Magic of a vector file.
pub const VEC_MAGIC: &[u8; 8] = b"RITSUDV1";
/// Magic of a raw result file.
pub const RES_MAGIC: &[u8; 8] = b"RITSURR1";
/// Bytes of the runner's prologue in front of `code_base` (fixed by the runner).
pub const PROLOGUE_BYTES: u64 = 256;
/// Number of BRK sentinel words after the test words (fixed by the runner).
pub const PAD_WORDS: u64 = 8;
/// Maximum number of test words per vector.
pub const MAX_WORDS: usize = 256;
/// Maximum data window length.
pub const MAX_MEM: usize = 4096;
/// Size of the mapped code and data regions (the same on the reference and in T0).
pub const REGION_SIZE: u64 = 0x1_0000;
/// The BRK sentinel word (`BRK #0xf001`).
pub const SENTINEL: u32 = 0xd43e_0020;

/// Signal numbers as recorded (Linux/aarch64 values).
pub mod sig {
    /// `SIGILL`
    pub const ILL: u16 = 4;
    /// `SIGTRAP`: the end-of-block sentinel or a `BRK` in the test words.
    pub const TRAP: u16 = 5;
    /// `SIGBUS`
    pub const BUS: u16 = 7;
    /// `SIGSEGV`
    pub const SEGV: u16 = 11;
    /// T0 only: the instruction is valid but not implemented in T0 (never produced by a
    /// reference; always a finding).
    pub const T0_UNIMPLEMENTED: u16 = 0x100;
    /// T0 only: the step limit was exceeded.
    pub const T0_STEP_LIMIT: u16 = 0x101;
}

/// Parameters shared by all vectors of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Master seed (informational in vector files).
    pub seed: u64,
    /// Generator version, bumped on every change that alters generated vectors.
    pub gen_version: u32,
    /// Address of the first test word.
    pub code_base: u64,
    /// Address of the data window.
    pub mem_base: u64,
    /// Data window length in bytes.
    pub mem_len: u32,
    /// `--units` the random classes were generated with (vectors per class = weight * units).
    pub units: u32,
}

impl Config {
    /// First address after the test words of a vector with `nwords` words.
    #[must_use]
    pub fn code_end(&self, nwords: usize) -> u64 {
        self.code_base + 4 * nwords as u64
    }
}

/// Machine state compared between oracles (`PC` included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// `X0`-`X30`.
    pub x: [u64; 31],
    /// `SP` (`SP_EL0`).
    pub sp: u64,
    /// `PSTATE` bits 31:28 (`NZCV`) in place; all other bits are 0.
    pub nzcv: u64,
    /// Program counter.
    pub pc: u64,
}

/// One test vector: initial state, instruction words, memory seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// Class id from the class table (stable, never reused).
    pub class_id: u16,
    /// Seed of the data window contents ([`crate::rng::fill_window`]).
    pub mem_seed: u64,
    /// Initial `X0`-`X30`.
    pub x: [u64; 31],
    /// Initial `SP`.
    pub sp: u64,
    /// Initial `NZCV` (bits 31:28).
    pub nzcv: u64,
    /// Instruction words, placed at `code_base`.
    pub words: Vec<u32>,
}

impl Vector {
    /// Serialises the record (the fixed part is 280 bytes).
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&(self.words.len() as u16).to_le_bytes())?;
        w.write_all(&self.class_id.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
        w.write_all(&self.mem_seed.to_le_bytes())?;
        for v in self.x {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&self.sp.to_le_bytes())?;
        w.write_all(&self.nzcv.to_le_bytes())?;
        for word in &self.words {
            w.write_all(&word.to_le_bytes())?;
        }
        Ok(())
    }

    /// Reads one record.
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let nwords = usize::from(read_u16(r)?);
        let class_id = read_u16(r)?;
        let _reserved = read_u32(r)?;
        let mem_seed = read_u64(r)?;
        let mut x = [0u64; 31];
        for v in &mut x {
            *v = read_u64(r)?;
        }
        let sp = read_u64(r)?;
        let nzcv = read_u64(r)?;
        if nwords == 0 || nwords > MAX_WORDS {
            return Err(bad("word count out of range"));
        }
        let mut words = Vec::with_capacity(nwords);
        for _ in 0..nwords {
            words.push(read_u32(r)?);
        }
        Ok(Self {
            class_id,
            mem_seed,
            x,
            sp,
            nzcv,
            words,
        })
    }

    /// The record as bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(280 + 4 * self.words.len());
        // Writing to a Vec cannot fail.
        let _ = self.write(&mut v);
        v
    }
}

/// Writes the 64-byte vector file header.
pub fn write_vec_header(w: &mut impl Write, cfg: &Config, count: u32) -> io::Result<()> {
    w.write_all(VEC_MAGIC)?;
    w.write_all(&1u32.to_le_bytes())?; // version
    w.write_all(&0u32.to_le_bytes())?; // flags
    w.write_all(&cfg.seed.to_le_bytes())?;
    w.write_all(&cfg.code_base.to_le_bytes())?;
    w.write_all(&cfg.mem_base.to_le_bytes())?;
    w.write_all(&count.to_le_bytes())?;
    w.write_all(&cfg.mem_len.to_le_bytes())?;
    w.write_all(&cfg.gen_version.to_le_bytes())?;
    w.write_all(&cfg.units.to_le_bytes())?;
    w.write_all(&[0u8; 8])
}

/// Reads and validates a vector file header; returns the config and the vector count.
pub fn read_vec_header(r: &mut impl Read) -> io::Result<(Config, u32)> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != VEC_MAGIC {
        return Err(bad("not a vector file (bad magic)"));
    }
    if read_u32(r)? != 1 {
        return Err(bad("unsupported vector file version"));
    }
    let _flags = read_u32(r)?;
    let seed = read_u64(r)?;
    let code_base = read_u64(r)?;
    let mem_base = read_u64(r)?;
    let count = read_u32(r)?;
    let mem_len = read_u32(r)?;
    let gen_version = read_u32(r)?;
    let units = read_u32(r)?;
    let mut reserved = [0u8; 8];
    r.read_exact(&mut reserved)?;
    Ok((
        Config {
            seed,
            gen_version,
            code_base,
            mem_base,
            mem_len,
            units,
        },
        count,
    ))
}

/// The final state of one vector as produced by an oracle (runner or T0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Signal (see [`sig`]); a block that ran to its sentinel is `SIGTRAP` at `pc >= code_end`.
    pub signal: u16,
    /// `si_code` of the signal (informational, never compared).
    pub si_code: i32,
    /// Final registers and `PC`.
    pub state: State,
    /// Final data window if it differs from the initial contents.
    pub mem: Option<Vec<u8>>,
}

/// Raw result file header (64 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResHeader {
    /// Runner version.
    pub runner_version: u32,
    /// Number of results.
    pub count: u32,
    /// Data window length.
    pub mem_len: u32,
    /// Copied from the vector header.
    pub code_base: u64,
    /// Copied from the vector header.
    pub mem_base: u64,
    /// Copied from the vector header.
    pub seed: u64,
}

/// Writes the raw result header.
pub fn write_res_header(w: &mut impl Write, h: &ResHeader) -> io::Result<()> {
    w.write_all(RES_MAGIC)?;
    w.write_all(&1u32.to_le_bytes())?;
    w.write_all(&h.runner_version.to_le_bytes())?;
    w.write_all(&h.count.to_le_bytes())?;
    w.write_all(&h.mem_len.to_le_bytes())?;
    w.write_all(&h.code_base.to_le_bytes())?;
    w.write_all(&h.mem_base.to_le_bytes())?;
    w.write_all(&h.seed.to_le_bytes())?;
    w.write_all(&[0u8; 16])
}

/// Reads the raw result header.
pub fn read_res_header(r: &mut impl Read) -> io::Result<ResHeader> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != RES_MAGIC {
        return Err(bad("not a raw result file (bad magic)"));
    }
    if read_u32(r)? != 1 {
        return Err(bad("unsupported raw result version"));
    }
    let runner_version = read_u32(r)?;
    let count = read_u32(r)?;
    let mem_len = read_u32(r)?;
    let code_base = read_u64(r)?;
    let mem_base = read_u64(r)?;
    let seed = read_u64(r)?;
    let mut reserved = [0u8; 16];
    r.read_exact(&mut reserved)?;
    Ok(ResHeader {
        runner_version,
        count,
        mem_len,
        code_base,
        mem_base,
        seed,
    })
}

impl Outcome {
    /// Serialises a raw result record (280 bytes + `mem_len` if the window changed).
    pub fn write_raw(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&self.signal.to_le_bytes())?;
        w.write_all(&u16::from(self.mem.is_some()).to_le_bytes())?;
        w.write_all(&self.si_code.to_le_bytes())?;
        w.write_all(&self.state.pc.to_le_bytes())?;
        for v in self.state.x {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&self.state.sp.to_le_bytes())?;
        w.write_all(&self.state.nzcv.to_le_bytes())?;
        if let Some(m) = &self.mem {
            w.write_all(m)?;
        }
        Ok(())
    }

    /// Reads a raw result record.
    pub fn read_raw(r: &mut impl Read, mem_len: usize) -> io::Result<Self> {
        let signal = read_u16(r)?;
        let flags = read_u16(r)?;
        let si_code = read_u32(r)? as i32;
        let pc = read_u64(r)?;
        let mut x = [0u64; 31];
        for v in &mut x {
            *v = read_u64(r)?;
        }
        let sp = read_u64(r)?;
        let nzcv = read_u64(r)?;
        let mem = if flags & 1 != 0 {
            let mut m = vec![0u8; mem_len];
            r.read_exact(&mut m)?;
            Some(m)
        } else {
            None
        };
        Ok(Self {
            signal,
            si_code,
            state: State { x, sp, nzcv, pc },
            mem,
        })
    }
}

/// Reads a whole vector file.
pub fn read_vectors(r: &mut impl Read) -> io::Result<(Config, Vec<Vector>)> {
    let (cfg, count) = read_vec_header(r)?;
    let mut v = Vec::with_capacity(count as usize);
    for _ in 0..count {
        v.push(Vector::read(r)?);
    }
    Ok((cfg, v))
}

/// Writes a whole vector file.
pub fn write_vectors(w: &mut impl Write, cfg: &Config, vectors: &[Vector]) -> io::Result<()> {
    let count = u32::try_from(vectors.len()).map_err(|_| bad("too many vectors"))?;
    write_vec_header(w, cfg, count)?;
    for v in vectors {
        v.write(w)?;
    }
    Ok(())
}

/// Reads a whole raw result file.
pub fn read_results(r: &mut impl Read) -> io::Result<(ResHeader, Vec<Outcome>)> {
    let h = read_res_header(r)?;
    let mut v = Vec::with_capacity(h.count as usize);
    for _ in 0..h.count {
        v.push(Outcome::read_raw(r, h.mem_len as usize)?);
    }
    Ok((h, v))
}

pub(crate) fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub(crate) fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

pub(crate) fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

pub(crate) fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vector {
        let mut x = [0u64; 31];
        for (i, v) in x.iter_mut().enumerate() {
            *v = 0x0101_0101_0101_0101u64.wrapping_mul(i as u64 + 1);
        }
        Vector {
            class_id: 7,
            mem_seed: 0x1234,
            x,
            sp: 0xdead_beef_0000,
            nzcv: 0xa000_0000,
            words: vec![0xd503_201f, 0xd280_0020],
        }
    }

    #[test]
    fn vector_roundtrip_and_size() {
        let v = sample();
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), 280 + 8);
        assert_eq!(Vector::read(&mut bytes.as_slice()).unwrap(), v);
    }

    #[test]
    fn vector_file_roundtrip() {
        let cfg = Config {
            seed: 9,
            gen_version: 3,
            code_base: 0x10_0000_0100,
            mem_base: 0x20_0000_0000,
            mem_len: 256,
            units: 5,
        };
        let vs = vec![sample(), sample()];
        let mut buf = Vec::new();
        write_vectors(&mut buf, &cfg, &vs).unwrap();
        assert_eq!(buf.len(), 64 + 2 * 288);
        let (c2, v2) = read_vectors(&mut buf.as_slice()).unwrap();
        assert_eq!(c2, cfg);
        assert_eq!(v2, vs);
    }

    #[test]
    fn rejects_garbage() {
        assert!(read_vec_header(&mut &b"NOTAFILE00000000"[..]).is_err());
        let mut v = sample();
        v.words.clear();
        assert!(Vector::read(&mut v.to_bytes().as_slice()).is_err());
    }

    #[test]
    fn raw_result_roundtrip() {
        let o = Outcome {
            signal: sig::TRAP,
            si_code: 1,
            state: State {
                x: [5; 31],
                sp: 6,
                nzcv: 0x4000_0000,
                pc: 0x10_0000_0108,
            },
            mem: Some(vec![9; 16]),
        };
        let mut buf = Vec::new();
        o.write_raw(&mut buf).unwrap();
        assert_eq!(buf.len(), 280 + 16);
        assert_eq!(Outcome::read_raw(&mut buf.as_slice(), 16).unwrap(), o);
    }
}
