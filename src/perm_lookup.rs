//! Permutation & Lookup Accumulators (feature-gated lookups; streaming-ready)
//!
//! This module implements the **multiplicative accumulators** used by the
//! permutation (Plonk-style) and (optionally) lookup arguments. The scheduler
//! must process blocks in **strictly increasing time order** so the products
//! factor correctly by block.
//!
//! ## Transcript / absorption order (whitepaper-aligned)
//! 1. **Wire commitments** are absorbed first.  
//! 2. Sample `(β, γ)` via Fiat–Shamir.  
//! 3. (Optional) absorb `Z`/`Z_L` commitments after `(β, γ)` and **before** `α`.  
//! 4. Sample `α` and proceed to quotient construction.
//!
//! Both permutation and lookup accumulators use the *same* `(β, γ)` challenges.
//!
//! ## Modes
//! - **Baseline (no features):**
//!   - Permutation accumulator `Z` is fully implemented.
//!   - Lookup path is a **no-op** (φₗ(i) ≡ 1), keeping API surface stable.
//! - **Feature `lookups`:**
//!   - Enables a concrete φₗ(i) for a **lookup accumulator** `Z_L`.
//!   - Adds helpers to compute compressed multiplicands from caller-provided
//!     slices (useful if your AIR exposes explicit lookup wiring).
//!   - Adds a streamed commitment helper `commit_lookup_acc_stream` that mirrors
//!     the `Z` flow (no global buffers; Blocked-IFFT → PCS aggregator).
//!
//! All public APIs compile and run identically whether or not `lookups` is
//! enabled; with the feature off, `Z_L` is definitionally the constant-1 column
//! so commitments/openings remain well-defined and cheap.

#![forbid(unsafe_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use ark_ff::{Field, One, Zero};

use crate::F;

// Internal deps used by the streamed commitment helper
use crate::{
    air,
    domain,
    pcs::{self, Aggregator, Basis, PcsParams},
    stream::{blocks, BlockIdx, RegIdx, RowIdx, Restreamer},
};

/// Plonk-style permutation accumulator `Z`.
///
/// In the protocol, `Z(i+1) = Z(i) * φ_perm(i)` where `φ_perm(i)` is the
/// per-row multiplicand derived from the local tuple and challenges `(β, γ)`.
#[derive(Debug, Clone, Copy)]
pub struct PermAcc {
    /// Current accumulator value (start at 1).
    pub z: F,
}

impl PermAcc {
    /// Initialize the permutation accumulator to 1.
    pub fn new() -> Self {
        Self { z: F::one() }
    }
}

/// Compute the per-row permutation multiplicand φ_perm for a given row.
///
/// φ_perm(i) = Π_c (w_c + β·id_c + γ) / Π_c (w_c + β·σ_c + γ)
#[inline]
fn phi_perm_row(row: &crate::air::Locals, beta: F, gamma: F) -> F {
    debug_assert_eq!(row.w_row.len(), row.id_row.len());
    debug_assert_eq!(row.w_row.len(), row.sigma_row.len());

    let mut num = F::one();
    let mut den = F::one();

    for ((&w, &id), &sig) in row
        .w_row
        .iter()
        .zip(row.id_row.iter())
        .zip(row.sigma_row.iter())
    {
        num *= w + beta * id + gamma;
        den *= w + beta * sig + gamma;
    }

    match den.inverse() {
        Some(inv) => num * inv,
        None => F::zero(), // pathological if (β,γ) collide; remains streaming-safe
    }
}

/// Absorb one **block** of rows into the permutation accumulator.
///
/// Time-order: callers **must** process blocks in strictly increasing `t`.
/// Challenges `(β, γ)` **must** be sampled *after* wire commitments and *before*
/// any `Z`/`Z_L` commitment is absorbed into the transcript.
pub fn absorb_block_perm(acc: &mut PermAcc, locals: &[crate::air::Locals], beta: F, gamma: F) {
    for row in locals {
        let phi = phi_perm_row(row, beta, gamma);
        acc.z *= phi;
    }
}

