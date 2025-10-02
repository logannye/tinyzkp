//! Streaming polynomial evaluation helpers
//!
//! These helpers bridge *time-ordered evaluation tiles* and *coefficient tiles*
//! using the canonical `domain::BlockedIfft` façade, so callers can evaluate or
//! open polynomials **without** materializing a full `Vec` of coefficients.
//!
//! ## Guarantees & bounds
//! - **Order:** coefficient tiles are emitted in ascending powers by default
//!   (`a_0..a_{t-1}`, then `a_t..`), with an explicit hi→lo adapter for openings.
//! - **Memory:** live memory is ~`O(b_blk)` (one tile) on the standard path,
//!   and **O(b_blk)** even for large `N` when `SSZKP_BLOCKED_IFFT=1` (the tape
//!   mode). No full coefficient vector is ever stored here.
//! - **Correctness:** we rely on the same IFFT used everywhere else in the
//!   codebase; this module is a light, well-documented adapter.
//!
//! See also: whitepaper §Streaming & Sublinear Space.

#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(unused_imports)]
#![allow(dead_code)]

use std::collections::VecDeque;
use ark_ff::Zero;

use crate::F;
use crate::pcs;
use crate::stream::CoeffTileStream;

// -----------------------------------------------------------------------------
// Blocked-IFFT → Coefficient-Tile Stream (ascending powers, O(b_blk))
// -----------------------------------------------------------------------------

/// Streaming adapter that yields **ascending-power** coefficient tiles produced
/// by `domain::BlockedIfft`. Internally:
/// 1) We run a single pass that *feeds* the time/eval tiles into the façade.
/// 2) We lazily drain `finish_low_to_high()` and return tile **slices**.
///
/// Memory: ~`O(b_blk)` live — we keep only one coefficient tile resident.
///
/// ### Example
/// ```ignore
/// // Given: a pushy producer of eval tiles
/// let coeff_stream = blocked_ifft_witness_stream(&domain, b_blk, |push| {
///     for eval_block in eval_blocks_iter() {
///         push(eval_block);
///     }
/// });
/// // Evaluate at ζ without materializing coefficients:
/// let value = crate::pcs::eval_at_stream(coeff_stream, zeta);
/// ```
struct BlockedIfftCoeffStream<'a, FE>
where
    FE: FnMut(&mut dyn FnMut(Vec<F>)),
{
    domain: &'a crate::domain::Domain,
    tile_len: usize,
    // Producer that will *push* evaluation tiles in time order.
    feed_evals: FE,

    // Lazily created tile iterator from BlockedIfft::finish_low_to_high()
    tiles_it: Option<Box<dyn Iterator<Item = Vec<F>> + 'a>>,

    // Reusable buffer to hold the *current* coefficient tile we expose by ref.
    cur_coeff_tile: Vec<F>,

    // Next base exponent (0, t, 2t, …) for introspection.
    base_exp: usize,

    // Whether we've already constructed the tile iterator.
    primed: bool,
}

impl<'a, FE> BlockedIfftCoeffStream<'a, FE>
where
    FE: FnMut(&mut dyn FnMut(Vec<F>)),
{
    fn new(
        domain: &'a crate::domain::Domain,
        tile_len: usize,
        feed_evals: FE,
    ) -> Self {
        assert!(tile_len > 0, "tile_len must be positive");
        Self {
            domain,
            tile_len,
            feed_evals,
            tiles_it: None,
            cur_coeff_tile: Vec::with_capacity(tile_len),
            base_exp: 0,
            primed: false,
        }
    }

    /// One-time priming: feed all eval tiles into a `BlockedIfft`, then obtain
    /// a **lazy** iterator over coefficient tiles (low→high).
    fn ensure_primed(&mut self) {
        if self.primed {
            return;
        }
        let mut bifft = crate::domain::BlockedIfft::new(self.domain, self.tile_len);
        (self.feed_evals)(&mut |blk: Vec<F>| {
            if !blk.is_empty() {
                bifft.feed_eval_block(&blk);
            }
        });
        self.tiles_it = Some(Box::new(bifft.finish_low_to_high()));
        self.primed = true;
    }

    /// Pull one coefficient tile (ascending power order) into our scratch buf.
    fn next_coeff_tile(&mut self) -> Option<&[F]> {
        self.ensure_primed();
        let it = self.tiles_it.as_mut().expect("tiles iterator present");
        let next = it.next()?;
        self.cur_coeff_tile.clear();
        self.cur_coeff_tile.extend_from_slice(&next);
        let out = &self.cur_coeff_tile[..];
        self.base_exp += out.len();
        Some(out)
    }
}

