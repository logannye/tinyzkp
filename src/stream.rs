//! Streaming & Block Buffers
//!
//! # What changed in this revision
//! - Added ergonomic helpers to make **blocked traversals** and **tiling**
//!   safer and clearer, with result-returning variants when appropriate.
//! - Introduced generic chunking utilities (low→high and high→low) that
//!   are used when wiring sublinear transforms and PCS ingestion.
//! - Kept all existing APIs and semantics intact (tests remain green).
//!
//! - NEW: `CoeffTileStream` trait — a light interface the blocked-IFFT can
//!   implement to yield coefficient tiles without materializing full polys.
//! - NEW: `horner_eval_stream` — evaluate ∑ a_i z^i over tiles (no O(N) buffer).
//! - NEW: `SliceTileStream` — a zero-copy adapter to turn a slice into tiles.
//! - NEW: `BorrowingRestreamer` — an optional, reference-based restreaming API
//!   to avoid cloning when the source can yield `&Row` (keeps `Restreamer` intact).
//!
//! # Rationale
//! The whitepaper’s sublinear-space design relies on *time-ordered* blocks
//! and coefficient/evaluation **tiles**. These helpers centralize the indexing
//! arithmetic and reduce off-by-one/overflow risk across the codebase.
//! The new tile interface and Horner folding enable streaming witness building
//! and remove O(T) peaks in the witness aggregator path.

#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(unused_imports)]

use crate::F;
use ark_ff::{Field, One, Zero}; // bring traits into scope

/// Index of a time block `t ∈ {0..B-1}`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockIdx(pub usize);
impl BlockIdx {
    /// Access the underlying index.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Index of a row in the global trace `i ∈ {0..T-1}`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowIdx(pub usize);
impl RowIdx {
    /// Access the underlying index.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Index of a register/column `m ∈ {0..k-1}`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegIdx(pub usize);
impl RegIdx {
    /// Access the underlying index.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Errors surfaced by the streaming utilities.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("b_blk must be positive (got {0})")]
    BadBlockSize(usize),
    #[error("block index {t} out of range (B={b})")]
    BlockOutOfRange { t: usize, b: usize },
}

/// Compute the number of blocks `B` for `n_rows = T` and block size `b_blk` (Result).
#[inline]
pub fn block_count_r(n_rows: usize, b_blk: usize) -> Result<usize, StreamError> {
    if b_blk == 0 {
        return Err(StreamError::BadBlockSize(b_blk));
    }
    Ok((n_rows + b_blk - 1) / b_blk)
}

/// Back-compat wrapper (panics on error).
#[inline]
pub fn block_count(n_rows: usize, b_blk: usize) -> usize {
    block_count_r(n_rows, b_blk).expect("b_blk must be positive")
}

/// Get the half-open row bounds `[start, end)` for block `t` (Result).
#[inline]
pub fn block_bounds_r(
    t: BlockIdx,
    n_rows: usize,
    b_blk: usize,
) -> Result<(RowIdx, RowIdx), StreamError> {
    let b_cnt = block_count_r(n_rows, b_blk)?;
    if t.0 >= b_cnt {
        return Err(StreamError::BlockOutOfRange { t: t.0, b: b_cnt });
    }
    let start = t.0 * b_blk;
    let end = ((t.0 + 1) * b_blk).min(n_rows);
    Ok((RowIdx(start), RowIdx(end)))
}

/// Back-compat wrapper (panics on error).
#[inline]
pub fn block_bounds(t: BlockIdx, n_rows: usize, b_blk: usize) -> (RowIdx, RowIdx) {
    block_bounds_r(t, n_rows, b_blk).expect("block index out of range")
}

/// Length of a block slice `[start, end)`.
#[inline]
pub fn block_len(start: RowIdx, end: RowIdx) -> usize {
    debug_assert!(end.0 >= start.0, "invalid block bounds");
    end.0 - start.0
}

/// Partition `n_rows = T` into contiguous time blocks of size `b_blk`
/// (except possibly the final shorter block).
///
/// Returns an iterator yielding `(BlockIdx, RowIdx(start), RowIdx(end))` triples.
///
/// # Notes
/// The internal arithmetic is simple and *non-panicking*. For the
/// result-returning variant, see [`blocks_r`].
pub fn blocks(
    n_rows: usize,
    b_blk: usize,
) -> impl Iterator<Item = (BlockIdx, RowIdx, RowIdx)> {
    let b_cnt = block_count(n_rows, b_blk);
    (0..b_cnt).map(move |t| {
        let start = t * b_blk;
        let end = ((t + 1) * b_blk).min(n_rows);
        (BlockIdx(t), RowIdx(start), RowIdx(end))
    })
}

/// Result-returning variant of [`blocks`], propagating any shape errors.
///
/// This **pre-validates** `b_blk` and the block count so the iterator itself
/// cannot fail mid-way. We also compute bounds directly (no panicking calls).
pub fn blocks_r(
    n_rows: usize,
    b_blk: usize,
) -> Result<impl Iterator<Item = (BlockIdx, RowIdx, RowIdx)>, StreamError> {
    let b_cnt = block_count_r(n_rows, b_blk)?;
    Ok((0..b_cnt).map(move |t| {
        // Safe by construction: t ∈ [0, b_cnt).
        let start = t * b_blk;
        let end = ((t + 1) * b_blk).min(n_rows);
        (BlockIdx(t), RowIdx(start), RowIdx(end))
    }))
}

/// Suggested traversal strategies for block processing.
pub enum Traversal {
    LayeredBfs,
    DfsSmallStack,
}

/// Return an iterator over block indices according to a traversal policy.
/// (Currently always increasing time to preserve causality.)
pub fn traverse_blocks(_t: Traversal, b_cnt: usize) -> impl Iterator<Item = BlockIdx> {
    (0..b_cnt).map(BlockIdx)
}

/// A tiny guard to help enforce **strictly increasing** block order.
pub struct MonotoneBlockGuard {
    prev: Option<BlockIdx>,
}
impl MonotoneBlockGuard {
    /// Create a new guard.
    #[inline]
    pub fn new() -> Self {
        Self { prev: None }
    }
    /// Observe `t` and debug-assert that it is strictly increasing.
    #[inline]
    pub fn observe(&mut self, t: BlockIdx) {
        if let Some(p) = self.prev {
            debug_assert!(
                t.0 > p.0,
                "block indices must be strictly increasing (got {}, prev {})",
                t.0,
                p.0
            );
        }
        self.prev = Some(t);
    }
}

/// Per-block workspace with preallocated buffers.
///
/// This keeps peak allocations ~O(b_blk) while reusing capacity across blocks.
pub struct BlockWs {
    pub reg_vals: Vec<F>,
    pub locals: Vec<crate::air::Locals>,
    pub msm_tmp: Vec<F>,
}

impl BlockWs {
    /// Create a new workspace with capacity `cap = b_blk` for all buffers.
    pub fn new(cap: usize) -> Self {
        Self {
            reg_vals: Vec::with_capacity(cap),
            locals: Vec::with_capacity(cap),
            msm_tmp: Vec::with_capacity(cap),
        }
    }

