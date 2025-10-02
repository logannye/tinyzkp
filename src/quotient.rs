//! Streaming Quotient Builder (whitepaper-correct façade)
//!
//! We construct `Q` such that `R(X) = (X^N − c)·Q(X) + Rem(X)` with `deg(Rem) < N`.
//!
//! ## What’s new here
//! - We keep the **result-returning** builders and the stable streamed entry
//!   point `build_and_commit_quotient_streamed_r`.
//! - We add a **tile-native builder** that emits Q-coefficients directly to the
//!   PCS aggregator in **high→low tiles**, avoiding a `Vec<Q>` buffer:
//!   [`build_and_commit_quotient_streamed_tile_native_r`].
//! - We expose a helper used by openings to obtain Q coefficient tiles
//!   **high→low** directly from a residual evaluation stream:
//!   [`stream_q_coeff_tiles_hi_to_lo_from_r_stream`].
//!
//! ## Memory bound (exact)
//! Let `b_blk` be the working tile size (typically `≈ √N`).
//!
//! - The builders’ **accumulators** (Q tile buffer, MSM temporary storage,
//!   and any per-tile scratch) are **O(b_blk)**.
//! - The legacy path that materializes `R`’s coefficients keeps **O(N)** live
//!   field elements. If you compile/run with the optional blocked-IFFT path
//!   (`SSZKP_BLOCKED_IFFT=1`), the transform itself is out-of-core; the
//!   remaining O(N) comes only from holding `R` coefficients for the final
//!   long-division. (A tape-based in-place long-division can eliminate this;
//!   its façade remains future-work without changing these APIs.)
//!
//! All public behaviors are unchanged; these additions are strictly additive.

#![forbid(unsafe_code)]
#![allow(missing_docs)]

use ark_ff::Zero;

use crate::{domain, pcs, F};

/// Errors surfaced by the quotient builder.
#[derive(Debug, thiserror::Error)]
pub enum QuotientError {
    #[error(transparent)]
    Domain(#[from] crate::domain::DomainError),
}

/// Long-division by X^N − c on **low→high** coefficients.
///
/// Input: `r_lo_to_hi` represents `R(X)` as `∑_{i=0}^{deg} r_i X^i`.
/// Output: `q_lo_to_hi` represents `Q(X)` as `∑_{j=0}^{deg-N} q_j X^j`.
///
/// This is the standard in-place recurrence:
/// ```text
/// for i = deg .. N:
///    q_{i-N} += r_i
///    r_{i-N} += c * r_i
///    r_i      = 0
/// ```
/// We implement it on a local copy of `r` and return only the `q` vector.
pub fn long_divide_xn_minus_c_lo_to_hi(r_lo_to_hi: &[F], n: usize, c: F) -> Vec<F> {
    if r_lo_to_hi.is_empty() {
        return Vec::new();
    }
    let mut r = r_lo_to_hi.to_vec();
    let mut q: Vec<F> = Vec::new();

    // Process from high degree down.
    let mut i = r.len().saturating_sub(1);
    loop {
        if i + 1 <= n {
            break; // degree < N → done
        }
        let coeff = r[i];
        if !coeff.is_zero() {
            let qi = i - n;
            if q.len() <= qi {
                q.resize(qi + 1, F::zero());
            }
            q[qi] += coeff;         // q_{i-N} += r_i
            r[qi] += c * coeff;     // r_{i-N} += c * r_i
            r[i] = F::zero();       // clear r_i
        }
        if i == 0 { break; }
        i -= 1;
    }

    while q.last().map_or(false, |x| x.is_zero()) {
        q.pop();
    }
    q
}

/// Behavior-preserving builder (Result-returning).
///
/// This collects `R`’s evaluations into a single `Vec`, performs an IFFT to get
/// `R`’s coefficients (low→high), runs the long-division by `X^N−c`, and streams
/// `Q` to the PCS in moderately sized tiles.
pub fn build_and_commit_quotient_r<'a>(
    domain: &domain::Domain,
    pcs: &'a pcs::PcsParams,
    _alpha: F,
    _beta: F,
    _gamma: F,
    stream_r_rows: impl Iterator<Item = F>,
) -> Result<pcs::Commitment, QuotientError> {
    let n = domain.n;

    // Collect/resize to N evaluations (time-order), global IFFT → R coeffs.
    let mut evals_r: Vec<F> = stream_r_rows.collect();
    if evals_r.len() < n { evals_r.resize(n, F::zero()); }
    else if evals_r.len() > n { evals_r.truncate(n); }

    let r_coeffs_lo_to_hi = domain::ifft_block_evals_to_coeffs_r(domain, &evals_r)?;

    // Long-divide by X^N − c.
    let q_coeffs_lo_to_hi = long_divide_xn_minus_c_lo_to_hi(&r_coeffs_lo_to_hi, n, domain.zh_c);

    // Stream Q coefficients to PCS in tiles.
    let mut agg = pcs::Aggregator::new(pcs, "Q");
    const TILE: usize = 1 << 12;
    for chunk in q_coeffs_lo_to_hi.chunks(TILE) {
        agg.add_block_coeffs(chunk);
    }
    Ok(agg.finalize())
}

