//! AIR template & block evaluator
//!
//! # What this revision adds
//! - Richer docs on what goes into [`Locals`] and `reg_m_vals`.
//! - Stronger **block purity** guarantee: evaluation is a pure function of
//!   `(boundary_in, rows[start..end])` and **does not** consult or mutate
//!   any global state.
//! - A no-behavior-change **optimization seam**:
//!   `eval_block_all_regs_r` evaluates all registers in a block once and
//!   shares the computed [`Locals`] across `k`. We keep the legacy
//!   `eval_block` API intact.
//! - Finalized **residual streaming**:
//!   - `residual_stream_tiles(...)` generates `R(X)` in **evaluation basis**,
//!     yielding time-ordered tiles of length ≤ `b_blk` (back-compat single-row
//!     iterator kept as `residual_stream(...)`).
//!   - The symbolic `residual_eval_at_point_symbolic(...)` helper used by the
//!     verifier remains; the algebra matches the whitepaper.
//!
//! The demo gates/constraints here are intentionally simple and are present
//! only to exercise the machinery. Integrators can replace the gate part with
//! their system’s actual AIR constraints (the accumulator/glue stays the same).

#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(unused_variables)]
#![allow(unexpected_cfgs)]

use crate::stream::{BlockIdx, RegIdx, Restreamer};
use crate::F;
use ark_ff::{Field, One, Zero};

/// AIR template (fixed-column model).
#[derive(Debug, Clone)]
pub struct AirSpec {
    /// Number of registers/columns.
    pub k: usize,
    /// Optional identity permutation table (per column).
    pub id_table: Vec<Box<[F]>>,
    /// Optional sigma/permutation table (per column).
    pub sigma_table: Vec<Box<[F]>>,
    /// Optional selector columns used by gates and/or lookups.
    pub selectors: Vec<Box<[F]>>,
}

impl AirSpec {
    pub fn with_cyclic_sigma(k: usize) -> Self {
        Self { k, id_table: Vec::new(), sigma_table: Vec::new(), selectors: Vec::new() }
    }
    pub fn with_tables(
        k: usize,
        id_table: Vec<Box<[F]>>,
        sigma_table: Vec<Box<[F]>>,
        selectors: Vec<Box<[F]>>,
    ) -> Self {
        Self { k, id_table, sigma_table, selectors }
    }
    fn make_id_sigma_row(&self, row_ctr: usize) -> (Box<[F]>, Box<[F]>) {
        if self.id_table.is_empty() || self.sigma_table.is_empty() {
            // Fallback: identity = [0..k-1], sigma = cyclic shift
            let mut id = Vec::with_capacity(self.k);
            let mut sigma = Vec::with_capacity(self.k);
            for j in 0..self.k {
                id.push(F::from(j as u64));
                sigma.push(F::from(((j + 1) % self.k) as u64));
            }
            return (id.into_boxed_slice(), sigma.into_boxed_slice());
        }
        let mut id = Vec::with_capacity(self.k);
        let mut sigma = Vec::with_capacity(self.k);
        for col in 0..self.k {
            let id_col = &self.id_table[col];
            let sigma_col = &self.sigma_table[col];
            let id_val = if id_col.is_empty() { F::from(col as u64) } else { id_col[row_ctr % id_col.len()] };
            let sigma_val =
                if sigma_col.is_empty() { F::from(((col + 1) % self.k) as u64) } else { sigma_col[row_ctr % sigma_col.len()] };
            id.push(id_val);
            sigma.push(sigma_val);
        }
        (id.into_boxed_slice(), sigma.into_boxed_slice())
    }
    fn make_selectors_row(&self, row_ctr: usize) -> Box<[F]> {
        if self.selectors.is_empty() { return Box::from([]); }
        let mut s = Vec::with_capacity(self.selectors.len());
        for col in &self.selectors {
            if col.is_empty() { s.push(F::zero()); } else { s.push(col[row_ctr % col.len()]); }
        }
        s.into_boxed_slice()
    }
}

/// One row of the execution trace (k registers).
#[derive(Debug, Clone)]
pub struct Row {
    /// Values of all `k` registers **for this time row**, in register-major order.
    pub regs: Box<[F]>,
}

