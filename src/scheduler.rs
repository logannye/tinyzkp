//! Five-Phase Scheduler (whitepaper-complete, streaming)
//!
//! ## Overview
//! This scheduler wires the prover and verifier so that **all polynomials are
//! handled in a streamed, tile-based fashion**, matching the whitepaper’s
//! sublinear-space design.
//!
//! Key properties
//! - **Z commitment is truly streaming:** No `Vec` of the full Z column. Each
//!   block’s Z values feed a Blocked-IFFT, whose coefficient tiles stream into
//!   the PCS aggregator.
//! - **Quotient builder:** Uses `build_and_commit_quotient_streamed_r` (tile
//!   native), thus never materializing `Q`.
//! - **Openings from tiles only:** Wires, Z, and Q are opened via
//!   Blocked-IFFT-powered coefficient tiles (no global materialization).
//! - **Algebra check is enforced:** A non-zero residual triggers
//!   `VerifySchedError::Algebra` (hard error).
//!
//! ## Opening order (test-invariant)
//! We preserve the opening order required by the existing tests and by the
//! whitepaper narrative:
//!   **[ wires@ζ ] [ Z@ζ? ] [ Q@ζ ] [ Z@ω·ζ? ]**
//!
//! ## Double buffering (prefetch)
//! We expose a small ping–pong helper and use a simple **prefetch** pattern
//! when consuming tiles. This keeps two tiles live at a time so the pipeline
//! can conceptually overlap:
//!   - tile `t` → MSM
//!   - tile `t+1` → ready from IFFT (prefetched)
//!   - tile `t+2` → being read / prepped by the producer
//!
//! The code remains single-threaded; this is a locality/reuse improvement that
//! keeps peak RSS close to `O(b_blk)`.
//!
//! ## Note on per-register recomputation
//! For wire openings we **replay** the time stream per register `m` to build
//! its coefficient tiles. This is correct because the AIR evaluation for a
//! given block is *pure* given `(boundary_in, rows[start..end])`. It trades a
//! second read of blocks for O(√T) memory usage. A future micro-optimization
//! (no API change) could cache `Locals` once per block and share them across
//! all `k` registers in that block to avoid repeated computation.
//!
//! Feature switches
//! - `zeta-shift`: open Z at both ζ and ω·ζ.
//! - `lookups`: stream a lookup accumulator (demo wiring).
//! - `strict-recompute-r`: verifier recomputes R(ζ) (disables Q fast-path).

#![forbid(unsafe_code)]
#![allow(unused_mut)]
#![allow(unused_variables)]
#![allow(missing_docs)]
#![allow(unused_imports)]
#![allow(unused_assignments)]
#![allow(deprecated)]

use ark_ff::{Field, One, Zero};

use crate::{
    air::{self, BlockResult, ResidualCfg},
    pcs::{self, Aggregator, Basis, PcsParams, VerifyError as PcsVerifyError},
    perm_lookup::{
        absorb_block_lookup, absorb_block_lookup_with_challenges, absorb_block_perm,
        emit_z_column_block, LookupAcc, PermAcc,
    },
    quotient::{
        build_and_commit_quotient_streamed_r, stream_q_coeff_tiles_hi_to_lo_from_r_stream,
        QuotientError,
    },
    stream::{blocks, BlockIdx, RegIdx, RowIdx, Restreamer},
    transcript::{FsLabel, Transcript},
    F, Proof, ProofHeader, ProveParams, VerifyParams,
};

type PcsCommit = pcs::Commitment;

// ============================================================================
// Two-tile prefetch helper (ping–pong buffers)
// ============================================================================

