"""Minimal LLVM bitstream reader/writer for test fixture generation (WP-SC-02).

Parses a raw LLVM bitcode stream into a tree of blocks and entries, keeps the exact bits of every
record and abbreviation definition, and writes the tree back. Unmodified trees are written back
bit-identically. Used by make_layout_fixtures.py to derive variants of our own clang-generated
bitcode with a different block layout. Only the public bitstream format
(https://llvm.org/docs/BitCodeFormat.html) and record numbers from llvm/Bitcode/LLVMBitCodes.h
are used; no Apple data (ADR-0008).

Function block positions are referenced by word offsets (MODULE_CODE_VSTOFFSET and the
symbol table's FNENTRY records). write() recomputes them after layout changes, so a variant is
consistent unless a test deliberately breaks an offset afterwards (Stream.offset_skew).
"""

BITCODE_MAGIC = b"BC\xc0\xde"
END_BLOCK, ENTER_SUBBLOCK, DEFINE_ABBREV, UNABBREV_RECORD = 0, 1, 2, 3
BLOCKINFO_BLOCK_ID = 0
MODULE_BLOCK_ID = 8
CONSTANTS_BLOCK_ID = 11
FUNCTION_BLOCK_ID = 12
IDENTIFICATION_BLOCK_ID = 13
VALUE_SYMTAB_BLOCK_ID = 14
MODULE_CODE_VSTOFFSET = 13
VST_CODE_FNENTRY = 3
ENC_FIXED, ENC_VBR, ENC_ARRAY, ENC_CHAR6, ENC_BLOB = 1, 2, 3, 4, 5


class BitstreamError(Exception):
    pass


class Reader:
    def __init__(self, data):
        self.data = data
        self.bit = 0

    def read(self, n):
        if self.bit + n > len(self.data) * 8:
            raise BitstreamError("unexpected end of bitstream")
        value = 0
        for i in range(n):
            pos = self.bit + i
            value |= ((self.data[pos >> 3] >> (pos & 7)) & 1) << i
        self.bit += n
        return value

    def read_vbr(self, n):
        value, shift = 0, 0
        while True:
            piece = self.read(n)
            value |= (piece & ((1 << (n - 1)) - 1)) << shift
            shift += n - 1
            if not piece & (1 << (n - 1)):
                return value
            if shift > 64:
                raise BitstreamError("unterminated VBR")

    def align32(self):
        self.bit = (self.bit + 31) & ~31

    def bits(self, start, end):
        out = []
        for pos in range(start, end):
            out.append((self.data[pos >> 3] >> (pos & 7)) & 1)
        return out