/// Back-compat wrapper (panics on error).
pub fn build_and_commit_quotient<'a>(
    domain: &domain::Domain,
    pcs: &'a pcs::PcsParams,
    _alpha: F,
    _beta: F,
    _gamma: F,
    stream_r_rows: impl Iterator<Item = F>,
) -> pcs::Commitment {
    build_and_commit_quotient_r(domain, pcs, _alpha, _beta, _gamma, stream_r_rows)
        .expect("quotient build failed")
}

/// **Streamed** builder (Result-returning; public API is stable).
///
/// Uses the [`domain::BlockedIfft`] façade to produce `R`’s coefficients in
/// tiles, then performs the same long-division and streams `Q` to the PCS.
/// Accumulator memory is **O(b_blk)**.
pub fn build_and_commit_quotient_streamed_r<'a>(
    domain: &domain::Domain,
    pcs: &'a pcs::PcsParams,
    _alpha: F,
    _beta: F,
    _gamma: F,
    b_blk: usize,
    stream_r_rows: impl Iterator<Item = F>,
) -> Result<pcs::Commitment, QuotientError> {
    // 1) Produce R coefficients (low→high) using BlockedIfft façade.
    let mut bifft = domain::BlockedIfft::new(domain, b_blk);

    // Feed time-ordered residual evaluations into the façade in b_blk-sized chunks.
    let mut buf: Vec<F> = Vec::with_capacity(b_blk);
    for r in stream_r_rows {
        buf.push(r);
        if buf.len() == b_blk {
            bifft.feed_eval_block(&buf);
            buf.clear();
        }
    }
    if !buf.is_empty() {
        bifft.feed_eval_block(&buf);
    }

    // Collect the **low→high** coefficient tiles into a single vector (behavior-preserving).
    let mut r_coeffs_lo_to_hi: Vec<F> = Vec::with_capacity(domain.n);
    for tile in bifft.finish_low_to_high() {
        r_coeffs_lo_to_hi.extend_from_slice(&tile);
    }
    if r_coeffs_lo_to_hi.len() < domain.n {
        r_coeffs_lo_to_hi.resize(domain.n, F::zero());
    } else if r_coeffs_lo_to_hi.len() > domain.n {
        r_coeffs_lo_to_hi.truncate(domain.n);
    }

    // 2) Fold-down by X^N − c to obtain Q (low→high).
    let q_coeffs_lo_to_hi = long_divide_xn_minus_c_lo_to_hi(&r_coeffs_lo_to_hi, domain.n, domain.zh_c);

    // 3) Stream Q coefficients to PCS as tiles (low→high).
    let mut agg = pcs::Aggregator::new(pcs, "Q");
    const TILE: usize = 1 << 12;
    for chunk in q_coeffs_lo_to_hi.chunks(TILE) {
        agg.add_block_coeffs(chunk);
    }
    Ok(agg.finalize())
}