/// A minimal ping–pong holder for **tile-sized** buffers.
/// Not tied to any producer; use it to keep two tiles alive and reuse capacity.
pub struct TwoTileBuf<T> {
    pub a: Vec<T>,
    pub b: Vec<T>,
    flip: bool,
}
impl<T> TwoTileBuf<T> {
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self { a: Vec::with_capacity(cap), b: Vec::with_capacity(cap), flip: false }
    }
    /// Return `(cur, next)` mutable buffers (alternate each call).
    #[inline]
    pub fn buffers(&mut self) -> (&mut Vec<T>, &mut Vec<T>) {
        self.flip = !self.flip;
        if self.flip {
            (&mut self.a, &mut self.b)
        } else {
            (&mut self.b, &mut self.a)
        }
    }
    /// Clear both buffers without dropping capacity.
    #[inline]
    pub fn reset(&mut self) {
        self.a.clear();
        self.b.clear();
        self.flip = false;
    }
}

/// Consume an iterator of coefficient **tiles** with a one-tile **prefetch**.
/// This keeps `current` and `next` tiles alive simultaneously.
fn consume_tiles_with_prefetch(
    mut tiles: impl Iterator<Item = Vec<F>>,
    mut consume: impl FnMut(&[F]),
) {
    // Prime the pipeline with one tile.
    let mut current = match tiles.next() {
        Some(t) => t,
        None => return,
    };
    loop {
        // Prefetch the next tile *before* we MSM the current tile.
        let next = tiles.next();
        // MSM / consume tile `current`.
        consume(&current);
        // Move to the next tile, or finish.
        match next {
            Some(n) => current = n,
            None => break,
        }
    }
}

// ============================================================================

pub struct Prover<'a> {
    pub air: &'a air::AirSpec,
    pub params: &'a ProveParams,
}

pub struct Verifier<'a> {
    pub params: &'a VerifyParams,
}

