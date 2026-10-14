#!/usr/bin/env python3
"""Rewrite single fields of an LLVM bitcode file in place (test fixture generator, WP-SC-02).

Used by generate.sh to derive "too new" variants of our own clang-generated bitcode, so the
tests can check that air2spirv reports them as A2S_ERROR_UNSUPPORTED. Only the publicly
documented LLVM bitstream format is used (https://llvm.org/docs/BitCodeFormat.html) and the
record numbering from llvm/Bitcode/LLVMBitCodes.h; no Apple data is involved (ADR-0008).

A value is replaced by re-encoding it into exactly the bits the old value occupied (a VBR
field keeps its chunk count; a non-canonical VBR with zero continuation chunks is still valid).
Block lengths therefore stay correct and nothing else in the file moves.

    patch_bitcode.py --epoch N            IN OUT   IDENTIFICATION_BLOCK / EPOCH record
    patch_bitcode.py --module-version N   IN OUT   MODULE_BLOCK / VERSION record
    patch_bitcode.py --attribute-kind N   IN OUT   first enum attribute in PARAMATTR_GROUP_BLOCK
                                                   whose old kind is >= 32 (two VBR6 chunks)
    patch_bitcode.py --attribute-index N  IN OUT   parameter index of the first attribute group
                                                   that can hold N in its encoded width
"""

import argparse
import struct
import sys

WRAPPER_MAGIC = 0x0B17C0DE
BITCODE_MAGIC = b"BC\xc0\xde"

# llvm/Bitstream/BitCodeEnums.h and llvm/Bitcode/LLVMBitCodes.h
END_BLOCK, ENTER_SUBBLOCK, DEFINE_ABBREV, UNABBREV_RECORD = 0, 1, 2, 3
BLOCKINFO_BLOCK_ID = 0
MODULE_BLOCK_ID = 8
PARAMATTR_GROUP_BLOCK_ID = 10
IDENTIFICATION_BLOCK_ID = 13
IDENTIFICATION_CODE_EPOCH = 2
MODULE_CODE_VERSION = 1
PARAMATTR_GRP_CODE_ENTRY = 3

ENC_FIXED, ENC_VBR, ENC_ARRAY, ENC_CHAR6, ENC_BLOB = 1, 2, 3, 4, 5


class BitcodeError(Exception):
    pass


class Field:
    """Location of one encoded scalar: bit offset, total bit width, VBR chunk size (0 = fixed)."""

    def __init__(self, bit, width, chunk, value):
        self.bit, self.width, self.chunk, self.value = bit, width, chunk, value


class Cursor:
    def __init__(self, data, start_bit=0):
        self.data = data
        self.bit = start_bit

    def read(self, n):
        if n == 0:
            return 0
        end = self.bit + n
        if end > len(self.data) * 8:
            raise BitcodeError("unexpected end of bitcode")
        value = 0
        for i in range(n):
            pos = self.bit + i
            value |= ((self.data[pos >> 3] >> (pos & 7)) & 1) << i
        self.bit = end
        return value

    def read_vbr(self, n):
        start = self.bit
        value, shift = 0, 0
        while True:
            piece = self.read(n)
            value |= (piece & ((1 << (n - 1)) - 1)) << shift
            shift += n - 1
            if not piece & (1 << (n - 1)):
                return Field(start, self.bit - start, n, value)
            if shift > 64:
                raise BitcodeError("unterminated VBR")

    def align32(self):
        self.bit = (self.bit + 31) & ~31


def read_abbrev_definition(cur):
    ops = []
    count = cur.read_vbr(5).value
    while len(ops) < count:
        if cur.read(1):
            ops.append(("literal", cur.read_vbr(8).value))
            continue
        enc = cur.read(3)
        if enc in (ENC_FIXED, ENC_VBR):
            ops.append((enc, cur.read_vbr(5).value))
        elif enc in (ENC_ARRAY, ENC_CHAR6, ENC_BLOB):
            ops.append((enc, 0))
        else:
            raise BitcodeError("invalid abbreviation encoding %d" % enc)
    return ops


