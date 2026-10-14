#!/usr/bin/env python3
"""Generate the small, self-made seed corpus under fuzz/corpus/<target>/.

Seeds are built from the format descriptions in the target sources (fuzz/fuzz_targets/*.rs) and
from the wire format in crates/ritsu-gpu-proto/src/lib.rs; they contain no third-party data
(ADR-0008). Deterministic: running it twice yields identical files.

Usage: python3 fuzz/tools/gen_seeds.py
"""

import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "corpus"


def put(target: str, name: str, data: bytes) -> None:
    d = CORPUS / target
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(data)


# ---- fuzz_gpu_proto_decode ----------------------------------------------------------------
def gpu_stream(records, version=1, magic=b"RTSG", count=None):
    body = b""
    for opcode, flags, payload in records:
        body += struct.pack("<HHI", opcode, flags, len(payload)) + payload
        body += b"\0" * ((-len(payload)) % 8)
    n = len(records) if count is None else count
    return magic + struct.pack("<HHII", version, 0, n, 0) + body


def gpu_seeds():
    t = "fuzz_gpu_proto_decode"
    put(t, "empty_stream", gpu_stream([]))
    put(t, "nop", gpu_stream([(0x0001, 0, b"hello")]))
    create_buf = struct.pack("<IBBBBQ", 1, 0, 0, 0, 0, 4096)
    write_buf = struct.pack("<IIQ", 1, 0, 0) + b"\xaa" * 13
    copy_buf = struct.pack("<IIQQQ", 1, 2, 0, 16, 64)
    create_tex = struct.pack("<IHHBBBBIIIII", 7, 80, 5, 2, 0, 1, 0, 256, 256, 1, 1, 1)
    attach = struct.pack("<IBBBB4f", 7, 2, 1, 0, 0, 0.0, 0.0, 0.0, 1.0)
    begin_rp = struct.pack("<IIII", 256, 256, 1, 0) + attach
    draw = struct.pack("<BBBBIIII", 3, 0, 0, 0, 0, 3, 1, 0)
    dispatch = struct.pack("<6I", 8, 8, 1, 16, 16, 1)
    event = struct.pack("<IIQ", 9, 0, 42)
    commit = struct.pack("<Q", 1)
    full = [
        (0x0100, 0, create_buf),
        (0x0100, 0, struct.pack("<IBBBBQ", 2, 0, 0, 0, 0, 4096)),
        (0x0102, 0, write_buf),
        (0x0103, 0, copy_buf),
        (0x0110, 0, create_tex),
        (0x0200, 0, begin_rp),
        (0x0202, 0, draw),
        (0x0201, 0, b""),
        (0x0300, 0, b""),
        (0x0302, 0, dispatch),
        (0x0301, 0, b""),
        (0x0400, 0, event),
        (0x0401, 0, event),
        (0x0101, 0, struct.pack("<I", 1)),
        (0x0111, 0, struct.pack("<I", 7)),
        (0x0500, 0, commit),
    ]
    put(t, "all_opcodes", gpu_stream(full))
    put(t, "render_pass", gpu_stream([full[4], full[5], full[6], full[7], full[-1]]))
    put(t, "bad_magic", gpu_stream([], magic=b"XXXX"))
    put(t, "bad_version", gpu_stream([], version=2))
    put(t, "count_too_large", gpu_stream([(0x0001, 0, b"")], count=0xFFFFFFFF))
    put(t, "unknown_opcode", gpu_stream([(0xBEEF, 0, b"")]))
    put(t, "huge_payload_len", gpu_stream([], count=1)[:16] + struct.pack("<HHI", 0x0102, 0, 0xFFFFFFF0))
    put(t, "truncated_header", b"RTSG\x01\x00")


# ---- fuzz_gpu_proto_hello / fuzz_gpu_proto_reply / fuzz_gpu_host_session -------------------
def hello_payload(lo=1, hi=1, features=0):
    return struct.pack("<HHIQ", lo, hi, 0, features)