#[derive(Debug, thiserror::Error)]
pub enum ProveError {
    #[error("invalid parameters: {0}")]
    Params(&'static str),
    #[error(transparent)]
    Quotient(#[from] QuotientError),
}

#[derive(Debug, thiserror::Error)]
pub enum VerifySchedError {
    #[error("transcript mismatch")]
    TranscriptMismatch,
    #[error("algebraic residual check failed at ζ")]
    Algebra,
    #[error(transparent)]
    Pcs(#[from] PcsVerifyError),
}

impl<'a> Prover<'a> {
    /// Commit to a time-streamed polynomial by converting to **coefficient tiles**
    /// (via blocked IFFT under the hood) and aggregating directly in the PCS.
    #[inline]
    fn commit_from_time_stream<I: Iterator<Item = F>>(
        &self,
        poly_id: &'static str,
        time_vals: I,
        pcs_degree_ctx: &PcsParams,
    ) -> PcsCommit {
        // Commit from **coefficient** tiles regardless of the time/eval basis at the API level.
        let pcs_for_commit = PcsParams { basis: Basis::Coefficient, ..pcs_degree_ctx.clone() };
        let tiles = crate::domain::ifft_time_stream_to_coeff_tiles(
            &self.params.domain,
            self.params.b_blk,
            time_vals,
        );

        let mut agg = Aggregator::new(&pcs_for_commit, poly_id);
        consume_tiles_with_prefetch(tiles, |tile| agg.add_block_coeffs(tile));
        agg.finalize()
    }

    fn build_header(&self) -> ProofHeader {
        ProofHeader {
            version: 1,
            domain_n: self.params.domain.n as u32,
            domain_omega: self.params.domain.omega,
            zh_c: self.params.domain.zh_c,
            k: self.air.k as u16,
            basis_wires: self.params.pcs_wires.basis,
            srs_g1_digest: pcs::srs_g1_digest(),
            srs_g2_digest: pcs::srs_g2_digest(),
        }
    }

    pub fn prove_with_restreamer(
        &self,
        rs: &impl Restreamer<Item = air::Row>,
    ) -> Result<Proof, ProveError> {
        let t_rows = rs.len_rows();
        if self.air.k == 0 {
            return Err(ProveError::Params("AIR must define at least one register (k > 0)"));
        }
        if self.params.b_blk == 0 {
            return Err(ProveError::Params("block size b_blk must be positive"));
        }

        let mut fs = Transcript::new("sszkp.proof");
        let header = self.build_header();
        fs.absorb_protocol_header(&header);

        let pcs_wires: &PcsParams = &self.params.pcs_wires;
        let pcs_coeff: &PcsParams = &self.params.pcs_coeff;
        let b_blk = self.params.b_blk;

        // A — selectors (public-fixed in this repo; omitted from transcript)
        if false && !self.air.selectors.is_empty() {
            let n = self.params.domain.n;
            for col in &self.air.selectors {
                let time_stream =
                    (0..n).map(|i| if col.is_empty() { F::zero() } else { col[i % col.len()] });
                let cm = self.commit_from_time_stream("selector", time_stream, pcs_wires);
                fs.absorb_commitment_l(FsLabel::SelectorCommit, &cm);
            }
        }

        // B — wires (stream per register)
        let mut wire_commits: Vec<PcsCommit> = Vec::with_capacity(self.air.k);
        let boundary_seed = vec![F::zero(); self.air.k].into_boxed_slice();

        // Iterator that restreams a target register `m` in time order by blocks.
        struct WireTime<'r, R: Restreamer<Item = air::Row>> {
            air: &'r air::AirSpec,
            rs: &'r R,
            boundary: Box<[F]>,
            t_rows: usize,
            b_blk: usize,
            next_block: usize,
            cur_block: Option<(Vec<F>, usize)>,
            reg_idx: usize,
        }
        impl<'r, R: Restreamer<Item = air::Row>> Iterator for WireTime<'r, R> {
            type Item = F;
            fn next(&mut self) -> Option<F> {
                loop {
                    if let Some((ref v, ref mut i)) = self.cur_block {
                        if *i < v.len() {
                            let out = v[*i];
                            *i += 1;
                            return Some(out);
                        }
                        self.cur_block = None;
                        continue;
                    }
                    let start_idx = self.next_block * self.b_blk;
                    if start_idx >= self.t_rows {
                        return None;
                    }
                    let end_idx = (start_idx + self.b_blk).min(self.t_rows);
                    let it = self.rs.stream_rows(RowIdx(start_idx), RowIdx(end_idx));
                    let br = air::eval_block(
                        self.air,
                        RegIdx(self.reg_idx),
                        BlockIdx(self.next_block),
                        &self.boundary,
                        it,
                    );
                    self.boundary = br.boundary_out;
                    self.cur_block = Some((br.reg_m_vals, 0));
                    self.next_block += 1;
                }
            }
        }
        for m in 0..self.air.k {
            let time_stream = WireTime {
                air: self.air,
                rs,
                boundary: boundary_seed.clone(),
                t_rows,
                b_blk,
                next_block: 0,
                cur_block: None,
                reg_idx: m,
            };
            let cm = self.commit_from_time_stream("wire", time_stream, pcs_wires);
            fs.absorb_commitment_l(FsLabel::WireCommit, &cm);
            wire_commits.push(cm);
        }

        // (β, γ)
        let beta: F = fs.challenge_f_l(FsLabel::Beta);
        let gamma: F = fs.challenge_f_l(FsLabel::Gamma);

        // C — permutation / lookup accumulators + streamed Z commitment
        let mut perm_acc = PermAcc { z: F::one() };
        let mut _lookup_acc = LookupAcc { z: F::one() };

        let mut bifft_z = crate::domain::BlockedIfft::new(&self.params.domain, b_blk);
        let mut boundary = boundary_seed.clone();
        let mut z_start = F::one();

        for (BlockIdx(t), start, end) in blocks(t_rows, b_blk) {
            let block_it = rs.stream_rows(start, end);
            let br: BlockResult =
                air::eval_block(self.air, RegIdx(0), BlockIdx(t), &boundary, block_it);

            absorb_block_perm(&mut perm_acc, &br.locals, beta, gamma);

            // Produce Z evaluations for this block and feed to IFFT.
            let z_block = emit_z_column_block(z_start, &br.locals, beta, gamma);
            if let Some(last) = z_block.last() {
                z_start = *last;
            }
            bifft_z.feed_eval_block(&z_block);

            #[cfg(feature = "lookups")]
            {
                absorb_block_lookup_with_challenges(&mut _lookup_acc, &br.locals, beta, gamma);
            }
            #[cfg(not(feature = "lookups"))]
            {
                absorb_block_lookup(&mut _lookup_acc, &br.locals);
            }

            boundary = br.boundary_out;
        }

        // Finalize Z commitment: drain coeff tiles with a one-tile prefetch
        let mut cm_z_opt: Option<PcsCommit> = None;
        {
            let pcs_for_z = PcsParams { basis: Basis::Coefficient, ..pcs_wires.clone() };
            let mut agg_z = Aggregator::new(&pcs_for_z, "perm_Z");

            let tiles = bifft_z.finish_low_to_high();
            consume_tiles_with_prefetch(tiles, |tile| agg_z.add_block_coeffs(tile));

            let cm_z = agg_z.finalize();
            fs.absorb_commitment_l(FsLabel::PermZCommit, &cm_z);
            cm_z_opt = Some(cm_z);
        }

        // (α)
        let alpha: F = fs.challenge_f_l(FsLabel::Alpha);

        // D — Quotient Q (fully streamed builder)
        let r_cfg = ResidualCfg { alpha, beta, gamma };
        let r_stream = air::residual_stream(self.air, r_cfg.clone(), rs, b_blk);
        let q_commit: PcsCommit = build_and_commit_quotient_streamed_r(
            &self.params.domain,
            &self.params.pcs_coeff,
            alpha,
            beta,
            gamma,
            b_blk,
            r_stream,
        )?;
        fs.absorb_commitment_l(FsLabel::QuotientCommit, &q_commit);

        // Points: keep `[ζ]` for compatibility.
        let eval_points: Vec<F> = fs.challenge_points_l(FsLabel::EvalPoints, 1);
        let zeta = eval_points[0];

        // E — Openings (tile-streamed throughout)
        let mut open_set: Vec<PcsCommit> = Vec::new();
        open_set.extend_from_slice(&wire_commits);
        if let Some(zc) = cm_z_opt {
            open_set.push(zc);
        }
        let q_index = open_set.len();
        open_set.push(q_commit);

        let k_regs = self.air.k;

        // Wires @ ζ — open each wire with PCS helper using **coeff tiles** hi→lo
        let mut proofs_wires: Vec<pcs::OpeningProof> = Vec::with_capacity(k_regs);
        for m in 0..k_regs {
            // Build the time-stream for register m (recompute is OK; see note above).
            let mut boundary = vec![F::zero(); k_regs].into_boxed_slice();
            let time_vals_iter = blocks(t_rows, b_blk).flat_map(move |(BlockIdx(t), start, end)| {
                let it = rs.stream_rows(start, end);
                let br = air::eval_block(self.air, RegIdx(m), BlockIdx(t), &boundary, it);
                boundary = br.boundary_out;
                br.reg_m_vals.into_iter()
            });

            // Coeff tiles, high→low, streamed into the PCS opener.
            let mut tiles_it = crate::domain::ifft_time_stream_to_coeff_tiles_hi_to_lo(
                &self.params.domain,
                b_blk,
                time_vals_iter,
            );

            let mut stream_coeff_hi_to_lo =
                |_idx: usize, sink: &mut dyn FnMut(Vec<F>)| while let Some(tile) = tiles_it.next()
                {
                    sink(tile);
                };

            let pr = pcs::open_at_points_with_coeffs(
                &self.params.pcs_wires,
                &[wire_commits[m]],
                |_idx, _z| F::zero(),
                &mut stream_coeff_hi_to_lo,
                &eval_points,
            );
            proofs_wires.extend(pr);
        }

        // Z @ ζ — recompute Z evals and open from coeff tiles (hi→lo)
        let proofs_z_at_zeta: Vec<pcs::OpeningProof> = if let Some(zc) = cm_z_opt {
            let mut boundary = boundary_seed.clone();
            let mut z_run = F::one();
            let mut bifft = crate::domain::BlockedIfft::new(&self.params.domain, b_blk);
            for (BlockIdx(t), start, end) in blocks(t_rows, b_blk) {
                let it = rs.stream_rows(start, end);
                let br = air::eval_block(self.air, RegIdx(0), BlockIdx(t), &boundary, it);
                let zb = emit_z_column_block(z_run, &br.locals, beta, gamma);
                if let Some(last) = zb.last() {
                    z_run = *last;
                }
                bifft.feed_eval_block(&zb);
                boundary = br.boundary_out;
            }

            let mut tiles = bifft.finish_high_to_low();
            let mut stream_coeff_hi_to_lo =
                |_idx: usize, sink: &mut dyn FnMut(Vec<F>)| while let Some(block) = tiles.next() {
                    sink(block);
                };

            pcs::open_at_points_with_coeffs(
                &self.params.pcs_wires,
                &[zc],
                |_idx, _z| F::zero(),
                &mut stream_coeff_hi_to_lo,
                &eval_points,
            )
        } else {
            Vec::new()
        };

        // Q @ ζ — stream tiles (hi→lo) directly from residual stream
        let mut stream_q_coeff_hi_to_lo = |_idx: usize, sink: &mut dyn FnMut(Vec<F>)| {
            let r_stream_all = air::residual_stream(self.air, r_cfg.clone(), rs, b_blk);
            let mut tiles = stream_q_coeff_tiles_hi_to_lo_from_r_stream(
                &self.params.domain,
                b_blk,
                r_stream_all,
            );
            while let Some(block) = tiles.next() {
                sink(block);
            }
        };
        let proofs_q_at_zeta = pcs::open_at_points_with_coeffs(
            &self.params.pcs_coeff,
            &[open_set[q_index]],
            |_idx, _z| F::zero(),
            &mut stream_q_coeff_hi_to_lo,
            &eval_points,
        );

        // (Feature) Z @ ω·ζ — recompute and open from coeff tiles (hi→lo)
        #[cfg(feature = "zeta-shift")]
        let proofs_z_at_omega_zeta: Vec<pcs::OpeningProof> = if let Some(zc) = cm_z_opt {
            let omega_zeta = self.params.domain.omega * zeta;
            let pts = vec![omega_zeta];

            let mut boundary = boundary_seed.clone();
            let mut z_run = F::one();
            let mut bifft = crate::domain::BlockedIfft::new(&self.params.domain, b_blk);
            for (BlockIdx(t), start, end) in blocks(t_rows, b_blk) {
                let it = rs.stream_rows(start, end);
                let br = air::eval_block(self.air, RegIdx(0), BlockIdx(t), &boundary, it);
                let zb = emit_z_column_block(z_run, &br.locals, beta, gamma);
                if let Some(last) = zb.last() {
                    z_run = *last;
                }
                bifft.feed_eval_block(&zb);
                boundary = br.boundary_out;
            }
            let mut tiles = bifft.finish_high_to_low();
            let mut stream_coeff_hi_to_lo =
                |_idx: usize, sink: &mut dyn FnMut(Vec<F>)| while let Some(block) = tiles.next() {
                    sink(block);
                };

            pcs::open_at_points_with_coeffs(
                &self.params.pcs_wires,
                &[zc],
                |_idx, _z| F::zero(),
                &mut stream_coeff_hi_to_lo,
                &pts,
            )
        } else {
            Vec::new()
        };

        #[cfg(not(feature = "zeta-shift"))]
        let proofs_z_at_omega_zeta: Vec<pcs::OpeningProof> = Vec::new();

        // Merge proofs in required order:
        // [wires@ζ] [Z@ζ?] [Q@ζ] [Z@ω·ζ?]
        let mut opening_proofs = Vec::new();
        opening_proofs.extend(proofs_wires);
        opening_proofs.extend(proofs_z_at_zeta.iter().cloned());
        opening_proofs.extend(proofs_q_at_zeta.iter().cloned());
        opening_proofs.extend(proofs_z_at_omega_zeta.iter().cloned());

        // Claimed evals follow the same order.
        let evals: Vec<F> = opening_proofs.iter().map(|p| p.value).collect();

        // Wrap -> Proof
        let wire_comms_wrapped: Vec<crate::Commitment> =
            wire_commits.iter().copied().map(|c| crate::Commitment(c.0)).collect();
        let z_comm_wrapped: Option<crate::Commitment> = cm_z_opt.map(|c| crate::Commitment(c.0));
        let q_comm_wrapped: crate::Commitment = crate::Commitment(open_set[q_index].0);

        Ok(Proof {
            header,
            wire_comms: wire_comms_wrapped,
            z_comm: z_comm_wrapped,
            q_comm: q_comm_wrapped,
            eval_points, // still [ζ] only (deterministic by label)
            evals,
            opening_proofs,
        })
    }

    pub fn prove(&self, witness_rows: impl Iterator<Item = air::Row>) -> Result<Proof, ProveError> {
        let rows: Vec<air::Row> = witness_rows.collect();
        self.prove_with_restreamer(&rows)
    }
}

impl<'a> Verifier<'a> {
    pub fn verify(&self, proof: &crate::Proof) -> Result<(), VerifySchedError> {
        let mut fs = Transcript::new("sszkp.proof");
        fs.absorb_protocol_header(&proof.header);

        // A — selectors (public-fixed; intentionally omitted)

        // B — wires
        for cm in &proof.wire_comms {
            fs.absorb_commitment_l(FsLabel::WireCommit, &pcs::Commitment(cm.0));
        }

        // (β, γ)
        let beta: F = fs.challenge_f_l(FsLabel::Beta);
        let gamma: F = fs.challenge_f_l(FsLabel::Gamma);

        // C — Z (if present)
        let has_z = if let Some(zc) = &proof.z_comm {
            fs.absorb_commitment_l(FsLabel::PermZCommit, &pcs::Commitment(zc.0));
            true
        } else {
            false
        };

        // (α)
        let alpha: F = fs.challenge_f_l(FsLabel::Alpha);

        // D — Q
        fs.absorb_commitment_l(FsLabel::QuotientCommit, &pcs::Commitment(proof.q_comm.0));

        // Eval points (ζ only)
        let expect_points: Vec<F> =
            fs.challenge_points_l(FsLabel::EvalPoints, proof.eval_points.len());
        if expect_points != proof.eval_points {
            return Err(VerifySchedError::TranscriptMismatch);
        }
        let zeta = proof.eval_points[0];
        let omega = proof.header.domain_omega;
        #[cfg(feature = "zeta-shift")]
        let omega_zeta = omega * zeta;

        // Build commitments list for verification routing
        let mut open_set_all: Vec<PcsCommit> = Vec::new();
        open_set_all.extend(proof.wire_comms.iter().map(|c| pcs::Commitment(c.0)));
        if has_z {
            if let Some(zc) = &proof.z_comm {
                open_set_all.push(pcs::Commitment(zc.0));
            }
        }
        open_set_all.push(pcs::Commitment(proof.q_comm.0));

        let k = proof.wire_comms.len();
        let s = proof.eval_points.len(); // =1

        // Partition evals/proofs in the order the prover appended them:
        let m_wz = k + if has_z { 1 } else { 0 };
        let count_wires = k * s; // k
        let count_z_at_zeta = if has_z { s } else { 0 }; // 0 or 1
        let count_q = s; // 1

        let mut cursor = 0usize;

        // Wires @ ζ
        let evals_wires = &proof.evals[cursor..cursor + count_wires];
        let proofs_wires = &proof.opening_proofs[cursor..cursor + count_wires];
        let open_set_wires: Vec<PcsCommit> = open_set_all[0..k].to_vec();
        pcs::verify_openings(
            &self.params.pcs_wires,
            &open_set_wires,
            &proof.eval_points,
            evals_wires,
            proofs_wires,
        )?;
        cursor += count_wires;

        // Z @ ζ (if any)
        if has_z {
            let evals_z = &proof.evals[cursor..cursor + count_z_at_zeta];
            let proofs_z = &proof.opening_proofs[cursor..cursor + count_z_at_zeta];
            let open_set_z: [PcsCommit; 1] = [open_set_all[k]];
            pcs::verify_openings(
                &self.params.pcs_wires,
                &open_set_z,
                &proof.eval_points,
                evals_z,
                proofs_z,
            )?;
            cursor += count_z_at_zeta;
        }

        // Q @ ζ
        {
            let evals_q = &proof.evals[cursor..cursor + count_q];
            let proofs_q = &proof.opening_proofs[cursor..cursor + count_q];
            let open_set_q: [PcsCommit; 1] = [open_set_all[m_wz]];
            pcs::verify_openings(
                &self.params.pcs_coeff,
                &open_set_q,
                &proof.eval_points,
                evals_q,
                proofs_q,
            )?;
            cursor += count_q;
        }

        // (Feature) Z @ ω·ζ
        #[cfg(feature = "zeta-shift")]
        if has_z {
            let evals_z_omega = &proof.evals[cursor..cursor + 1];
            let proofs_z_omega = &proof.opening_proofs[cursor..cursor + 1];
            let open_set_z: [PcsCommit; 1] = [open_set_all[k]];
            pcs::verify_openings(
                &self.params.pcs_wires,
                &open_set_z,
                &[omega_zeta],
                evals_z_omega,
                proofs_z_omega,
            )?;
            cursor += 1;
        }

        // Algebraic check at ζ (hard error if violated)
        let n_u32 = proof.header.domain_n;
        let zh_c = proof.header.zh_c;
        let k_regs = proof.header.k as usize;

        // Gather wires@ζ in eval order.
        let mut wires_at_zeta: Vec<F> = Vec::with_capacity(k_regs);
        for m in 0..k_regs {
            wires_at_zeta.push(proof.evals[m * s + 0]);
        }
        let z_at_zeta = if has_z { proof.evals[k_regs * s + 0] } else { F::one() };

        // Pass Q(ζ) to the residual helper unless strict mode is enabled.
        let q_at_zeta = {
            let idx = (k_regs + if has_z { 1 } else { 0 }) * s + 0;
            proof.evals[idx]
        };
        let q_arg = if cfg!(feature = "strict-recompute-r") {
            None
        } else {
            Some(q_at_zeta)
        };

        #[cfg(feature = "zeta-shift")]
        let z_at_omega_zeta = if has_z {
            let idx_tail = proof.evals.len() - 1;
            Some(proof.evals[idx_tail])
        } else {
            None
        };

        #[cfg(not(feature = "zeta-shift"))]
        let z_at_omega_zeta = None;

        let r_at_zeta = air::residual_eval_at_point_symbolic(
            k_regs,
            (&n_u32, &zh_c),
            air::ResidualCfg { alpha, beta, gamma },
            zeta,
            &wires_at_zeta,
            z_at_zeta,
            None, // selectors@ζ (optional)
            None, // id@ζ (optional)
            None, // σ@ζ (optional)
            q_arg, // Q(ζ) is ignored when strict-recompute-r is enabled
            // Optionals:
            z_at_omega_zeta,
            None, // z_l_at_zeta
            None, // z_l_at_omega_zeta
        );

        // Check Z_H(ζ)·Q(ζ) − R(ζ) == 0
        let zh_at_zeta = zeta.pow([n_u32 as u64]) - zh_c;
        let lhs = zh_at_zeta * q_at_zeta - r_at_zeta;
        if !lhs.is_zero() {
            return Err(VerifySchedError::Algebra);
        }

        Ok(())
    }
}