def read_scalar(cur, op):
    kind, arg = op
    if kind == "literal":
        return Field(cur.bit, 0, 0, arg)
    if kind == ENC_FIXED:
        start = cur.bit
        return Field(start, arg, 0, cur.read(arg))
    if kind == ENC_VBR:
        return cur.read_vbr(arg)
    if kind == ENC_CHAR6:
        start = cur.bit
        return Field(start, 6, 0, cur.read(6))
    raise BitcodeError("unexpected operand encoding %r" % (kind,))


def read_record(cur, abbrev_id, abbrevs):
    """Returns (code Field, [operand Fields]); blobs are skipped and not returned."""
    if abbrev_id == UNABBREV_RECORD:
        code = cur.read_vbr(6)
        count = cur.read_vbr(6).value
        return code, [cur.read_vbr(6) for _ in range(count)]
    index = abbrev_id - 4
    if index < 0 or index >= len(abbrevs):
        raise BitcodeError("undefined abbreviation id %d" % abbrev_id)
    ops = abbrevs[index]
    fields = []
    i = 0
    while i < len(ops):
        kind = ops[i][0]
        if kind == ENC_ARRAY:
            length = cur.read_vbr(6).value
            element = ops[i + 1]
            fields.extend(read_scalar(cur, element) for _ in range(length))
            i += 2
        elif kind == ENC_BLOB:
            length = cur.read_vbr(6).value
            cur.align32()
            cur.bit += length * 8
            cur.align32()
            i += 1
        else:
            fields.append(read_scalar(cur, ops[i]))
            i += 1
    if not fields:
        raise BitcodeError("record without code")
    return fields[0], fields[1:]


def walk_block(cur, abbrev_width, block_id, blockinfo, visit, depth=0):
    """Walks one block body (cursor after the block header) until END_BLOCK."""
    if depth > 16:
        raise BitcodeError("blocks nested too deeply")
    abbrevs = list(blockinfo.get(block_id, []))
    current_info_block = None
    while True:
        abbrev_id = cur.read(abbrev_width)
        if abbrev_id == END_BLOCK:
            cur.align32()
            return
        if abbrev_id == ENTER_SUBBLOCK:
            sub_id = cur.read_vbr(8).value
            sub_width = cur.read_vbr(4).value
            cur.align32()
            num_words = cur.read(32)
            end = cur.bit + num_words * 32
            if visit.wants_block(sub_id) or sub_id == BLOCKINFO_BLOCK_ID:
                walk_block(cur, sub_width, sub_id, blockinfo, visit, depth + 1)
            cur.bit = end
            continue
        if abbrev_id == DEFINE_ABBREV:
            ops = read_abbrev_definition(cur)
            if block_id == BLOCKINFO_BLOCK_ID:
                if current_info_block is None:
                    raise BitcodeError("DEFINE_ABBREV in BLOCKINFO before SETBID")
                blockinfo.setdefault(current_info_block, []).append(ops)
            else:
                abbrevs.append(ops)
            continue
        code, operands = read_record(cur, abbrev_id, abbrevs)
        if block_id == BLOCKINFO_BLOCK_ID:
            if code.value == 1 and operands:  # SETBID
                current_info_block = operands[0].value
            continue
        visit.record(block_id, code.value, operands)