def reply_stream(records, version=1):
    body = b""
    for opcode, payload in records:
        body += struct.pack("<HHI", opcode, 0, len(payload)) + payload
        body += b"\0" * ((-len(payload)) % 8)
    return b"RTSR" + struct.pack("<HHII", version, 0, len(records), 0) + body


def gpu_session_seeds():
    t = "fuzz_gpu_proto_hello"
    put(t, "hello", gpu_stream([(0x0002, 0, hello_payload())]))
    put(t, "hello_events", gpu_stream([(0x0002, 0, hello_payload(1, 1, 1))]))
    put(t, "hello_unknown_features", gpu_stream([(0x0002, 0, hello_payload(1, 4, 2**64 - 1))]))
    put(t, "hello_empty_range", gpu_stream([(0x0002, 0, hello_payload(3, 2))]))
    put(t, "hello_not_alone", gpu_stream([(0x0002, 0, hello_payload()), (0x0001, 0, b"")]))
    put(t, "no_hello", gpu_stream([(0x0001, 0, b"")]))
    put(t, "session_events", gpu_stream([(0x0400, 0, struct.pack("<IIQ", 9, 0, 42))]))
    put(t, "session_second_hello", gpu_stream([(0x0100, 0, struct.pack("<IBBBBQ", 1, 0, 0, 0, 0, 64)),
                                               (0x0002, 0, hello_payload())]))

    t = "fuzz_gpu_proto_reply"
    caps = (struct.pack("<I", 1) + bytes([1]) + bytes([0, 1, 2, 0, 1, 2, 0, 1, 2, 0]) + b"\0"
            + struct.pack("<Q", 1 << 30) + struct.pack("<6I", 16384, 16384, 2048, 2048, 1024, 8))
    put(t, "hello_ack", reply_stream([(0x8001, struct.pack("<H6xQ", 1, 1) + caps)]))
    put(t, "hello_reject", reply_stream([(0x8002, struct.pack("<HHI", 1, 1, 0))]))
    put(t, "session_reset", reply_stream([(0x8003, struct.pack("<QQHHI", 3, 16, 0x0100, 1, 0))]))
    put(t, "record_error", reply_stream([(0x8004, struct.pack("<QIHHIIQ", 2, 5, 0x0102, 1, 7, 0, 99))]))
    put(t, "status_events", reply_stream([
        (0x8005, struct.pack("<QBBHI", 1, 0, 0, 0, 0)),
        (0x8005, struct.pack("<QBBHI", 2, 1, 0, 1, 0)),
        (0x8006, struct.pack("<IIQ", 9, 0, 42)),
    ]))
    put(t, "bad_magic", b"RTSG" + reply_stream([])[4:])

    t = "fuzz_gpu_host_session"

    def chunk(stream):
        return struct.pack("<H", len(stream)) + stream

    cb = (0x0100, 0, struct.pack("<IBBBBQ", 1, 0, 0, 0, 0, 4096))
    tex = (0x0110, 0, struct.pack("<IHHBBBBIIIII", 7, 80, 5, 2, 0, 1, 0, 256, 256, 1, 1, 1))
    wb = (0x0102, 0, struct.pack("<IIQ", 1, 0, 0) + b"\xaa" * 13)
    attach = struct.pack("<IBBBB4f", 7, 2, 1, 0, 0, 0.0, 0.0, 0.0, 1.0)
    rp = (0x0200, 0, struct.pack("<IIII", 256, 256, 1, 0) + attach)
    draw = (0x0202, 0, struct.pack("<BBBBIIII", 3, 0, 0, 0, 0, 3, 1, 0))
    commit = (0x0500, 0, struct.pack("<Q", 1))
    sig = (0x0400, 0, struct.pack("<IIQ", 9, 0, 1))
    put(t, "upload_commit", bytes([0]) + chunk(gpu_stream([cb, wb, commit])))
    put(t, "render_pass", bytes([0]) + chunk(gpu_stream([tex, rp, draw, (0x0201, 0, b""), commit])))
    put(t, "events", bytes([1]) + chunk(gpu_stream([sig, commit])) + chunk(gpu_stream([(0x0401, 0, struct.pack("<IIQ", 9, 0, 1))])))
    put(t, "events_not_negotiated", bytes([0]) + chunk(gpu_stream([sig])))
    put(t, "unknown_features_iris", bytes([7]) + chunk(gpu_stream([cb, commit])))
    put(t, "garbage_then_valid", bytes([0]) + chunk(b"\xff" * 40) + chunk(gpu_stream([cb])))
    put(t, "destroy_unknown", bytes([0]) + chunk(gpu_stream([(0x0101, 0, struct.pack("<I", 77)),
                                                              (0x0111, 0, struct.pack("<I", 78))])))
    put(t, "second_hello", bytes([0]) + chunk(gpu_stream([(0x0002, 0, hello_payload())])))