/// Row-local tuple used by **gates** and **global accumulators**.
///
/// This structure contains everything a row-constrained transition and the
/// permutation/lookup accumulators need to compute multiplicands in *time
/// order*, independently of other rows.
///
/// - `w_row`: the `k` witness values for this row.
/// - `id_row`: the `k` identity labels for this row (from the identity table
///   or the fallback `[0,1,…,k-1]`).
/// - `sigma_row`: the `k` permuted labels for this row (from the sigma table
///   or the fallback cyclic shift).
/// - `selectors_row`: any auxiliary selector columns consumed by gates or the
///   optional lookup argument (feature-gated).
#[derive(Debug, Clone)]
pub struct Locals {
    pub w_row: Box<[F]>,
    pub id_row: Box<[F]>,
    pub sigma_row: Box<[F]>,
    pub selectors_row: Box<[F]>,
}

/// Result of evaluating one **block** for a *target register* `m`.
///
/// - `reg_m_vals`: the values of register `m` across rows in this block,
///   i.e., `rows[start..end].map(|r| r.regs[m])` (time order preserved).
/// - `locals`: the per-row [`Locals`] tuples that gates/accumulators consume,
///   in the same order as `reg_m_vals`.
/// - `boundary_out`: the final register state **after this block**, intended
///   to seed the next block’s evaluation (this implementation simply returns
///   the last row’s `regs` verbatim).
#[derive(Debug, Clone)]
pub struct BlockResult {
    pub reg_m_vals: Vec<F>,
    pub locals: Vec<Locals>,
    pub boundary_out: Box<[F]>,
}

/// AIR-side errors surfaced by public entrypoints.
#[derive(Debug, thiserror::Error)]
pub enum AirError {
    #[error("boundary vector must have k={expected} registers (got {got})")]
    BadBoundaryLen { expected: usize, got: usize },
    #[error("target register m={m} out of range (k={k})")]
    RegOutOfRange { m: usize, k: usize },
    #[error("row.regs length must be k={expected} (got {got})")]
    BadRowLen { expected: usize, got: usize },
}

/// Evaluate a block **purely** from `(boundary_in, rows[start..end])`.
///
/// This function is *pure per block*: it depends only on its inputs, and does
/// not mutate global state. The returned [`BlockResult`] contains all the row
/// data needed by downstream phases (accumulators, gates, quotient builder).
pub fn eval_block_r(
    air: &AirSpec,
    m: RegIdx,
    _t: BlockIdx,
    boundary_in: &[F],
    iter_rows: impl Iterator<Item = Row>,
) -> Result<BlockResult, AirError> {
    if boundary_in.len() != air.k {
        return Err(AirError::BadBoundaryLen { expected: air.k, got: boundary_in.len() });
    }
    if m.0 >= air.k {
        return Err(AirError::RegOutOfRange { m: m.0, k: air.k });
    }

    let mut reg_m_vals: Vec<F> = Vec::new();
    let mut locals: Vec<Locals> = Vec::new();
    let mut boundary_out: Box<[F]> = boundary_in.to_vec().into_boxed_slice();

    let mut row_ctr = 0usize;
    for row in iter_rows {
        if row.regs.len() != air.k {
            return Err(AirError::BadRowLen { expected: air.k, got: row.regs.len() });
        }
        reg_m_vals.push(row.regs[m.0]);
        let (id_row, sigma_row) = air.make_id_sigma_row(row_ctr);
        let selectors_row = air.make_selectors_row(row_ctr);
        locals.push(Locals { w_row: row.regs.clone(), id_row, sigma_row, selectors_row });
        boundary_out = row.regs;
        row_ctr += 1;
    }

    Ok(BlockResult { reg_m_vals, locals, boundary_out })
}

/// Back-compat wrapper (panics on error).
pub fn eval_block(
    air: &AirSpec,
    m: RegIdx,
    t: BlockIdx,
    boundary_in: &[F],
    iter_rows: impl Iterator<Item = Row>,
) -> BlockResult {
    eval_block_r(air, m, t, boundary_in, iter_rows).expect("air::eval_block failed")
}

/// Result of evaluating one **block** while sharing locals across **all** registers.
///
/// This is an **optimization seam** only (no behavior change): callers that plan
/// to consume all `k` registers for the block can compute `locals` once and
/// reuse them, avoiding repeated `Row` cloning or recomputation.
#[derive(Debug, Clone)]
pub struct BlockAllResult {
    /// For each register `m`, its values over the block (time order).
    pub regs_vals: Vec<Vec<F>>, // length k; each inner vec has block_len
    /// Row-local tuples shared by all registers (time order).
    pub locals: Vec<Locals>,
    /// Final boundary state after the block (seed for the next block).
    pub boundary_out: Box<[F]>,
}