impl<'a, FE> CoeffTileStream for BlockedIfftCoeffStream<'a, FE>
where
    FE: FnMut(&mut dyn FnMut(Vec<F>)),
{
    /// Returns the next tile of coefficients in ascending power order.
    fn next_tile(&mut self) -> Option<&[F]> {
        self.next_coeff_tile()
    }

    /// Starting exponent of the **next** tile (0, t, 2t, …).
    fn base_exp(&self) -> usize {
        self.base_exp
    }
}

/// Public constructor used by callers:
/// Build a **coefficient tile stream** from a push-style **evaluation-tile**
/// source (time order). Returned tiles are **ascending powers**.
///
/// - `domain`: evaluation domain
/// - `tile_len`: preferred blocked-IFFT tile length (`b_blk`)
/// - `stream_evals`: callback that will **push** eval tiles in time order
pub fn blocked_ifft_witness_stream<'a, FE>(
    domain: &'a crate::domain::Domain,
    tile_len: usize,
    stream_evals: FE,
) -> impl CoeffTileStream + 'a
where
    FE: FnMut(&mut dyn FnMut(Vec<F>)) + 'a,
{
    BlockedIfftCoeffStream::<'a, FE>::new(domain, tile_len, stream_evals)
}

// -----------------------------------------------------------------------------
// Streamed evaluation helpers (Horner over tiles)
// -----------------------------------------------------------------------------

/// Evaluate a streamed polynomial (in **coeff tiles**) at `zeta` **without
/// materializing** the full coefficient vector. This is a thin wrapper around
/// `pcs::eval_at_stream` and your `CoeffTileStream` tiles.
///
/// Memory: ~`O(b_blk)`.
pub fn eval_streamed_at(
    coeff_stream: impl CoeffTileStream,
    zeta: F,
) -> F {
    pcs::eval_at_stream(coeff_stream, zeta)
}

// -----------------------------------------------------------------------------
// Integration shims for the current opening pipeline
// -----------------------------------------------------------------------------

/// Convenience: evaluate the *same* polynomial you’ll open later via the
/// **evaluation tile** source `stream_evals`, but **without** storing all coeffs.
/// Internally builds a `CoeffTileStream` over a blocked-IFFT and folds via Horner.
pub fn eval_from_evals_stream(
    domain: &crate::domain::Domain,
    b_blk: usize,
    mut stream_evals: impl FnMut(&mut dyn FnMut(Vec<F>)),
    zeta: F,
) -> F {
    let coeff_stream = blocked_ifft_witness_stream(domain, b_blk, move |sink| {
        stream_evals(sink)
    });
    eval_streamed_at(coeff_stream, zeta)
}

/// Build **high→low** coefficient tiles from an evaluation-tile stream.
/// Useful for KZG openings that ingest hi→lo tiles.
///
/// Memory: ~`O(b_blk)` live.
/// Tiles are contiguous chunks of the *reversed* coefficient vector.
pub fn coeff_tiles_hi_to_lo_from_eval_stream<'a>(
    domain: &'a crate::domain::Domain,
    b_blk: usize,
    mut stream_evals: impl FnMut(&mut dyn FnMut(Vec<F>)) + 'a,
) -> impl Iterator<Item = Vec<F>> + 'a {
    // Feed time-ordered eval tiles into BlockedIfft, then finish **high→low**.
    let mut bifft = crate::domain::BlockedIfft::new(domain, b_blk);
    stream_evals(&mut |blk: Vec<F>| bifft.feed_eval_block(&blk));
    bifft.finish_high_to_low()
}

/// Open at points from **evaluation streams** by converting each eval-tile to
/// coeff-tiles via the blocked-IFFT façade (still sublinear), then delegating
/// to the coeff-stream opening path in `pcs`.
///
/// Public signature unchanged.
pub fn open_eval_stream_at_points(
    pcs_for_poly: &crate::pcs::PcsParams,
    commitments: &[crate::pcs::Commitment],
    domain: &crate::domain::Domain,
    mut stream_evals: impl FnMut(usize, &mut dyn FnMut(Vec<F>)),
    points: &[F],
) -> Vec<crate::pcs::OpeningProof> {
    // Adapter: for each polynomial index `idx`, emit **hi→lo** coeff tiles.
    let mut as_coeff_hi_to_lo = |idx: usize, sink: &mut dyn FnMut(Vec<F>)| {
        let mut tiles = coeff_tiles_hi_to_lo_from_eval_stream(domain, /*b_blk*/ 1 << 12, |push| {
            stream_evals(idx, push);
        });
        while let Some(tile) = tiles.next() {
            sink(tile);
        }
    };

    crate::pcs::open_at_points_with_coeffs(
        pcs_for_poly,
        commitments,
        |_i, _z| F::zero(),
        &mut as_coeff_hi_to_lo,
        points,
    )
}