    /// Clear buffers between blocks without freeing capacity.
    #[inline]
    pub fn reset(&mut self) {
        self.reg_vals.clear();
        self.locals.clear();
        self.msm_tmp.clear();
    }

    /// Ensure capacities are at least `cap` (useful if `b_blk` changes).
    pub fn ensure_cap(&mut self, cap: usize) {
        if self.reg_vals.capacity() < cap {
            self.reg_vals.reserve(cap - self.reg_vals.capacity());
        }
        if self.locals.capacity() < cap {
            self.locals.reserve(cap - self.locals.capacity());
        }
        if self.msm_tmp.capacity() < cap {
            self.msm_tmp.reserve(cap - self.msm_tmp.capacity());
        }
    }

    /// Debug-only: assert live memory looks O(b_blk).
    #[inline]
    pub fn debug_assert_o_bblk(&self, b_blk: usize) {
        debug_assert!(
            self.reg_vals.capacity() <= 2 * b_blk
                && self.locals.capacity() <= 2 * b_blk
                && self.msm_tmp.capacity() <= 2 * b_blk,
            "BlockWs buffers look larger than expected: reg_vals cap={}, locals cap={}, msm_tmp cap={}, b_blk={}",
            self.reg_vals.capacity(),
            self.locals.capacity(),
            self.msm_tmp.capacity(),
            b_blk
        );
        let _ = b_blk; // silence when debug_asserts are stripped
    }
}

// ============================================================================
// Restreaming API — *read again* without buffering more state
// ============================================================================

/// A source that can **re-stream** rows between `[start, end)` on demand.
///
/// This version returns owned items. Use it when your row type is cheap to
/// clone or already `Copy`. For zero-copy sources, see [`BorrowingRestreamer`].
pub trait Restreamer {
    type Item;

    /// Total number of rows `T` available from this source.
    fn len_rows(&self) -> usize;

    /// Produce a fresh iterator over rows in the half-open range `[start, end)`.
    fn stream_rows(
        &self,
        start: RowIdx,
        end: RowIdx,
    ) -> Box<dyn Iterator<Item = Self::Item> + '_>;
}

/// Optional: a **borrowing** variant to avoid deep clones when the source can
/// yield references to its rows. This is additive and does not replace
/// `Restreamer` to preserve back-compat in downstream code.
pub trait BorrowingRestreamer {
    type Item;