/// **Tile-native** builder over the residual stream (high→low emission).
///
/// This variant avoids allocating a `Vec<Q>` by performing the long-division
/// *in place* on a local `R` buffer and emitting **high→low tiles** directly
/// into the PCS aggregator. The PCS ingests these tiles with
/// `CoeffTileOrder::HighToLow`.
///
/// Memory:
/// - `R` coefficients: O(N) (legacy in-mem path).
/// - Q tile buffer + MSM scratch: **O(b_blk)**.
///
/// Note: when `SSZKP_BLOCKED_IFFT=1`, the IFFT itself is out-of-core; this
/// function still holds `R` locally to perform the fold-down. A future taped
/// fold-down would remove that O(N) as well without changing this API.
pub fn build_and_commit_quotient_streamed_tile_native_r<'a>(
    domain: &domain::Domain,
    pcs: &'a pcs::PcsParams,
    _alpha: F,
    _beta: F,
    _gamma: F,
    b_blk: usize,
    stream_r_rows: impl Iterator<Item = F>,
) -> Result<pcs::Commitment, QuotientError> {
    // IFFT to get R coefficients (low→high), as in the streamed builder.
    let mut bifft = domain::BlockedIfft::new(domain, b_blk);
    let mut buf: Vec<F> = Vec::with_capacity(b_blk);
    for r in stream_r_rows {
        buf.push(r);
        if buf.len() == b_blk {
            bifft.feed_eval_block(&buf);
            buf.clear();
        }
    }
    if !buf.is_empty() {
        bifft.feed_eval_block(&buf);
    }
    let mut r_coeffs: Vec<F> = Vec::with_capacity(domain.n);
    for tile in bifft.finish_low_to_high() {
        r_coeffs.extend_from_slice(&tile);
    }
    if r_coeffs.len() < domain.n { r_coeffs.resize(domain.n, F::zero()); }
    else if r_coeffs.len() > domain.n { r_coeffs.truncate(domain.n); }

    // Tile-native emission: iterate i from high→low and emit q_{i-N} on the fly.
    use crate::domain::CoeffTileOrder;
    let mut agg = pcs::Aggregator::new(pcs, "Q");
    let mut q_tile_hi_to_lo: Vec<F> = Vec::with_capacity(b_blk);

    if r_coeffs.len() > domain.n {
        r_coeffs.truncate(domain.n);
    }
    // i runs [len-1 .. N]
    let mut i = r_coeffs.len().saturating_sub(1);
    loop {
        if i + 1 <= domain.n { break; } // done when degree < N

        let coeff = r_coeffs[i];
        if !coeff.is_zero() {
            // Emit q_{i-N} (in high→low order).
            q_tile_hi_to_lo.push(coeff);
            // Apply fold-down effect to r_{i-N}.
            let qi = i - domain.n;
            r_coeffs[qi] += domain.zh_c * coeff;
            // clear (not strictly required since we won’t touch r[i] again)
            // r_coeffs[i] = F::zero();
        }

        // Flush tile if full.
        if q_tile_hi_to_lo.len() == b_blk {
            // Aggregator can ingest **high→low** tiles.
            agg.add_coeff_tile(&q_tile_hi_to_lo, CoeffTileOrder::HighToLow)
                .expect("aggregator add tile");
            q_tile_hi_to_lo.clear();
        }

        if i == 0 { break; }
        i -= 1;
    }

    // Flush any remaining Q coeffs in the last (short) tile.
    if !q_tile_hi_to_lo.is_empty() {
        agg.add_coeff_tile(&q_tile_hi_to_lo, CoeffTileOrder::HighToLow)
            .expect("aggregator add tail tile");
    }

    Ok(agg.finalize())
}

/// Helper for openings: return **Q coefficient tiles high→low** from a residual
/// time stream. The iterator yields contiguous **high→low** tiles, each of size
/// at most `b_blk`.
pub fn stream_q_coeff_tiles_hi_to_lo_from_r_stream<'a>(
    domain: &domain::Domain,
    b_blk: usize,
    stream_r_rows: impl Iterator<Item = F> + 'a,
) -> impl Iterator<Item = Vec<F>> + 'a {
    // 1) Use BlockedIfft to derive **low→high** coefficient tiles for R.
    let mut bifft = domain::BlockedIfft::new(domain, b_blk);

    // Feed time-ordered residual evaluations (chunked).
    let mut buf: Vec<F> = Vec::with_capacity(b_blk);
    let mut it = stream_r_rows;
    while let Some(x) = it.next() {
        buf.push(x);
        if buf.len() == b_blk {
            bifft.feed_eval_block(&buf);
            buf.clear();
        }
    }
    if !buf.is_empty() {
        bifft.feed_eval_block(&buf);
    }

    // Gather R coeffs (low→high) exactly to length N.
    let mut r_coeffs_lo_to_hi: Vec<F> = Vec::with_capacity(domain.n);
    for tile in bifft.finish_low_to_high() {
        r_coeffs_lo_to_hi.extend_from_slice(&tile);
    }
    if r_coeffs_lo_to_hi.len() < domain.n {
        r_coeffs_lo_to_hi.resize(domain.n, F::zero());
    } else if r_coeffs_lo_to_hi.len() > domain.n {
        r_coeffs_lo_to_hi.truncate(domain.n);
    }

    // 2) Fold-down → Q (low→high), then reverse to high→low for emission.
    let mut q_hi_to_lo: Vec<F> =
        long_divide_xn_minus_c_lo_to_hi(&r_coeffs_lo_to_hi, domain.n, domain.zh_c);
    q_hi_to_lo.reverse();

    // 3) Yield high→low tiles of size ≤ b_blk.
    struct QRevIter {
        coeffs_hi_to_lo: Vec<F>,
        idx: usize,
        tile: usize,
    }
    impl Iterator for QRevIter {
        type Item = Vec<F>;
        fn next(&mut self) -> Option<Self::Item> {
            if self.idx >= self.coeffs_hi_to_lo.len() { return None; }
            let end = (self.idx + self.tile).min(self.coeffs_hi_to_lo.len());
            let out = self.coeffs_hi_to_lo[self.idx..end].to_vec();
            self.idx = end;
            Some(out)
        }
    }
    QRevIter { coeffs_hi_to_lo: q_hi_to_lo, idx: 0, tile: b_blk }
}