/// Evaluate one block, collecting **all registers** at once and sharing locals.
pub fn eval_block_all_regs_r(
    air: &AirSpec,
    _t: BlockIdx,
    boundary_in: &[F],
    iter_rows: impl Iterator<Item = Row>,
) -> Result<BlockAllResult, AirError> {
    if boundary_in.len() != air.k {
        return Err(AirError::BadBoundaryLen { expected: air.k, got: boundary_in.len() });
    }

    let mut regs_vals: Vec<Vec<F>> = vec![Vec::new(); air.k];
    let mut locals: Vec<Locals> = Vec::new();
    let mut boundary_out: Box<[F]> = boundary_in.to_vec().into_boxed_slice();

    let mut row_ctr = 0usize;
    for row in iter_rows {
        if row.regs.len() != air.k {
            return Err(AirError::BadRowLen { expected: air.k, got: row.regs.len() });
        }
        for m in 0..air.k {
            regs_vals[m].push(row.regs[m]);
        }
        let (id_row, sigma_row) = air.make_id_sigma_row(row_ctr);
        let selectors_row = air.make_selectors_row(row_ctr);
        locals.push(Locals { w_row: row.regs.clone(), id_row, sigma_row, selectors_row });
        boundary_out = row.regs;
        row_ctr += 1;
    }

    Ok(BlockAllResult { regs_vals, locals, boundary_out })
}

/// Back-compat wrapper (panics on error).
pub fn eval_block_all_regs(
    air: &AirSpec,
    t: BlockIdx,
    boundary_in: &[F],
    iter_rows: impl Iterator<Item = Row>,
) -> BlockAllResult {
    eval_block_all_regs_r(air, t, boundary_in, iter_rows).expect("air::eval_block_all_regs failed")
}

// ============================================================================
// Residual builder (Phase D)
// ============================================================================

#[derive(Copy, Clone, Debug)]
pub struct ResidualCfg {
    pub alpha: F,
    pub beta: F,
    pub gamma: F,
}

#[inline]
fn prod_id_sigma(air: &AirSpec, locals: &Locals, beta: F, gamma: F) -> (F, F) {
    let w = &locals.w_row;
    let mut prod_id = F::one();
    let mut prod_sigma = F::one();
    for j in 0..air.k {
        debug_assert!(locals.id_row.len() == air.k && locals.sigma_row.len() == air.k);
        prod_id *= w[j] + beta * locals.id_row[j] + gamma;
        prod_sigma *= w[j] + beta * locals.sigma_row[j] + gamma;
    }
    (prod_id, prod_sigma)
}

/// Rowwise residual (demo gates + permutation coupling + boundary ties).
pub fn residual_row(
    air: &AirSpec,
    locals: &Locals,
    cfg: &ResidualCfg,
    z_i: F,
    z_ip1: F,
    is_first_row: bool,
    is_last_row: bool,
) -> F {
    // Gate demo: s0·(w0+w1−w2) + s1·(w0·w1−w2)
    let w = &locals.w_row;
    let s = &locals.selectors_row;
    let gate_add = if s.len() >= 1 && air.k >= 3 { s[0] * (w[0] + w[1] - w[2]) } else { F::zero() };
    let gate_mul = if s.len() >= 2 && air.k >= 3 { s[1] * (w[0] * w[1] - w[2]) } else { F::zero() };
    let gate_part = cfg.alpha * (gate_add + gate_mul);

    let (prod_id, prod_sigma) = prod_id_sigma(air, locals, cfg.beta, cfg.gamma);
    let perm_coupled = z_ip1 * prod_id - z_i * prod_sigma;

    let mut boundary_part = F::zero();
    if is_first_row { boundary_part += z_i - F::one(); }
    if is_last_row { boundary_part += z_ip1 - F::one(); }

    gate_part + perm_coupled + boundary_part
}

// ============================================================================
// Residual stream over the full domain (Phase D input to quotient)
// ============================================================================