class Writer:
    def __init__(self):
        self.bits = []

    def emit(self, value, n):
        for i in range(n):
            self.bits.append((value >> i) & 1)

    def emit_vbr(self, value, n, chunks=1):
        payload = n - 1
        written = 0
        while True:
            piece = value & ((1 << payload) - 1)
            value >>= payload
            written += 1
            more = value != 0 or written < chunks
            self.emit(piece | ((1 << payload) if more else 0), n)
            if not more:
                return

    def align32(self):
        while len(self.bits) % 32:
            self.bits.append(0)

    def patch(self, bit, value, n):
        for i in range(n):
            self.bits[bit + i] = (value >> i) & 1

    def to_bytes(self):
        out = bytearray((len(self.bits) + 7) // 8)
        for i, b in enumerate(self.bits):
            if b:
                out[i >> 3] |= 1 << (i & 7)
        return bytes(out)


class Abbrev:
    def __init__(self, ops):
        self.ops = ops  # [("literal", v) | (ENC_*, width)]


class Record:
    """A record or an abbreviation definition, with its original bits (incl. the abbrev ID)."""

    def __init__(self, raw=None, abbrev_id=None, abbrev=None, fields=None, is_definition=False):
        self.raw = raw
        self.abbrev_id = abbrev_id
        self.abbrev = abbrev
        self.fields = fields or []  # scalar values in abbreviation order, code first
        self.is_definition = is_definition
        self.definition = None  # Abbrev, for definitions
        self.overrides = {}  # field index -> new value (re-encodes the record)
        self.field_bits = []  # absolute bit position of each scalar after write()
        self.start_bit = None

    @property
    def code(self):
        return None if self.is_definition else self.fields[0]

    @property
    def operands(self):
        values = [self.overrides.get(i, v) for i, v in enumerate(self.fields)]
        return values[1:]


class Block:
    def __init__(self, block_id, codelen, children=None):
        self.block_id = block_id
        self.codelen = codelen
        self.children = children if children is not None else []
        self.id_chunks = 1  # VBR8 chunks used for the block ID (1 = canonical)
        self.extra_words = 0  # zero words after END_BLOCK inside the declared length
        self.start_bit = None  # position of the ENTER_SUBBLOCK abbreviation ID after write()
        self.orig_start_bit = None


def make_record(code, operands):
    """An unabbreviated record (written with the width of the block it is placed in)."""
    return Record(abbrev_id=UNABBREV_RECORD, fields=[code] + list(operands))


def make_definition(ops):
    record = Record(is_definition=True)
    record.definition = Abbrev(ops)
    return record


def _read_definition(r):
    ops = []
    count = r.read_vbr(5)
    while len(ops) < count:
        if r.read(1):
            ops.append(("literal", r.read_vbr(8)))
            continue
        enc = r.read(3)
        if enc in (ENC_FIXED, ENC_VBR):
            ops.append((enc, r.read_vbr(5)))
        elif enc in (ENC_ARRAY, ENC_CHAR6, ENC_BLOB):
            ops.append((enc, 0))
        else:
            raise BitstreamError("invalid abbreviation encoding %d" % enc)
    return Abbrev(ops)


def _read_scalar(r, op):
    kind, arg = op
    if kind == "literal":
        return arg
    if kind == ENC_FIXED:
        return r.read(arg)
    if kind == ENC_VBR:
        return r.read_vbr(arg)
    if kind == ENC_CHAR6:
        return r.read(6)
    raise BitstreamError("unexpected operand encoding")


def _read_record(r, abbrev_id, abbrevs):
    if abbrev_id == UNABBREV_RECORD:
        code = r.read_vbr(6)
        count = r.read_vbr(6)
        return None, [code] + [r.read_vbr(6) for _ in range(count)]
    index = abbrev_id - 4
    if not 0 <= index < len(abbrevs):
        raise BitstreamError("undefined abbreviation id %d" % abbrev_id)
    abbrev = abbrevs[index]
    fields = []
    i = 0
    while i < len(abbrev.ops):
        kind = abbrev.ops[i][0]
        if kind == ENC_ARRAY:
            length = r.read_vbr(6)
            fields.append(("array", [_read_scalar(r, abbrev.ops[i + 1]) for _ in range(length)]))
            i += 2
        elif kind == ENC_BLOB:
            length = r.read_vbr(6)
            r.align32()
            r.bit += length * 8
            r.align32()
            fields.append(("blob", length))
            i += 1
        else:
            fields.append(_read_scalar(r, abbrev.ops[i]))
            i += 1
    return abbrev, fields


def _parse_block_body(r, block, blockinfo):
    abbrevs = list(blockinfo.get(block.block_id, []))
    current_info = None
    while True:
        start = r.bit
        abbrev_id = r.read(block.codelen)
        if abbrev_id == END_BLOCK:
            r.align32()
            return
        if abbrev_id == ENTER_SUBBLOCK:
            child = _parse_block_header(r, start)
            block.children.append(child)
            _parse_block_body(r, child, blockinfo)
            continue
        if abbrev_id == DEFINE_ABBREV:
            definition = _read_definition(r)
            record = Record(raw=r.bits(start, r.bit), is_definition=True)
            record.definition = definition
            block.children.append(record)
            if block.block_id == BLOCKINFO_BLOCK_ID:
                blockinfo.setdefault(current_info, []).append(definition)
            else:
                abbrevs.append(definition)
            continue
        abbrev, fields = _read_record(r, abbrev_id, abbrevs)
        record = Record(raw=r.bits(start, r.bit), abbrev_id=abbrev_id, abbrev=abbrev,
                        fields=fields)
        block.children.append(record)
        if block.block_id == BLOCKINFO_BLOCK_ID and fields[0] == 1 and len(fields) > 1:
            current_info = fields[1]  # SETBID


def _parse_block_header(r, start):
    block_id = r.read_vbr(8)
    codelen = r.read_vbr(4)
    r.align32()
    r.read(32)  # length, recomputed on write
    block = Block(block_id, codelen)
    block.orig_start_bit = start
    return block


class Stream:
    def __init__(self, top):
        self.top = top  # top-level blocks
        self.offset_skew = {}  # ("vst" | "fn", index) -> words added after fix-ups

    @staticmethod
    def parse(data):
        if data[:4] != BITCODE_MAGIC:
            raise BitstreamError("not raw LLVM bitcode (wrapper not supported here)")
        r = Reader(data)
        r.bit = 32
        top = []
        while r.bit + 64 <= len(data) * 8:
            start = r.bit
            if r.read(2) != ENTER_SUBBLOCK:
                raise BitstreamError("unexpected top-level entry")
            block = _parse_block_header(r, start)
            _parse_block_body(r, block, {})
            top.append(block)
        return Stream(top)

    def module(self):
        return next(b for b in self.top if b.block_id == MODULE_BLOCK_ID)

    def write(self):
        for _ in range(8):  # offsets are fixed points of the layout; converge quickly
            writer = Writer()
            writer.emit(0xDEC04342, 32)
            for block in self.top:
                _write_block(writer, block, 2)
            if not self._fix_offsets():
                break
        else:
            raise BitstreamError("offsets did not converge")
        for (kind, index), skew in self.offset_skew.items():
            record = self._offset_records(kind)[index]
            field = 1 if kind == "vst" else 2
            record.overrides[field] = record.operands[field - 1] + skew
        if self.offset_skew:
            writer = Writer()
            writer.emit(0xDEC04342, 32)
            for block in self.top:
                _write_block(writer, block, 2)
        return writer.to_bytes()

    def _base_bit(self):
        module = self.module()
        index = self.top.index(module)
        if index > 0 and self.top[index - 1].block_id == IDENTIFICATION_BLOCK_ID:
            return self.top[index - 1].start_bit
        return module.start_bit

    def _offset_records(self, kind):
        module = self.module()
        if kind == "vst":
            return [c for c in module.children
                    if isinstance(c, Record) and c.code == MODULE_CODE_VSTOFFSET]
        vst = [c for c in module.children
               if isinstance(c, Block) and c.block_id == VALUE_SYMTAB_BLOCK_ID]
        return [c for b in vst for c in b.children
                if isinstance(c, Record) and c.code == VST_CODE_FNENTRY]

    def _fix_offsets(self):
        """Point VSTOFFSET and FNENTRY at the written positions; True if anything changed."""
        module = self.module()
        base = self._base_bit()
        changed = False
        functions = [c for c in module.children
                     if isinstance(c, Block) and c.block_id == FUNCTION_BLOCK_ID]
        by_orig = {}
        for f in functions:
            if f.orig_start_bit is not None:
                by_orig[f.orig_start_bit] = f
        vst = [c for c in module.children
               if isinstance(c, Block) and c.block_id == VALUE_SYMTAB_BLOCK_ID]
        orig_base = self._orig_base_bit()
        for record in self._offset_records("vst"):
            if not vst:
                continue
            value = (vst[0].start_bit - base) // 32 + 1
            if record.operands[0] != value:
                record.overrides[1] = value
                changed = True
        for record in self._offset_records("fn"):
            original = record.fields[2]
            target = by_orig.get(orig_base + (original - 1) * 32)
            if target is None:
                raise BitstreamError("FNENTRY offset does not match a function block")
            value = (target.start_bit - base) // 32 + 1
            if record.operands[1] != value:
                record.overrides[2] = value
                changed = True
        return changed

    def _orig_base_bit(self):
        module = self.module()
        index = self.top.index(module)
        if index > 0 and self.top[index - 1].block_id == IDENTIFICATION_BLOCK_ID:
            return self.top[index - 1].orig_start_bit
        return module.orig_start_bit


def _write_scalar(writer, op, value):
    kind, arg = op
    if kind == "literal":
        if value != arg:
            raise BitstreamError("literal operand cannot change")
    elif kind == ENC_FIXED:
        writer.emit(value, arg)
    elif kind == ENC_VBR:
        writer.emit_vbr(value, arg)
    elif kind == ENC_CHAR6:
        writer.emit(value, 6)
    else:
        raise BitstreamError("cannot re-encode this operand")


def _write_record(writer, record, width):
    record.start_bit = len(writer.bits)
    if record.raw is not None and not record.overrides:
        writer.bits.extend(record.raw)
        return
    if record.is_definition:
        writer.emit(DEFINE_ABBREV, width)
        writer.emit_vbr(len(record.definition.ops), 5)
        for kind, arg in record.definition.ops:
            if kind == "literal":
                writer.emit(1, 1)
                writer.emit_vbr(arg, 8)
            else:
                writer.emit(0, 1)
                writer.emit(kind, 3)
                if kind in (ENC_FIXED, ENC_VBR):
                    writer.emit_vbr(arg, 5)
        return
    values = [record.overrides.get(i, v) for i, v in enumerate(record.fields)]
    if record.abbrev is None:
        writer.emit(UNABBREV_RECORD, width)
        writer.emit_vbr(values[0], 6)
        writer.emit_vbr(len(values) - 1, 6)
        for v in values[1:]:
            writer.emit_vbr(v, 6)
        return
    writer.emit(record.abbrev_id, width)
    ops = record.abbrev.ops
    fi = 0
    i = 0
    while i < len(ops):
        if ops[i][0] in (ENC_ARRAY, ENC_BLOB):
            raise BitstreamError("cannot re-encode records with arrays or blobs")
        _write_scalar(writer, ops[i], values[fi])
        fi += 1
        i += 1


def _write_block(writer, block, width):
    block.start_bit = len(writer.bits)
    writer.emit(ENTER_SUBBLOCK, width)
    writer.emit_vbr(block.block_id, 8, block.id_chunks)
    writer.emit_vbr(block.codelen, 4)
    writer.align32()
    length_bit = len(writer.bits)
    writer.emit(0, 32)
    body = len(writer.bits)
    for child in block.children:
        if isinstance(child, Block):
            _write_block(writer, child, block.codelen)
        else:
            _write_record(writer, child, block.codelen)
    writer.emit(END_BLOCK, block.codelen)
    writer.align32()
    writer.emit(0, 32 * block.extra_words)
    writer.patch(length_bit, (len(writer.bits) - body) // 32, 32)