    /// Total number of rows `T` available from this source.
    fn len_rows(&self) -> usize;

    /// Produce a fresh iterator over `&Item` in `[start, end)`.
    fn stream_rows_ref<'a>(
        &'a self,
        start: RowIdx,
        end: RowIdx,
    ) -> Box<dyn Iterator<Item = &'a Self::Item> + 'a>;
}

/// Trivial restreamer over an in-memory `Vec<Row>` (owned items).
impl Restreamer for Vec<crate::air::Row> {
    type Item = crate::air::Row;

    #[inline]
    fn len_rows(&self) -> usize {
        self.len()
    }

    #[inline]
    fn stream_rows(
        &self,
        start: RowIdx,
        end: RowIdx,
    ) -> Box<dyn Iterator<Item = Self::Item> + '_> {
        let s = start.as_usize();
        let e = end.as_usize();
        debug_assert!(s <= e && e <= self.len(), "restream range out of bounds");
        let s = s.min(self.len());
        let e = e.min(self.len());
        Box::new(self[s..e].iter().cloned())
    }
}

/// Borrowing restreamer for `Vec<Row>` (zero-copy).
impl BorrowingRestreamer for Vec<crate::air::Row> {
    type Item = crate::air::Row;

    #[inline]
    fn len_rows(&self) -> usize {
        self.len()
    }

    #[inline]
    fn stream_rows_ref<'a>(
        &'a self,
        start: RowIdx,
        end: RowIdx,
    ) -> Box<dyn Iterator<Item = &'a Self::Item> + 'a> {
        let s = start.as_usize().min(self.len());
        let e = end.as_usize().min(self.len());
        debug_assert!(s <= e, "restream range out of bounds");
        Box::new(self[s..e].iter())
    }
}

// ============================================================================
// Generic tiling helpers (evaluation/coefficients, safe chunking)
// ============================================================================

/// Chunk a slice into **low→high** tiles of (at most) `tile` elements.
pub fn chunks_low_to_high<'a, T>(v: &'a [T], tile: usize) -> impl Iterator<Item = &'a [T]> {
    assert!(tile > 0, "tile size must be positive");
    let n = v.len();
    let mut start = 0usize;
    std::iter::from_fn(move || {
        if start >= n {
            return None;
        }
        let end = (start + tile).min(n);
        let out = &v[start..end];
        start = end;
        Some(out)
    })
}

/// Chunk a slice into **high→low** tiles of (at most) `tile` elements.
pub fn chunks_high_to_low<'a, T>(v: &'a [T], tile: usize) -> impl Iterator<Item = &'a [T]> {
    assert!(tile > 0, "tile size must be positive");
    let n = v.len();
    let mut end = n;
    std::iter::from_fn(move || {
        if end == 0 {
            return None;
        }
        let start = end.saturating_sub(tile);
        let out = &v[start..end];
        end = start;
        Some(out)
    })
}

/// Apply a closure to each block triple `(t, start, end)` in increasing time.
pub fn for_each_block(n_rows: usize, b_blk: usize, mut f: impl FnMut(BlockIdx, RowIdx, RowIdx)) {
    for (t, s, e) in blocks(n_rows, b_blk) {
        f(t, s, e);
    }
}

// ============================================================================
// Coefficient tile streaming & Horner folding
// ============================================================================

/// A light interface the blocked-IFFT can implement to yield **coefficient tiles**
/// in ascending power order `a_0..a_{t-1}`. Implementations may reuse the
/// returned slice buffer between calls.
pub trait CoeffTileStream {
    /// Returns the next tile of coefficients in *ascending* power order `a_0..a_{t-1}`,
    /// or `None` when the stream is exhausted.
    fn next_tile(&mut self) -> Option<&[F]>;

    /// (Optional) starting exponent for the **next** tile — i.e. the global
    /// base exponent of the first element in the slice that would be returned
    /// by the next `next_tile()` call. Implementers should advance this by
    /// `tile.len()` each time they yield a tile. Consumers can ignore this.
    fn base_exp(&self) -> usize {
        0
    }
}

/// Zero-copy adapter to treat a coefficient slice as a stream of tiles.
/// Useful for tests and as a simple baseline producer.
///
/// Yields tiles in low→high order and advances `base_exp` accordingly.
pub struct SliceTileStream<'a> {
    data: &'a [F],
    tile_len: usize,
    cursor: usize,
    base_exp: usize,
}

impl<'a> SliceTileStream<'a> {
    /// Construct a new slice-backed tile stream.
    pub fn new(data: &'a [F], tile_len: usize) -> Self {
        assert!(tile_len > 0, "tile_len must be positive");
        Self {
            data,
            tile_len,
            cursor: 0,
            base_exp: 0,
        }
    }
}