// -----------------------------------------------------------------------------
// (Optional) One-shot helper tying eval + open together for a single point
// -----------------------------------------------------------------------------

/// Helper that demonstrates the **new flow** end-to-end:
/// 1) Build a coeff-tile stream via `BlockedIfft`
/// 2) Evaluate via `pcs::eval_at_stream` (no materialization)
/// 3) Produce an opening proof using the streamed coeff path
///
/// Memory: ~`O(b_blk)` live.
pub fn eval_and_open_one_point(
    pcs_for_poly: &crate::pcs::PcsParams,
    commitment: crate::pcs::Commitment,
    domain: &crate::domain::Domain,
    b_blk: usize,
    mut stream_evals: impl FnMut(&mut dyn FnMut(Vec<F>)),
    zeta: F,
) -> (F, crate::pcs::OpeningProof) {
    // Recreate the stream for each pass (cheap + stateless).
    let coeff_stream_eval = blocked_ifft_witness_stream(domain, b_blk, |push| stream_evals(push));
    let value = pcs::eval_at_stream(coeff_stream_eval, zeta);

    // For the proof, use the existing coeff-stream opening path (hi→lo).
    let proofs = open_eval_stream_at_points(
        pcs_for_poly,
        &[commitment],
        domain,
        |_idx, push| stream_evals(push),
        &[zeta],
    );
    (value, proofs.into_iter().next().unwrap())
}

// -----------------------------------------------------------------------------
// Tests (sanity: streamed vs baseline; only compiled in test builds)
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::{Field, One};
    use crate::domain;

    fn small_domain(n: usize) -> domain::Domain {
        let omega = F::get_root_of_unity(n as u64).expect("root");
        domain::Domain { n, omega, zh_c: F::one() }
    }

    #[test]
    fn streamed_eval_matches_direct() {
        let n = 8usize;
        let d = small_domain(n);
        let b_blk = 4;

        // Build a small polynomial in coefficient basis: f(x) = 1 + 2x + 3x^2 + … (len=n)
        let coeffs: Vec<F> = (0..n).map(|i| F::from((i as u64) + 1)).collect();
        // Convert to time-evaluations (on H) to simulate a witness stream.
        let evals = domain::ntt_block_coeffs_to_evals(&d, &coeffs);

        // Push evals in tiles.
        let mut tiles: Vec<Vec<F>> = evals.chunks(b_blk).map(|c| c.to_vec()).collect();

        // Streamed evaluation at some point z (not necessarily on H).
        let z = F::from(7u64);
        let streamed_val = eval_from_evals_stream(&d, b_blk, |push| {
            for t in tiles.drain(..) { push(t); }
        }, z);

        // Direct Horner on full coefficients.
        let mut direct = F::zero();
        for &a in coeffs.iter().rev() {
            direct = direct * z + a;
        }

        assert_eq!(streamed_val, direct);
    }

    #[test]
    fn hi_to_lo_tiles_match_reversed_baseline() {
        let n = 8usize;
        let d = small_domain(n);
        let b_blk = 3;

        // Polynomial & its evaluations
        let coeffs: Vec<F> = (0..n).map(|i| F::from((i as u64) + 1)).collect();
        let evals = domain::ntt_block_coeffs_to_evals(&d, &coeffs);

        // Baseline Q: collect coeffs then reverse and tile
        let mut reversed = coeffs.clone();
        reversed.reverse();
        let mut baseline: Vec<Vec<F>> = reversed.chunks(b_blk).map(|c| c.to_vec()).collect();

        // Streaming hi→lo tiles from eval stream
        let mut tiles = coeff_tiles_hi_to_lo_from_eval_stream(&d, b_blk, |push| {
            for ch in evals.chunks(b_blk) { push(ch.to_vec()); }
        });

        while let Some(tile) = tiles.next() {
            let expect = baseline.remove(0);
            assert_eq!(tile, expect);
        }
        assert!(baseline.is_empty());
    }
}