/// **Emit the committed Z-column for a block** and return the final carry.
///
/// Given the start value and block `locals`, returns `(z_vals, carry)` where:
/// - `z_vals[i]` equals `Z` **after** processing the i-th row of the block;
/// - `carry` equals `Z` **after** the **last** row (i.e. the seed for the
///   *next* block). This mirrors the whitepaper’s block-product factoring and
///   avoids off-by-one seeding errors when streaming across blocks.
///
/// This function never allocates more than O(block_len) and performs no I/O.
pub fn emit_z_column_block_carry(
    start: F,
    locals: &[crate::air::Locals],
    beta: F,
    gamma: F,
) -> (Vec<F>, F) {
    let mut out = Vec::with_capacity(locals.len());
    let mut z = start;
    for row in locals {
        let phi = phi_perm_row(row, beta, gamma);
        z *= phi;
        out.push(z);
    }
    (out, z)
}

/// Back-compat wrapper: emit only the block values. The **final carry** is the
/// last element of the returned vector (if any).
pub fn emit_z_column_block(start: F, locals: &[crate::air::Locals], beta: F, gamma: F) -> Vec<F> {
    let (vals, _carry) = emit_z_column_block_carry(start, locals, beta, gamma);
    vals
}

// ============================ Lookup (feature-gated) ============================

/// Lookup accumulator `Z_L` (optional, scheme-dependent).
///
/// For standard lookup arguments, the accumulator evolves multiplicatively by
/// a per-row factor φ_L(i) derived from `Locals` and challenges `(β, γ)`.
#[derive(Debug, Clone, Copy)]
pub struct LookupAcc {
    /// Current lookup accumulator value (start at 1).
    pub z: F,
}

impl LookupAcc {
    /// Initialize the lookup accumulator to 1.
    pub fn new() -> Self {
        Self { z: F::one() }
    }
}

/// A **generic compressed multiplicand** builder for lookup-style accumulators.
///
/// Given *left* and *right* slices for a row (caller-defined), compress them
/// with `(β, γ)` as Π_j ( left_j + β·right_j + γ ). This is useful if your AIR
/// exposes explicit lookup wiring and you want to form a ratio across multiple
/// calls (e.g., multiply with witness terms, divide with table terms).
#[inline]
pub fn phi_lookup_compress(left: &[F], right: &[F], beta: F, gamma: F) -> F {
    debug_assert_eq!(left.len(), right.len());
    let mut acc = F::one();
    for (&l, &r) in left.iter().zip(right.iter()) {
        acc *= l + beta * r + gamma;
    }
    acc
}

/// Convenience helper to build a **fractional** lookup multiplicand:
///   φ_L = Π (LHS_j + β·RHS_j + γ)  /  Π (LHS'_j + β·RHS'_j + γ)
#[inline]
pub fn phi_lookup_fraction(
    lhs: &[F],
    rhs: &[F],
    lhs_den: &[F],
    rhs_den: &[F],
    beta: F,
    gamma: F,
) -> F {
    let num = phi_lookup_compress(lhs, rhs, beta, gamma);
    let den = phi_lookup_compress(lhs_den, rhs_den, beta, gamma);
    match den.inverse() {
        Some(inv) => num * inv,
        None => F::zero(),
    }
}

/// Feature **ON**: φ_L(i) demo wiring using `selectors_row`.
///
/// Convention used across this repo:
///   selectors_row = [table_0..table_{t-1} | (optional) rhs_0..rhs_{t-1}]
/// where `t = min(k, selectors_row.len())`. If RHS is provided, we form a
/// **fractional** multiplicand (numerator uses table, denominator uses rhs),
/// otherwise we use only the numerator compression.
#[cfg(feature = "lookups")]
#[inline]
fn phi_lookup_row(row: &crate::air::Locals, beta: F, gamma: F) -> F {
    let k = row.w_row.len();
    let s = &row.selectors_row;
    if s.is_empty() {
        return F::one();
    }
    let t_len = core::cmp::min(k, s.len());
    let lhs_w = &row.w_row[..t_len];
    let rhs_table = &s[..t_len];

    if s.len() >= 2 * t_len {
        let rhs_den = &s[t_len..(2 * t_len)];
        phi_lookup_fraction(lhs_w, rhs_table, lhs_w, rhs_den, beta, gamma)
    } else {
        phi_lookup_compress(lhs_w, rhs_table, beta, gamma)
    }
}

/// Feature **OFF**: φ_L(i) ≡ 1 (lookup path becomes a no-op).
#[cfg(not(feature = "lookups"))]
#[inline]
fn phi_lookup_row(_row: &crate::air::Locals, _beta: F, _gamma: F) -> F {
    F::one()
}