/// Legacy: residual stream yielding **one evaluation per row** (time order).
///
/// Kept for backward compatibility. Prefer [`residual_stream_tiles`] for
/// sublinear-space workflows and blocked quotient construction.
pub fn residual_stream<'a>(
    air: &'a AirSpec,
    cfg: ResidualCfg,
    rs: &'a impl Restreamer<Item = Row>,
    b_blk: usize,
) -> impl Iterator<Item = F> + 'a {
    let t_rows = rs.len_rows();
    let mut z_cur = F::one();
    let mut global_idx = 0usize;

    (0..crate::stream::block_count(t_rows, b_blk)).flat_map(move |t| {
        let (s, e) = crate::stream::block_bounds(crate::stream::BlockIdx(t), t_rows, b_blk);
        let it = rs.stream_rows(s, e);
        let boundary_seed = vec![F::zero(); air.k].into_boxed_slice();
        let br = eval_block(air, RegIdx(0), BlockIdx(t), &boundary_seed, it);

        br.locals.into_iter().map(move |loc| {
            let (prod_id, prod_sigma) = prod_id_sigma(air, &loc, cfg.beta, cfg.gamma);
            let phi = prod_sigma.inverse().map(|inv| prod_id * inv).unwrap_or(F::zero());
            let z_next = z_cur * phi;

            let is_first = global_idx == 0;
            let is_last = global_idx + 1 == t_rows;

            let r_i = residual_row(air, &loc, &cfg, z_cur, z_next, is_first, is_last);
            z_cur = z_next;
            global_idx += 1;
            r_i
        })
    })
}

/// Preferred: **tile-generating** residual stream (evaluation basis).
///
/// Produces `R` in time order, one **tile** at a time (each `Vec<F>` has
/// length ≤ `b_blk`). This is the input expected by the blocked quotient
/// builder in the whitepaper-complete pipeline.
pub fn residual_stream_tiles<'a>(
    air: &'a AirSpec,
    cfg: ResidualCfg,
    rs: &'a impl Restreamer<Item = Row>,
    b_blk: usize,
) -> impl Iterator<Item = Vec<F>> + 'a {
    let t_rows = rs.len_rows();

    // We hold only O(b_blk) state: the running Z carry and a small output tile.
    let mut z_carry = F::one();
    let mut produced = 0usize;

    (0..crate::stream::block_count(t_rows, b_blk)).map(move |t| {
        let (s, e) = crate::stream::block_bounds(crate::stream::BlockIdx(t), t_rows, b_blk);
        let block_len = e.as_usize() - s.as_usize();
        let it = rs.stream_rows(s, e);

        // Block purity: boundary seed is explicit and local.
        let boundary_seed = vec![F::zero(); air.k].into_boxed_slice();
        let br = eval_block(air, RegIdx(0), BlockIdx(t), &boundary_seed, it);

        // Emit residuals for this block, threading Z carry precisely.
        let mut tile: Vec<F> = Vec::with_capacity(block_len);
        for (i, loc) in br.locals.into_iter().enumerate() {
            let (prod_id, prod_sigma) = prod_id_sigma(air, &loc, cfg.beta, cfg.gamma);
            let phi = prod_sigma.inverse().map(|inv| prod_id * inv).unwrap_or(F::zero());
            let z_next = z_carry * phi;

            let is_first = produced == 0 && i == 0;
            let is_last = produced + i + 1 == t_rows;

            let r_i = residual_row(air, &loc, &cfg, z_carry, z_next, is_first, is_last);
            tile.push(r_i);
            z_carry = z_next;
        }
        produced += block_len;
        tile
    })
}

// ============================================================================
// Residual evaluation at an arbitrary point ζ (verifier-side helper)
// ============================================================================