impl<'a> CoeffTileStream for SliceTileStream<'a> {
    fn next_tile(&mut self) -> Option<&[F]> {
        if self.cursor >= self.data.len() {
            return None;
        }
        let end = (self.cursor + self.tile_len).min(self.data.len());
        let out = &self.data[self.cursor..end];
        self.cursor = end;
        self.base_exp += out.len();
        Some(out)
    }

    fn base_exp(&self) -> usize {
        self.base_exp
    }
}

/// Exponentiation by squaring `base^exp` for field elements.
#[inline]
pub(crate) fn pow_usize(base: F, mut exp: usize) -> F {
    let mut acc = F::one();
    let mut b = base;
    while exp > 0 {
        if (exp & 1) == 1 {
            acc = acc * b;
        }
        b = b * b;
        exp >>= 1;
    }
    acc
}

/// Horner evaluation over tiles: compute `∑_i a_i z^i` without storing `a_i`.
///
/// This folds each tile with Horner (high→low within the tile) to produce a
/// tile polynomial `local(z)`, and then accounts for the global exponent shift
/// by multiplying with the current `pow = z^{total_len_so_far}`.
///
/// Memory usage is O(tile_len). The stream is consumed once.
pub fn horner_eval_stream<S: CoeffTileStream>(mut s: S, z: F) -> F {
    let mut acc = F::zero();
    let mut pow = F::one();

    while let Some(tile) = s.next_tile() {
        // Horner within the tile: evaluate a_{t-1} ... a_0 at `z`
        let mut local = F::zero();
        for &ai in tile.iter().rev() {
            local = local * z + ai;
        }
        acc = acc + pow * local;
        // Advance pow by tile length: pow *= z^{tile.len()}
        pow = pow * pow_usize(z, tile.len());
    }
    acc
}

// (Optional barycentric path left out intentionally; Horner suffices here.)

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_math_counts_and_bounds() {
        // T=10, b=4 ⇒ blocks: [0..4), [4..8), [8..10)
        let (t, b) = (10usize, 4usize);
        let cnt = block_count_r(t, b).unwrap();
        assert_eq!(cnt, 3);

        let (s0, e0) = block_bounds_r(BlockIdx(0), t, b).unwrap();
        assert_eq!((s0.as_usize(), e0.as_usize()), (0, 4));

        let (s1, e1) = block_bounds_r(BlockIdx(1), t, b).unwrap();
        assert_eq!((s1.as_usize(), e1.as_usize()), (4, 8));

        let (s2, e2) = block_bounds_r(BlockIdx(2), t, b).unwrap();
        assert_eq!((s2.as_usize(), e2.as_usize()), (8, 10));

        assert!(block_bounds_r(BlockIdx(3), t, b).is_err());

        // Result iterator matches direct bounds.
        let it = blocks_r(t, b).unwrap();
        let got: Vec<_> = it
            .map(|(bi, s, e)| (bi.0, s.as_usize(), e.as_usize()))
            .collect();
        assert_eq!(got, vec![(0, 0, 4), (1, 4, 8), (2, 8, 10)]);
    }

    #[test]
    fn chunks_low_high_and_high_low() {
        let data: Vec<u32> = (0..10).collect();

        let tiles_lh: Vec<Vec<u32>> =
            chunks_low_to_high(&data, 3).map(|s| s.to_vec()).collect();
        assert_eq!(tiles_lh, vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9]]);

        let tiles_hl: Vec<Vec<u32>> =
            chunks_high_to_low(&data, 3).map(|s| s.to_vec()).collect();
        assert_eq!(tiles_hl, vec![vec![7, 8, 9], vec![4, 5, 6], vec![1, 2, 3], vec![0]]);
    }

    #[test]
    fn slice_tile_stream_and_horner() {
        // a(x) = 1 + 2x + 3x^2 + 4x^3 over BN254.Fr (deterministic elements)
        let a = [F::from(1u64), F::from(2u64), F::from(3u64), F::from(4u64)];
        let z = F::from(7u64);

        // Evaluate naively: sum a_i * z^i
        let mut zpow = F::one();
        let mut naive = F::zero();
        for ai in a.iter() {
            naive += *ai * zpow;
            zpow *= z;
        }

        // Evaluate via tiles (tile_len=2)
        let s = SliceTileStream::new(&a, 2);
        let tiled = horner_eval_stream(s, z);
        assert_eq!(naive, tiled);
    }

    #[test]
    fn pow_usize_matches_sequential_mul() {
        let z = F::from(5u64);
        for e in 0..32usize {
            let by_func = super::pow_usize(z, e);
            let mut by_loop = F::one();
            for _ in 0..e {
                by_loop *= z;
            }
            assert_eq!(by_func, by_loop);
        }
    }
}
