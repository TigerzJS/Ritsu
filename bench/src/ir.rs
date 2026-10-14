//! IR workloads (Q-PERF-1, input for WP-DBT-17): lowering and verifying a region made of
//! `CBZ` instructions. A 16 KiB page of `CBZ` (4096 instructions) is the worst case for block
//! count: every `CBZ` splits the current block, giving `2 * count + 1` blocks (8193 per page).

use ritsu_dbt::ir::{Builder, Region};
use ritsu_dbt::{Inst, decode};

/// `CBZ X0, #+8`
const CBZ_X0_PLUS_8: u32 = 0xb400_0000 | (2 << 5);

/// Decoded straight-line sequence of `count` identical `CBZ` instructions.
#[must_use]
pub fn cbz_insts(count: usize) -> Vec<Inst> {
    vec![decode(CBZ_X0_PLUS_8); count]
}

/// The region built from `count` `CBZ` instructions.
#[must_use]
pub fn cbz_region(count: usize) -> Region {
    Builder::lower_sequence(&cbz_insts(count))
}

/// Number of blocks of [`cbz_region`]`(count)`, derived by hand: the entry block, and per `CBZ`
/// one taken-exit block and one fall-through block.
#[must_use]
pub const fn cbz_region_blocks(count: usize) -> usize {
    2 * count + 1
}

/// Shape of a region: blocks, SSA values, operations (all deterministic for a given input).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    /// Number of basic blocks.
    pub blocks: u64,
    /// Number of SSA values.
    pub values: u64,
    /// Number of operations over all blocks (terminators not included).
    pub ops: u64,
}

/// Counts blocks, values and operations of `r`.
#[must_use]
pub fn shape(r: &Region) -> Shape {
    Shape {
        blocks: r.blocks().len() as u64,
        values: r.num_values() as u64,
        ops: r.blocks().iter().map(|b| b.insts.len() as u64).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_sizes_match_the_hand_derived_block_count() {
        for count in [1usize, 2, 256, 4096] {
            let r = cbz_region(count);
            assert_eq!(
                r.blocks().len(),
                cbz_region_blocks(count),
                "count = {count}"
            );
            assert_eq!(r.verify(), Ok(()), "count = {count}");
        }
    }

    #[test]
    fn full_page_is_8193_blocks() {
        assert_eq!(cbz_region_blocks(16 * 1024 / 4), 8193);
    }

    #[test]
    fn shape_grows_linearly() {
        let a = shape(&cbz_region(256));
        let b = shape(&cbz_region(1024));
        assert_eq!(b.blocks - 1, 4 * (a.blocks - 1));
        assert_eq!(b.ops, 4 * a.ops);
        assert_eq!(b.values, 4 * a.values);
    }

    #[test]
    fn the_input_word_is_a_cbz() {
        assert!(matches!(
            decode(CBZ_X0_PLUS_8),
            Inst::CompareBranch {
                nonzero: false,
                offset: 8,
                ..
            }
        ));
    }
}
