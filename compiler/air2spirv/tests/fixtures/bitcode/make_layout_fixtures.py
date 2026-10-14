#!/usr/bin/env python3
"""Derive block-layout variants of our own bitcode (WP-SC-02, pre-scan vs. LLVM 18 decoding).

Each variant changes where blocks or abbreviation definitions sit, or how long a block claims
to be, so that the tests can check that air2spirv's pre-scan reads the stream the way LLVM 18's
reader does, or rejects layouts where that cannot be guaranteed. See README.md for the table.

    make_layout_fixtures.py <input.bc> <output-directory>
"""

import os
import sys

import bitstream as bs


def blocks(parent, block_id):
    return [c for c in parent.children if isinstance(c, bs.Block) and c.block_id == block_id]


def index_of(parent, child):
    return next(i for i, c in enumerate(parent.children) if c is child)


def small_blockinfo():
    """A BLOCKINFO block that gives CONSTANTS_BLOCK a single, different abbreviation."""
    info = bs.Block(bs.BLOCKINFO_BLOCK_ID, 2)
    info.children = [
        bs.make_record(1, [bs.CONSTANTS_BLOCK_ID]),  # SETBID
        bs.make_definition([("literal", 1), (bs.ENC_FIXED, 4)]),
    ]
    return info


def variant(source, name, change):
    stream = bs.Stream.parse(source)
    change(stream)
    return name, stream.write()


def blockinfo_in_function_block(s):
    function = blocks(s.module(), bs.FUNCTION_BLOCK_ID)[0]
    function.children.insert(0, small_blockinfo())


def blockinfo_at_top_level(s):
    s.top.insert(0, small_blockinfo())


def blockinfo_twice(s):
    module = s.module()
    first = blocks(module, bs.BLOCKINFO_BLOCK_ID)[0]
    module.children.insert(index_of(module, first) + 1, small_blockinfo())


def blockinfo_after_function_block(s):
    module = s.module()
    info = blocks(module, bs.BLOCKINFO_BLOCK_ID)[0]
    function = blocks(module, bs.FUNCTION_BLOCK_ID)[0]
    module.children.insert(index_of(module, function) + 1, info)


def abbrev_after_function_block(s):
    module = s.module()
    function = blocks(module, bs.FUNCTION_BLOCK_ID)[0]
    definition = bs.make_definition([("literal", 99), (bs.ENC_FIXED, 8)])
    module.children.insert(index_of(module, function) + 1, definition)


def function_block_length_mismatch(s):
    blocks(s.module(), bs.FUNCTION_BLOCK_ID)[0].extra_words = 1


def vstoffset_mismatch(s):
    s.offset_skew[("vst", 0)] = 1


def fnentry_offset_mismatch(s):
    s.offset_skew[("fn", 0)] = 1


def fnentry_in_function_symtab(s):
    module = s.module()
    entry = s._offset_records("fn")[0]
    symtab = bs.Block(bs.VALUE_SYMTAB_BLOCK_ID, 4)
    symtab.children = [bs.make_record(bs.VST_CODE_FNENTRY, entry.operands[:2])]
    blocks(module, bs.FUNCTION_BLOCK_ID)[0].children.insert(0, symtab)


def function_block_id_two_chunks(s):
    blocks(s.module(), bs.FUNCTION_BLOCK_ID)[0].id_chunks = 2


def nested_constants_block(s):
    function = blocks(s.module(), bs.FUNCTION_BLOCK_ID)[0]
    constants = blocks(function, bs.CONSTANTS_BLOCK_ID)[0]
    nested = bs.Block(bs.CONSTANTS_BLOCK_ID, 4)
    nested.children = [bs.make_record(1, [0])]  # SETTYPE 0
    constants.children.insert(1, nested)


VARIANTS = [
    ("layout_blockinfo_in_function_block.bc", blockinfo_in_function_block),
    ("layout_blockinfo_at_top_level.bc", blockinfo_at_top_level),
    ("layout_blockinfo_twice.bc", blockinfo_twice),
    ("layout_blockinfo_after_function_block.bc", blockinfo_after_function_block),
    ("layout_abbrev_after_function_block.bc", abbrev_after_function_block),
    ("layout_function_block_length_mismatch.bc", function_block_length_mismatch),
    ("layout_vstoffset_mismatch.bc", vstoffset_mismatch),
    ("layout_fnentry_offset_mismatch.bc", fnentry_offset_mismatch),
    ("layout_fnentry_in_function_symtab.bc", fnentry_in_function_symtab),
    ("layout_function_block_id_two_chunks.bc", function_block_id_two_chunks),
    ("layout_nested_constants_block.bc", nested_constants_block),
]


def main():
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    with open(sys.argv[1], "rb") as f:
        source = f.read()
    if bs.Stream.parse(source).write() != source:
        print("make_layout_fixtures.py: round trip is not bit-identical", file=sys.stderr)
        return 1
    for name, change in VARIANTS:
        name, data = variant(source, name, change)
        with open(os.path.join(sys.argv[2], name), "wb") as f:
            f.write(data)
        print("%s: %d bytes" % (name, len(data)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