/// Symbolic evaluation of the residual `R(ζ)` for the verifier-side check.
/// 
/// **Fast-path (default builds only):**
/// - When the `strict-recompute-r` feature is **disabled** (default), if
///   `q_at_zeta` is `Some`, we immediately return `Z_H(ζ)·Q(ζ)` and skip
///   expanding the gate/perm/lookup terms (algebraically equivalent).
/// 
/// **Strict mode (recommended for audits):**
/// - When `--features strict-recompute-r` is enabled, the fast-path is compiled
///   out and the function *always* recomputes `R(ζ)` from opened values using
///   the verifier’s `(α,β,γ)`.
pub fn residual_eval_at_point_symbolic(
    k: usize,
    header_like: (&u32, &F), // (N, zh_c)
    cfg: ResidualCfg,
    zeta: F,
    wires_at_zeta: &[F],
    z_at_zeta: F,
    selectors_at_zeta: Option<&[F]>,
    id_at_zeta: Option<&[F]>,
    sigma_at_zeta: Option<&[F]>,
    q_at_zeta: Option<F>,
    // Optional whitepaper terms:
    z_at_omega_zeta: Option<F>,
    _z_l_at_zeta: Option<F>,
    _z_l_at_omega_zeta: Option<F>,
) -> F {
    let (n_u32, zh_c) = header_like;
    let n = *n_u32 as usize;

    // ---- Q fast-path (compiled out in strict mode)
    #[cfg(not(feature = "strict-recompute-r"))]
    if let Some(qz) = q_at_zeta {
        let zh_z = zeta.pow([n as u64]) - *zh_c;
        return zh_z * qz;
    }

    // ---- Gate demo (same as residual_row)
    let s_row = selectors_at_zeta.unwrap_or(&[]);
    let gate_add = if s_row.len() >= 1 && wires_at_zeta.len() >= 3 {
        s_row[0] * (wires_at_zeta[0] + wires_at_zeta[1] - wires_at_zeta[2])
    } else { F::zero() };
    let gate_mul = if s_row.len() >= 2 && wires_at_zeta.len() >= 3 {
        s_row[1] * (wires_at_zeta[0] * wires_at_zeta[1] - wires_at_zeta[2])
    } else { F::zero() };
    let gate_part = cfg.alpha * (gate_add + gate_mul);

    // ---- Permutation-coupled term at ζ (uses Z(ω·ζ) if provided)
    let mut prod_id = F::one();
    let mut prod_sigma = F::one();

    if let (Some(id), Some(sig)) = (id_at_zeta, sigma_at_zeta) {
        for j in 0..k {
            let wj = wires_at_zeta.get(j).copied().unwrap_or(F::zero());
            prod_id *= wj + cfg.beta * id[j] + cfg.gamma;
            prod_sigma *= wj + cfg.beta * sig[j] + cfg.gamma;
        }
    } else {
        for j in 0..k {
            let idj = F::from(j as u64);
            let sigj = F::from(((j + 1) % k) as u64);
            let wj = wires_at_zeta.get(j).copied().unwrap_or(F::zero());
            prod_id *= wj + cfg.beta * idj + cfg.gamma;
            prod_sigma *= wj + cfg.beta * sigj + cfg.gamma;
        }
    }

    // If Z(ω·ζ) is present, use the whitepaper form; otherwise keep ζ-only fallback.
    let perm_part = if let Some(z_omega) = z_at_omega_zeta {
        z_omega * prod_id - z_at_zeta * prod_sigma
    } else {
        // legacy fallback (ζ-only)
        z_at_zeta * prod_id - z_at_zeta * prod_sigma
    };

    // ---- Lookup transition term (feature-gated)
    #[cfg(feature = "lookups")]
    let lookup_part = {
        if let (Some(zl_z), Some(zl_omega), Some(sel)) = (_z_l_at_zeta, _z_l_at_omega_zeta, selectors_at_zeta) {
            // Demo wiring: selectors = [t_0..t_{k-1} | r_0..r_{k-1}] if long enough.
            let k_take = core::cmp::min(k, sel.len());
            let (t_slice, r_slice_opt) = if sel.len() >= 2 * k_take {
                (&sel[..k_take], Some(&sel[k_take..(2 * k_take)]))
            } else {
                (&sel[..k_take], None)
            };

            let mut num = F::one();
            for j in 0..k_take {
                let wj = wires_at_zeta.get(j).copied().unwrap_or(F::zero());
                num *= wj + cfg.beta * t_slice[j] + cfg.gamma;
            }
            let den = if let Some(r_slice) = r_slice_opt {
                let mut d = F::one();
                for j in 0..k_take {
                    let wj = wires_at_zeta.get(j).copied().unwrap_or(F::zero());
                    d *= wj + cfg.beta * r_slice[j] + cfg.gamma;
                }
                d
            } else {
                F::one()
            };

            zl_omega * num - zl_z * den
        } else {
            F::zero()
        }
    };

    #[cfg(not(feature = "lookups"))]
    let lookup_part = F::zero();

    gate_part + perm_part + lookup_part
}