/// Absorb one **block** into the lookup accumulator (β,γ = 0 in baseline).
///
/// Baseline: no-op unless the `lookups` feature is enabled.
pub fn absorb_block_lookup(acc: &mut LookupAcc, locals: &[crate::air::Locals]) {
    #[allow(unused_variables)]
    for row in locals {
        #[cfg(feature = "lookups")]
        {
            // In baseline protocol text, lookups can be bound with separate
            // challenges; here we default to β=γ=0 unless the caller supplies them.
            let phi = phi_lookup_row(row, F::zero(), F::zero());
            acc.z *= phi;
        }
        #[cfg(not(feature = "lookups"))]
        {
            let _ = row;
        }
    }
}

/// Absorb one **block** into the lookup accumulator using `(β, γ)`.
///
/// Use this variant from the scheduler when you *do* want real lookups
/// (behind the `lookups` feature). With the feature off, φ_L(i) ≡ 1.
pub fn absorb_block_lookup_with_challenges(
    acc: &mut LookupAcc,
    locals: &[crate::air::Locals],
    beta: F,
    gamma: F,
) {
    for row in locals {
        let phi = phi_lookup_row(row, beta, gamma);
        acc.z *= phi;
    }
}

/// Optionally **emit a committed lookup Z_L column** for this block.
///
/// Mirrors `emit_z_column_block_carry` for permutation, but uses the lookup
/// multiplicand φ_L(i). Useful if your protocol commits the lookup
/// accumulator column. Time-order still must be increasing.
pub fn emit_lookup_column_block(
    start: F,
    locals: &[crate::air::Locals],
    beta: F,
    gamma: F,
) -> Vec<F> {
    let mut out = Vec::with_capacity(locals.len());
    let mut z = start;
    for row in locals {
        let phi = phi_lookup_row(row, beta, gamma);
        z *= phi;
        out.push(z);
    }
    out
}

// -----------------------------------------------------------------------------
// Fully-streamed commitment helper for Z_L (mirrors Z path)
// -----------------------------------------------------------------------------

/// Build a commitment to the **lookup accumulator column `Z_L`** by streaming
/// time-ordered rows once, converting to coefficient tiles via a blocked-IFFT,
/// and feeding tiles directly to the PCS aggregator. **No global materialization**
/// of either the time column or the coefficient vector.
///
/// This compiles and runs identically with or without `--features lookups`.
/// With the feature disabled, the column is constant-1 (still well-formed).
pub fn commit_lookup_acc_stream<R: Restreamer<Item = air::Row>>(
    air: &air::AirSpec,
    rs: &R,
    domain: &domain::Domain,
    pcs_degree_ctx: &PcsParams,
    b_blk: usize,
    beta: F,
    gamma: F,
    poly_id: &'static str,
) -> pcs::Commitment {
    // 0) Sanity: trivial guard helps catch misconfigurations early.
    debug_assert!(b_blk > 0, "b_blk must be positive");

    // 1) Time → coeff tiles via a Blocked IFFT
    let mut bifft = domain::BlockedIfft::new(domain, b_blk);

    let t_rows = rs.len_rows();
    let mut boundary = vec![F::zero(); air.k].into_boxed_slice();
    let mut z_start = F::one();

    for (BlockIdx(t), start, end) in blocks(t_rows, b_blk) {
        let it = rs.stream_rows(start, end);
        let br = air::eval_block(air, RegIdx(0), BlockIdx(t), &boundary, it);

        // Emit the Z_L time column for this block and carry the exact seed
        // into the next block (no recomputation).
        let (z_l_block, carry) = {
            let vals = emit_lookup_column_block(z_start, &br.locals, beta, gamma);
            let next = vals.last().copied().unwrap_or(z_start);
            (vals, next)
        };
        z_start = carry;

        bifft.feed_eval_block(&z_l_block);
        boundary = br.boundary_out;
    }

    // 2) Aggregate coefficient tiles into a PCS commitment
    let pcs_for_commit = PcsParams { basis: Basis::Coefficient, ..pcs_degree_ctx.clone() };
    let mut agg = Aggregator::new(&pcs_for_commit, poly_id);
    for tile in bifft.finish_low_to_high() {
        agg.add_block_coeffs(&tile);
    }
    agg.finalize()
}