class Finder:
    def __init__(self, mode):
        self.mode = mode
        self.found = None

    def wants_block(self, block_id):
        return {
            "epoch": block_id == IDENTIFICATION_BLOCK_ID,
            "module-version": block_id == MODULE_BLOCK_ID,
            "attribute-kind": block_id in (MODULE_BLOCK_ID, PARAMATTR_GROUP_BLOCK_ID),
            "attribute-index": block_id in (MODULE_BLOCK_ID, PARAMATTR_GROUP_BLOCK_ID),
        }[self.mode]

    def record(self, block_id, code, operands):
        if self.found is not None:
            return
        if self.mode == "epoch":
            if block_id == IDENTIFICATION_BLOCK_ID and code == IDENTIFICATION_CODE_EPOCH:
                self.found = operands[0]
        elif self.mode == "module-version":
            if block_id == MODULE_BLOCK_ID and code == MODULE_CODE_VERSION:
                self.found = operands[0]
        elif self.mode == "attribute-index":
            if block_id == PARAMATTR_GROUP_BLOCK_ID and code == PARAMATTR_GRP_CODE_ENTRY:
                index = operands[1]
                if index.chunk and (index.width // index.chunk) * (index.chunk - 1) >= 32:
                    self.found = index
        elif block_id == PARAMATTR_GROUP_BLOCK_ID and code == PARAMATTR_GRP_CODE_ENTRY:
            # [grpid, paramidx, (kind, ...)*]; kind 0 = enum attribute: [0, attr]
            i = 2
            while i < len(operands):
                kind = operands[i].value
                if kind == 0:
                    attr = operands[i + 1]
                    if attr.chunk == 6 and attr.width == 12:
                        self.found = attr
                        return
                    i += 2
                elif kind == 1:  # int attribute: [1, attr, value]
                    i += 3
                elif kind in (3, 4):  # string attribute: [kind, key..., 0 (, value..., 0)]
                    i += 1
                    for _ in range(2 if kind == 4 else 1):
                        while operands[i].value != 0:
                            i += 1
                        i += 1
                elif kind in (5, 6):  # type attribute: [kind, attr (, type)]
                    i += 3 if kind == 6 else 2
                else:
                    raise BitcodeError("unknown attribute encoding kind %d" % kind)


def encode_into(field, value):
    """Bits (LSB first) that encode `value` in exactly field.width bits."""
    if field.chunk == 0:
        if value >= 1 << field.width:
            raise BitcodeError("value %d does not fit into %d bits" % (value, field.width))
        return [(value >> i) & 1 for i in range(field.width)]
    payload = field.chunk - 1
    chunks = field.width // field.chunk
    if value >= 1 << (payload * chunks):
        raise BitcodeError("value %d does not fit into %d VBR%d chunks" % (value, chunks, field.chunk))
    bits = []
    for c in range(chunks):
        piece = (value >> (c * payload)) & ((1 << payload) - 1)
        if c + 1 < chunks:
            piece |= 1 << payload
        bits.extend((piece >> i) & 1 for i in range(field.chunk))
    return bits


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--epoch", type=int)
    group.add_argument("--module-version", type=int)
    group.add_argument("--attribute-kind", type=int)
    group.add_argument("--attribute-index", type=int)
    parser.add_argument("input")
    parser.add_argument("output")
    args = parser.parse_args()

    if args.epoch is not None:
        mode, value = "epoch", args.epoch
    elif args.module_version is not None:
        mode, value = "module-version", args.module_version
    elif args.attribute_kind is not None:
        mode, value = "attribute-kind", args.attribute_kind
    else:
        mode, value = "attribute-index", args.attribute_index

    with open(args.input, "rb") as f:
        data = bytearray(f.read())

    base = 0
    if len(data) >= 20 and struct.unpack_from("<I", data, 0)[0] == WRAPPER_MAGIC:
        base = struct.unpack_from("<I", data, 8)[0]
    if data[base : base + 4] != BITCODE_MAGIC:
        raise BitcodeError("not LLVM bitcode")
    cur = Cursor(data, (base + 4) * 8)
    finder = Finder(mode)
    end_bit = len(data) * 8
    if base:
        end_bit = (base + struct.unpack_from("<I", data, 12)[0]) * 8
    while cur.bit + 32 <= end_bit and finder.found is None:
        abbrev_id = cur.read(2)
        if abbrev_id != ENTER_SUBBLOCK:
            raise BitcodeError("unexpected top-level abbreviation id %d" % abbrev_id)
        block_id = cur.read_vbr(8).value
        width = cur.read_vbr(4).value
        cur.align32()
        num_words = cur.read(32)
        end = cur.bit + num_words * 32
        if finder.wants_block(block_id):
            walk_block(cur, width, block_id, {}, finder)
        cur.bit = end

    field = finder.found
    if field is None:
        raise BitcodeError("no field to patch for --%s" % mode)
    for i, bit in enumerate(encode_into(field, value)):
        pos = field.bit + i
        if bit:
            data[pos >> 3] |= 1 << (pos & 7)
        else:
            data[pos >> 3] &= ~(1 << (pos & 7)) & 0xFF
    with open(args.output, "wb") as f:
        f.write(data)
    print("%s: %s %d -> %d at bit %d" % (args.output, mode, field.value, value, field.bit))


if __name__ == "__main__":
    try:
        main()
    except BitcodeError as e:
        print("patch_bitcode.py: error: %s" % e, file=sys.stderr)
        sys.exit(1)