# ---- fuzz_virtqueue_walk ------------------------------------------------------------------
def poke(off, data):
    assert len(data) < 256
    return bytes([0]) + struct.pack("<HB", off, len(data)) + data


def vq_seeds():
    t = "fuzz_virtqueue_walk"
    POP, PUSH, NOTIFY, RESET, PUBLISH, SETIDX = 1, 2, 3, 4, 5, 6

    def desc(addr, ln, flags, nxt):
        return struct.pack("<QIHH", addr, ln, flags, nxt)

    # Standard layout, size 8: desc at 0 (128 bytes), avail at 128, used after.
    hdr = bytes([3, 0])
    # one two-descriptor chain: readable then writable
    chain = poke(0, desc(0x2000, 64, 1, 1) + desc(0x2100, 32, 2, 0))
    pub = bytes([PUBLISH]) + struct.pack("<H", 0)
    put(t, "chain_ok", hdr + chain + pub + bytes([POP]) + bytes([PUSH]) + struct.pack("<HI", 0, 32)
        + bytes([NOTIFY, POP]))
    put(t, "loop", hdr + poke(0, desc(0x2000, 8, 1, 1) + desc(0x2000, 8, 1, 0)) + pub + bytes([POP, POP]))
    put(t, "readable_after_writable",
        hdr + poke(0, desc(0x2000, 8, 3, 1) + desc(0x2000, 8, 0, 0)) + pub + bytes([POP]))
    put(t, "indirect", hdr + poke(0, desc(0x2000, 16, 4, 0)) + pub + bytes([POP]))
    put(t, "avail_overrun", hdr + bytes([SETIDX]) + struct.pack("<H", 100) + bytes([POP]))
    put(t, "bad_head", hdr + bytes([PUBLISH]) + struct.pack("<H", 999) + bytes([POP]))
    put(t, "buffer_overflow",
        hdr + poke(0, desc(0xFFFFFFFFFFFFFFF0, 0x100, 0, 0)) + pub + bytes([POP]))
    put(t, "push_used_bad_head", hdr + bytes([PUSH]) + struct.pack("<HI", 99, 1))
    put(t, "reset_cycle", hdr + chain + pub + bytes([POP, RESET]) + pub + bytes([POP, PUSH])
        + struct.pack("<HI", 0, 0))
    put(t, "invalid_size", bytes([10, 0]) + struct.pack("<H", 3) + bytes([POP]))
    put(t, "layout_offsets", bytes([2, 1]) + struct.pack("<3I", 0, 0x100, 0x200) + chain + pub
        + bytes([POP]))
    put(t, "layout_raw", bytes([4, 2]) + struct.pack("<3Q", 0x10000000, 0x10000100, 0x10000200)
        + pub + bytes([POP, NOTIFY]))
    put(t, "max_size", bytes([15, 0, POP]))


# ---- fuzz_mmio_bus ------------------------------------------------------------------------
def mmio_seeds():
    t = "fuzz_mmio_bus"

    def raw(a):
        return bytes([0]) + struct.pack("<Q", a)

    def anchor(n, delta=0):
        return bytes([n]) + struct.pack("<h", delta)

    def reg(kind, base, ln):
        return bytes([0, kind]) + base + ln

    def rd(a, sz):
        return bytes([1]) + a + bytes([sz])

    def wr(a, sz, v):
        return bytes([2]) + a + bytes([sz]) + struct.pack("<Q", v)

    LEN_1000 = bytes([1])
    UART = 0x0900_0000
    put(t, "uart_basic",
        reg(0, raw(UART), LEN_1000) + rd(raw(UART + 0x18), 2) + wr(raw(UART), 0, 0x41)
        + rd(raw(UART + 0xFE0), 2) + wr(raw(UART + 0x30), 2, 0x301))
    put(t, "probe_unaligned_base",
        reg(1, raw(UART + 1), LEN_1000) + rd(raw(UART + 4), 2) + wr(raw(UART + 8), 3, ~0 & (2**64 - 1)))
    put(t, "overlap_and_adjacent",
        reg(0, raw(UART), LEN_1000) + reg(1, raw(UART + 0x800), LEN_1000)
        + reg(1, raw(UART + 0x1000), LEN_1000) + rd(raw(UART + 0x1000), 2))
    put(t, "zero_and_overflow",
        reg(0, raw(UART), bytes([0])) + reg(1, raw(2**64 - 8), bytes([3]) + struct.pack("<H", 16))
        + reg(1, raw(1), bytes([2])))
    put(t, "region_end",
        reg(1, raw(0x1000), bytes([3]) + struct.pack("<H", 12)) + rd(anchor(3, 8), 3)
        + rd(anchor(3, 11), 0) + wr(anchor(3, 4), 3, 1))
    put(t, "unmapped_and_misaligned",
        reg(0, raw(UART), LEN_1000) + rd(raw(0), 2) + rd(raw(2**64 - 1), 3) + rd(raw(UART + 2), 2))


# ---- fuzz_aarch64_decode / fuzz_t0_step ---------------------------------------------------
def words(*ws):
    return b"".join(struct.pack("<I", w) for w in ws)


def a64_seeds():
    t = "fuzz_aarch64_decode"
    # Hand-assembled from the Arm ARM encodings.
    nop = 0xD503201F
    movz_x0 = 0xD2800540  # MOVZ X0, #42
    add_imm = 0x91000421  # ADD X1, X1, #1
    subs_imm = 0xF1000421  # SUBS X1, X1, #1
    b_self = 0x14000000  # B .
    b_fwd = 0x14000002
    bl = 0x94000004
    br_x1 = 0xD61F0020
    ret = 0xD65F03C0
    cbz = 0xB4000040
    bcond = 0x54000040
    allzero = 0x00000000
    allone = 0xFFFFFFFF
    put(t, "straightline", words(nop, movz_x0, add_imm, subs_imm, nop))
    put(t, "branches", words(b_fwd, nop, bl, br_x1, ret, cbz, bcond))
    put(t, "edge_words", words(allzero, allone, 0x8000_0000, 0x7FFF_FFFF, 0xD4000001))
    put(t, "self_loop", words(b_self))

    t0 = "fuzz_t0_step"
    hdr = bytes([0x00, 1, 2, 3, 4, 5, 6, 7])
    put(t0, "straightline", hdr + words(nop, movz_x0, add_imm, subs_imm, bcond, nop, b_self))
    put(t0, "el0_flags", bytes([0xF1, 9, 8, 7, 6, 5, 4, 3]) + words(add_imm, subs_imm, bcond, ret))
    put(t0, "undefined_first", hdr + words(allzero, nop))
    put(t0, "misc", hdr + words(0xD2800000, 0xF2A00000, 0x92400000, 0x52800020, 0xB2000000, 0x8B000000))


def main() -> None:
    gpu_seeds()
    gpu_session_seeds()
    vq_seeds()
    mmio_seeds()
    a64_seeds()


if __name__ == "__main__":
    main()
