//! Polynomial Commitment Scheme (PCS) — KZG on BN254
//!
//! # What changed in this revision
//! - **Tile-friendly aggregation APIs**: the `Aggregator` now ingests monomial
//!   **coefficient tiles** in either order (low→high or high→low), a key
//!   building block for *sublinear-space* workflows that avoid materializing
//!   the entire coefficient vector.
//! - Added error-typed, result-returning variants (`*_r`) while keeping the
//!   existing panic-on-error methods for **backward compatibility**.
//! - Ergonomics: `PcsParams::with_basis(basis)` convenience to switch bases
//!   without reloading the SRS.
//! - **Streaming entry points**:
//!     - `commit_stream` — commit from a `CoeffTileStream` without ever owning
//!       a full `Vec` of coefficients.
//!     - `eval_at_stream` — Horner folding over tiles (wrapper).
//! - Kept SRS digest helpers and all public types intact (no API break).
//!
//! ## Notes (whitepaper alignment)
//! The PCS aggregator operates over **monomial coefficients** (tiles) and is
//! deliberately **independent of the time/evaluation basis**. If callers have
//! time-ordered evaluations on `H`, they must convert blockwise via IFFT and
//! feed the resulting coefficient tiles (this is how our streaming scheduler
//! maintains sublinear space).

#![forbid(unsafe_code)]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_variables)]
#![allow(missing_docs)]
#![allow(non_snake_case)]

use ark_bn254::{Bn254, Fr as ScalarField, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::{pairing::Pairing, AffineRepr, CurveGroup, Group};
use ark_ff::{Field, One, PrimeField, UniformRand, Zero};
use ark_serialize::{
    CanonicalDeserialize, CanonicalSerialize, Compress, Read, SerializationError, Validate, Valid,
    Write,
};
use blake3::Hasher;
use rand::{rngs::StdRng, SeedableRng};
use std::sync::{Mutex, OnceLock};

use crate::{domain, F};
// Streaming tile trait + Horner fold
use crate::stream::{horner_eval_stream, CoeffTileStream};

/// Enable (future) blinding hooks in openings (currently NO-OP).
#[cfg(feature = "hiding-kzg")]
const HIDING_KZG: bool = true;
#[cfg(not(feature = "hiding-kzg"))]
const HIDING_KZG: bool = false;

/// Which basis the PCS expects when **committing**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Commitment expects evaluations on a subgroup/coset.
    Evaluation,
    /// Commitment expects monomial coefficients (low→high).
    Coefficient,
}

// Manual canonical ser/de for enums so ark-serialize derives can include Basis.
impl CanonicalSerialize for Basis {
    fn serialize_with_mode<W: Write>(
        &self,
        mut w: W,
        _cm: Compress,
    ) -> Result<(), SerializationError> {
        let byte = match self {
            Basis::Evaluation => 0u8,
            Basis::Coefficient => 1u8,
        };
        w.write_all(&[byte])?;
        Ok(())
    }
    fn serialized_size(&self, _cm: Compress) -> usize {
        1
    }
}
impl CanonicalDeserialize for Basis {
    fn deserialize_with_mode<R: Read>(
        mut r: R,
        _cm: Compress,
        _validate: Validate,
    ) -> Result<Self, SerializationError> {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        match b[0] {
            0 => Ok(Basis::Evaluation),
            1 => Ok(Basis::Coefficient),
            _ => Err(SerializationError::InvalidData),
        }
    }
}
impl Valid for Basis {
    fn check(&self) -> Result<(), SerializationError> {
        Ok(())
    }
}

/// Public parameters for the polynomial commitment scheme.
#[derive(Debug, Clone, CanonicalSerialize, CanonicalDeserialize)]
pub struct PcsParams {
    /// Maximum supported degree **d** (inclusive). Number of SRS powers is `d+1`.
    pub max_degree: usize,
    /// Basis expected by the commit-time interface for *the polynomial*.
    pub basis: Basis,
    /// Placeholder to keep the type stable if we ever inline SRS metadata.
    pub srs_placeholder: (),
}

impl PcsParams {
    /// Return a copy of these parameters with a different expected **basis**.
    ///
    /// This does **not** reload or change the SRS; it only switches the local
    /// basis setting for committing/aggregation APIs.
    #[inline]
    pub fn with_basis(mut self, basis: Basis) -> Self {
        self.basis = basis;
        self
    }
}

/// PCS commitment newtype (wrap **G1Affine** directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, CanonicalSerialize, CanonicalDeserialize)]
pub struct Commitment(pub G1Affine);

/// KZG opening proof at a single point.
#[derive(Debug, Clone, CanonicalSerialize, CanonicalDeserialize)]
pub struct OpeningProof {
    /// Evaluation point ζ.
    pub zeta: F,
    /// Claimed value f(ζ) (redundant with transcript but convenient).
    pub value: F,
    /// Commitment to the witness polynomial W(X) = (f(X) − f(ζ)) / (X − ζ).
    pub witness_comm: Commitment,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("shape mismatch: expected {expected} items, got {got}")]
    Shape { expected: usize, got: usize },
    #[error("G2 SRS not loaded")]
    MissingG2,
    #[error("final pairing check failed")]
    Pairing,
}

#[derive(Debug, thiserror::Error)]
pub enum SrsLoadError {
    #[error("empty SRS provided")]
    Empty,
}

// ===========================================================================
// Internal SRS (BN254) — G1 powers of τ and a single G2 element [τ]G2
// ===========================================================================

#[derive(Debug)]
struct SrsG1 {
    powers: Vec<G1Affine>,
    #[cfg(feature = "dev-srs")]
    tau: ScalarField,
}

impl SrsG1 {
    #[cfg(feature = "dev-srs")]
    fn new_dev() -> Self {
        let mut rng = StdRng::from_seed([42u8; 32]);
        let tau = ScalarField::rand(&mut rng);
        let mut s = SrsG1 { powers: Vec::new(), tau };
        s.ensure_len(1);
        s
    }

    fn ensure_len(&mut self, new_len: usize) {
        if self.powers.len() >= new_len {
            return;
        }
        #[cfg(feature = "dev-srs")]
        {
            let gen = G1Projective::generator();
            let current = self.powers.len();
            for idx in current..new_len {
                let gi = gen.mul_bigint(self.tau.pow([idx as u64]).into_bigint());
                self.powers.push(gi.into_affine());
            }
        }
        #[cfg(not(feature = "dev-srs"))]
        {
            assert!(
                self.powers.len() >= new_len,
                "G1 SRS insufficient; call try_load_srs_g1 with at least {} elements",
                new_len
            );
        }
    }

    #[inline]
    fn get_power(&self, idx: usize) -> G1Affine {
        self.powers[idx]
    }
}

fn srs_g1() -> &'static Mutex<SrsG1> {
    static SRS: OnceLock<Mutex<SrsG1>> = OnceLock::new();
    #[cfg(feature = "dev-srs")]
    {
        SRS.get_or_init(|| Mutex::new(SrsG1::new_dev()))
    }
    #[cfg(not(feature = "dev-srs"))]
    {
        SRS.get_or_init(|| Mutex::new(SrsG1 { powers: Vec::new() }))
    }
}

/// Load a trusted **G1** SRS and return a template (Result).
pub fn try_load_srs_g1(powers: &[G1Affine]) -> Result<PcsParams, SrsLoadError> {
    if powers.is_empty() {
        return Err(SrsLoadError::Empty);
    }
    let mut guard = srs_g1().lock().expect("SRS mutex poisoned");
    guard.powers.clear();
    guard.powers.extend_from_slice(powers);
    drop(guard);

    Ok(PcsParams {
        max_degree: powers.len() - 1,
        basis: Basis::Coefficient,
        srs_placeholder: (),
    })
}

/// Back-compat wrapper: panics on error.
pub fn load_srs_g1(powers: &[G1Affine]) -> PcsParams {
    try_load_srs_g1(powers).expect("invalid G1 SRS")
}

#[derive(Debug, Clone)]
struct SrsG2 {
    tau_g2: Option<G2Affine>,
}

impl SrsG2 {
    #[cfg(feature = "dev-srs")]
    fn new_dev() -> Self {
        let tau = srs_g1().lock().expect("SRS mutex poisoned").tau;
        let g2_gen = <Bn254 as Pairing>::G2::generator();
        let tau_g2 = (G2Projective::from(g2_gen) * tau).into_affine();
        Self { tau_g2: Some(tau_g2) }
    }

    #[cfg(not(feature = "dev-srs"))]
    fn new_prod() -> Self {
        Self { tau_g2: None }
    }
}

fn srs_g2() -> &'static Mutex<SrsG2> {
    static SRS2: OnceLock<Mutex<SrsG2>> = OnceLock::new();
    #[cfg(feature = "dev-srs")]
    {
        SRS2.get_or_init(|| Mutex::new(SrsG2::new_dev()))
    }
    #[cfg(not(feature = "dev-srs"))]
    {
        SRS2.get_or_init(|| Mutex::new(SrsG2::new_prod()))
    }
}

/// Load **G2** SRS element `[τ]G2` for verification (Result).
pub fn try_load_srs_g2(tau_g2: G2Affine) -> Result<(), SrsLoadError> {
    let mut guard = srs_g2().lock().expect("SRS mutex poisoned");
    guard.tau_g2 = Some(tau_g2);
    Ok(())
}

/// Back-compat wrapper.
pub fn load_srs_g2(tau_g2: G2Affine) {
    try_load_srs_g2(tau_g2).expect("invalid G2 SRS");
}

// ----------------------- SRS digests (public) -----------------------

fn hash_bytes(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(b"SSZKP.SRS.v1");
    for p in parts {
        h.update(&((*p).len() as u64).to_be_bytes());
        h.update(p);
    }
    *h.finalize().as_bytes()
}

pub fn srs_g1_digest() -> [u8; 32] {
    let guard = srs_g1().lock().expect("SRS G1 mutex poisoned");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(guard.powers.len() as u64).to_be_bytes());
    for p in &guard.powers {
        let mut tmp = Vec::with_capacity(48);
        p.serialize_compressed(&mut tmp).expect("serialize G1");
        bytes.extend_from_slice(&tmp);
    }
    hash_bytes(&[&bytes])
}

pub fn srs_g2_digest() -> [u8; 32] {
    let guard = srs_g2().lock().expect("SRS G2 mutex poisoned");
    let mut bytes = Vec::new();
    if let Some(tau_g2) = guard.tau_g2 {
        let mut tmp = Vec::with_capacity(96);
        tau_g2.serialize_compressed(&mut tmp).expect("serialize G2");
        bytes.extend_from_slice(&tmp);
    }
    hash_bytes(&[&bytes])
}

// ===========================================================================
// Aggregator — streaming-friendly, tile-aware coefficient ingestion
// ===========================================================================

/// Error type for result-returning aggregator APIs.
#[derive(Debug, thiserror::Error)]
pub enum AggregatorError {
    #[error("coefficient stream exceeds max_degree: cursor {cursor}, adding {adding} exceeds limit {limit}")]
    DegreeOverflow { cursor: usize, adding: usize, limit: usize },
    #[error("PCS basis mismatch (expected {expected:?}, got {got:?})")]
    Basis { expected: Basis, got: Basis },
}

/// Aggregates contributions `a_i · [τ^i]G₁` as tiles of **coefficients** arrive.
///
/// The aggregator is intentionally **basis-agnostic w.r.t. time**: callers
/// must provide *monomial coefficients* in stream order. If they start from
/// evaluations, they should convert blocks with IFFT first (see `domain.rs`).
pub struct Aggregator<'a> {
    pub(crate) pcs: &'a PcsParams,
    pub(crate) poly_id: &'static str,
    acc: G1Projective,
    cursor: usize,
    // --- diagnostics (opt-in via env) ---
    memlog: bool,
    peak_inflight_coeffs: usize,
    total_blocks: usize,
    peak_buffered_blocks: usize, // keep 0 if no internal staging
}

impl<'a> Aggregator<'a> {
    /// Create a new aggregator. The initial `cursor` is 0 (constant term slot).
    pub fn new(pcs: &'a PcsParams, poly_id: &'static str) -> Self {
        let memlog = std::env::var("SSZKP_MEMLOG").ok().as_deref() == Some("1");
        Self {
            pcs,
            poly_id,
            acc: G1Projective::zero(),
            cursor: 0,
            memlog,
            peak_inflight_coeffs: 0,
            total_blocks: 0,
            peak_buffered_blocks: 0,
        }
    }

    /// Current stream cursor (number of coefficients already absorbed).
    #[inline]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Remaining capacity (#coefficients) before reaching `max_degree + 1`.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.pcs.max_degree + 1 - self.cursor
    }

    /// Pre-reserve SRS powers to reduce mutex traffic (optional micro-optim).
    pub fn reserve_coeffs(&mut self, count: usize) {
        let need = self.cursor.saturating_add(count);
        let mut guard = srs_g1().lock().expect("SRS mutex poisoned");
        guard.ensure_len(need);
    }

    // ----------------- High-level ingestion (legacy behavior kept) -----------------

    /// Add a block of **evaluations**, converting to coefficients via IFFT.
    pub fn add_block_evals(&mut self, d: &crate::domain::Domain, slice: &[F]) {
        assert!(
            matches!(self.pcs.basis, Basis::Evaluation),
            "PCS basis mismatch (eval)"
        );
        let coeffs = domain::ifft_block_evals_to_coeffs(d, slice);
        if self.memlog && coeffs.len() > self.peak_inflight_coeffs {
            self.peak_inflight_coeffs = coeffs.len();
        }
        self.total_blocks += 1;
        self.add_block_coeffs_inner(&coeffs);
    }

    /// Add a block of **coefficients** provided in **low→high** order.
    pub fn add_block_coeffs(&mut self, slice: &[F]) {
        assert!(
            matches!(self.pcs.basis, Basis::Coefficient),
            "PCS basis mismatch (coeff)"
        );
        if self.memlog && slice.len() > self.peak_inflight_coeffs {
            self.peak_inflight_coeffs = slice.len();
        }
        self.total_blocks += 1;
        self.add_block_coeffs_inner(slice);
    }

    /// Result-returning variant of `add_block_coeffs`.
    pub fn add_block_coeffs_r(&mut self, slice: &[F]) -> Result<(), AggregatorError> {
        if !matches!(self.pcs.basis, Basis::Coefficient) {
            return Err(AggregatorError::Basis {
                expected: Basis::Coefficient,
                got: self.pcs.basis,
            });
        }
        if self.memlog && slice.len() > self.peak_inflight_coeffs {
            self.peak_inflight_coeffs = slice.len();
        }
        self.total_blocks += 1;
        self.add_block_coeffs_checked(slice)
    }

    /// Finalize and return the commitment.
    pub fn finalize(self) -> Commitment {
        if self.memlog {
            eprintln!(
                "[memlog] Aggregator(poly='{}'): peak_inflight_coeffs={}, total_blocks={}, peak_buffered_blocks={}",
                self.poly_id, self.peak_inflight_coeffs, self.total_blocks, self.peak_buffered_blocks
            );
        }
        Commitment(self.acc.into_affine())
    }

    // ----------------- Tile-oriented ingestion (new) -----------------

    /// Add a **coefficient tile** in the given order.
    ///
    /// - `LowToHigh`: tile is `[a_i, a_{i+1}, …]` matching the current cursor.
    /// - `HighToLow`: tile is `[a_j, a_{j-1}, …]` with *highest degree first*.
    ///
    /// Tiles **must** be contiguous with the current cursor, i.e., callers
    /// should stream tiles for the same polynomial sequentially.
    pub fn add_coeff_tile(
        &mut self,
        tile: &[F],
        order: crate::domain::CoeffTileOrder,
    ) -> Result<(), AggregatorError> {
        match order {
            crate::domain::CoeffTileOrder::LowToHigh => self.add_block_coeffs_r(tile),
            crate::domain::CoeffTileOrder::HighToLow => {
                // Reverse into a short temp buffer; tile sizes are typically ≤ √N.
                let mut tmp: Vec<F> = tile.iter().rev().copied().collect();
                self.add_block_coeffs_r(&tmp)
            }
        }
    }

    /// Add multiple coefficient tiles produced by an iterator.
    pub fn add_coeff_tiles<I>(
        &mut self,
        mut tiles: I,
        order: crate::domain::CoeffTileOrder,
    ) -> Result<(), AggregatorError>
    where
        I: Iterator<Item = &'static [F]>,
    {
        while let Some(t) = tiles.next() {
            self.add_coeff_tile(t, order)?;
        }
        Ok(())
    }

    // ----------------- Internals -----------------

    fn add_block_coeffs_checked(&mut self, coeffs: &[F]) -> Result<(), AggregatorError> {
        let add = coeffs.len();
        let limit = self.pcs.max_degree + 1;
        if self.cursor + add > limit {
            return Err(AggregatorError::DegreeOverflow {
                cursor: self.cursor,
                adding: add,
                limit,
            });
        }

        {
            let mut guard = srs_g1().lock().expect("SRS mutex poisoned");
            guard.ensure_len(self.cursor + add);
            // If you stage blocks internally, update peak_buffered_blocks here.
            // (We stream directly; keep at zero.)
        }

        let guard = srs_g1().lock().expect("SRS mutex poisoned");
        for (i, c) in coeffs.iter().enumerate() {
            if c.is_zero() {
                continue;
            }
            let base = guard.get_power(self.cursor + i);
            let term = base.into_group().mul_bigint(c.into_bigint());
            self.acc += term;
        }
        drop(guard);

        self.cursor += add;
        Ok(())
    }

    fn add_block_coeffs_inner(&mut self, coeffs: &[F]) {
        // Legacy behavior: panic on overflow to retain current callers’ expectations.
        self.add_block_coeffs_checked(coeffs)
            .expect("coefficient stream exceeds max_degree")
    }
}

// ===========================================================================
// NEW: Streaming PCS entry points
// ===========================================================================

/// Opaque SRS handle for streaming APIs. We delegate to this module’s
/// internal global SRS; the handle exists to match the requested signature.
#[derive(Debug, Clone, Copy, Default)]
pub struct SRS;

/// MSM/window configuration for streaming commits.
#[derive(Debug, Clone, Copy)]
pub struct CommitStreamCfg {
    /// MSM fixed-window size (bits). Kept here for future bucketed MSM.
    pub window_bits: u32,
    /// Preferred tile length (aka `b_blk`). Not strictly required here, but
    /// recorded for diagnostics and future tuning.
    pub tile_len: usize,
}

/// Minimal handle returned by `commit_stream` for re-use during openings.
/// (Extend later if you want to cache per-poly metadata.)
#[derive(Debug, Clone)]
pub struct StreamingHandle {
    pub degree: usize,
    pub basis: Basis,
}

impl Default for StreamingHandle {
    fn default() -> Self {
        Self { degree: 0, basis: Basis::Coefficient }
    }
}

/// Commit from a **coefficient tile stream** without materializing the full `Vec`.
///
/// Conceptually this could maintain MSM buckets per window; for now we
/// accumulate directly into a single `G1Projective` using the global SRS
/// powers, which already keeps memory at **O(tile_len)**.
///
/// The returned `StreamingHandle` captures the polynomial degree/basis.
pub fn commit_stream<TS>(
    mut tiles: TS,
    _srs: &SRS,
    cfg: &CommitStreamCfg,
) -> (Commitment, StreamingHandle)
where
    TS: CoeffTileStream,
{
    let memlog = std::env::var("SSZKP_MEMLOG").ok().as_deref() == Some("1");
    let mut acc = G1Projective::zero();
    let mut cursor = 0usize;
    let mut peak_inflight = 0usize;
    let mut total_tiles = 0usize;

    while let Some(tile) = tiles.next_tile() {
        total_tiles += 1;
        if memlog && tile.len() > peak_inflight {
            peak_inflight = tile.len();
        }

        {
            let mut guard = srs_g1().lock().expect("SRS G1 mutex poisoned");
            guard.ensure_len(cursor + tile.len());
        }
        let guard = srs_g1().lock().expect("SRS G1 mutex poisoned");
        for (i, c) in tile.iter().enumerate() {
            if c.is_zero() {
                continue;
            }
            let base = guard.get_power(cursor + i);
            acc += base.into_group().mul_bigint(c.into_bigint());
        }
        drop(guard);

        cursor += tile.len();
    }

    if memlog {
        eprintln!(
            "[memlog] commit_stream: peak_inflight_coeffs={}, total_blocks={}, b_blk_hint={}, window_bits={}",
            peak_inflight, total_tiles, cfg.tile_len, cfg.window_bits
        );
    }

    let c = Commitment(acc.into_affine());
    let handle = StreamingHandle { degree: cursor.saturating_sub(1), basis: Basis::Coefficient };
    (c, handle)
}

/// Streaming Horner evaluation wrapper (used by openings).
#[inline]
pub fn eval_at_stream<TS>(tiles: TS, z: F) -> F
where
    TS: CoeffTileStream,
{
    horner_eval_stream(tiles, z)
}

// ===========================================================================
// Openings (commit-side helpers used by the scheduler)
// ===========================================================================

/// Open at points using a caller-supplied evaluation closure.
/// This shape is preserved for backward compatibility with existing code.
pub fn open_at_points(
    _pcs: &PcsParams,
    commitments: &[Commitment],
    stream_eval: impl Fn(usize, F) -> F,
    points: &[F],
) -> Vec<OpeningProof> {
    let mut proofs = Vec::with_capacity(commitments.len().saturating_mul(points.len()));
    for (pi, _c) in commitments.iter().enumerate() {
        for &zeta in points {
            let val = stream_eval(pi, zeta);
            proofs.push(OpeningProof { zeta, value: val, witness_comm: Commitment(G1Affine::identity()) });
        }
    }
    proofs
}

/// Open at points using **coefficient streaming** (high→low) for the witness.
/// Single-pass, no buffering of the witness, no replay of the source:
/// - Uses Horner to accumulate f(ζ).
/// - Computes quotient coefficients on the fly (synthetic division).
/// - Adds each quotient coefficient directly into the MSM at absolute index j=i−1,
///   where `i` counts down from `pcs_for_poly.max_degree`.
pub fn open_at_points_with_coeffs(
    pcs_for_poly: &PcsParams,
    commitments: &[Commitment],
    _stream_eval: impl Fn(usize, F) -> F,
    mut stream_coeff_hi_to_lo: impl FnMut(usize, &mut dyn FnMut(Vec<F>)),
    points: &[F],
) -> Vec<OpeningProof> {
    let mut proofs = Vec::with_capacity(commitments.len().saturating_mul(points.len()));
    let memlog = std::env::var("SSZKP_MEMLOG").ok().as_deref() == Some("1");

    // Upper-bound SRS reservation once (we'll also check inside the loop defensively).
    {
        let mut g = srs_g1().lock().expect("SRS G1 mutex poisoned");
        g.ensure_len(pcs_for_poly.max_degree + 1);
    }

    for (pi, _c) in commitments.iter().enumerate() {
        for &zeta in points {
            let mut eval_acc = F::zero();          // Horner accumulator for f(ζ)
            let mut w_acc = G1Projective::zero();  // MSM accumulator for W(X)
            let mut i_abs: isize = pcs_for_poly.max_degree as isize; // absolute coefficient index (a_i), high→low

            let mut peak_inflight = 0usize;
            let mut total_blocks = 0usize;

            let mut consume_block = |mut blk_hi_to_lo: Vec<F>| {
                total_blocks += 1;
                if memlog && blk_hi_to_lo.len() > peak_inflight { peak_inflight = blk_hi_to_lo.len(); }

                // Make sure SRS has enough powers for upcoming indices (defensive).
                {
                    let mut g = srs_g1().lock().expect("SRS G1 mutex poisoned");
                    // We may touch up to (i_abs as usize) next, but guard for zero/negative below.
                    let need = (i_abs.max(0) as usize) + 1;
                    g.ensure_len(need);
                }

                // Drain the block (already high→low). For each incoming a_i:
                //   b_{i-1} = a_i + z * b_i, with b_{deg} := 0.
                //   f_acc    = a_i + z * f_acc  (standard Horner)
                //
                // We add b_{i-1} into MSM at index (i-1) immediately.
                let g = srs_g1().lock().expect("SRS G1 mutex poisoned");
                for a_i in blk_hi_to_lo.drain(..) {
                    // quotient recurrence (synthetic division)
                    let b_im1 = a_i + zeta * eval_acc;

                    // Horner remainder accumulator (f(ζ))
                    eval_acc = b_im1;

                    // Absolute MSM index for b_{i-1} is (i_abs - 1)
                    if i_abs > 0 && !b_im1.is_zero() {
                        let base = g.get_power((i_abs - 1) as usize);
                        w_acc += base.into_group().mul_bigint(b_im1.into_bigint());
                    }

                    // Move to next coefficient (downwards)
                    i_abs -= 1;
                }
                // drop(g) by leaving scope
            };

            stream_coeff_hi_to_lo(pi, &mut consume_block);

            if memlog {
                eprintln!(
                    "[memlog] WitnessStream(poly_idx={}, zeta=?): peak_inflight_coeffs={}, total_blocks={}",
                    pi, peak_inflight, total_blocks
                );
            }

            // eval_acc now equals f(ζ); w_acc holds commitment to W(X).
            proofs.push(OpeningProof {
                zeta,
                value: eval_acc,
                witness_comm: Commitment(w_acc.into_affine()),
            });
        }
    }

    proofs
}

/// Open at points from **evaluation streams** by converting each eval-tile to
/// coeff-tiles on the fly (still sublinear), then delegating to the coeff path.
/// Preserves old behavior for tile-local IFFTs.
pub fn open_eval_stream_at_points(
    pcs_for_poly: &PcsParams,
    commitments: &[Commitment],
    domain: &crate::domain::Domain,
    mut stream_evals: impl FnMut(usize, &mut dyn FnMut(Vec<F>)),
    points: &[F],
) -> Vec<OpeningProof> {
    let mut as_coeff_hi_to_lo = |idx: usize, sink: &mut dyn FnMut(Vec<F>)| {
        // Convert each eval block to coeffs, then flip to high→low and forward.
        let mut coeff_blocks: Vec<Vec<F>> = Vec::new();
        let mut collect = |eval_block: Vec<F>| {
            let coeffs = domain::ifft_block_evals_to_coeffs(domain, &eval_block);
            coeff_blocks.push(coeffs);
        };
        stream_evals(idx, &mut collect);

        for mut block in coeff_blocks.into_iter().rev() {
            block.reverse(); // high→low
            sink(block);
        }
    };

    open_at_points_with_coeffs(
        pcs_for_poly,
        commitments,
        |_i, _z| F::zero(),
        &mut as_coeff_hi_to_lo,
        points,
    )
}

// ===========================================================================
// Verification (unchanged math; production-hardened)
// ===========================================================================

pub fn verify_openings(
    _pcs: &PcsParams,
    commitments: &[Commitment],
    points: &[F],
    claimed_evals: &[F],
    proofs: &[OpeningProof],
) -> Result<(), VerifyError> {
    let expected = commitments.len().saturating_mul(points.len());
    if proofs.len() != expected || claimed_evals.len() != expected {
        return Err(VerifyError::Shape { expected, got: proofs.len().max(claimed_evals.len()) });
    }

    let g1_gen = {
        let guard = srs_g1().lock().expect("SRS G1 mutex poisoned");
        guard.get_power(0)
    };
    let g2_gen = <Bn254 as Pairing>::G2::generator().into_affine();
    let g2_tau = {
        let guard = srs_g2().lock().expect("SRS G2 mutex poisoned");
        match guard.tau_g2 {
            Some(t) => t,
            None => return Err(VerifyError::MissingG2),
        }
    };

    let mut a_all: Vec<<Bn254 as Pairing>::G1Prepared> = Vec::with_capacity(expected * 3);
    let mut b_all: Vec<<Bn254 as Pairing>::G2Prepared> = Vec::with_capacity(expected * 3);

    let mut idx = 0usize;
    for cmt in commitments.iter() {
        let c_aff = cmt.0;
        for &pt in points.iter() {
            let pr = &proofs[idx];
            let val = claimed_evals[idx];

            if pr.value != val || pr.zeta != pt {
                return Err(VerifyError::Pairing);
            }

            // e(C, G2)
            a_all.push(<Bn254 as Pairing>::G1Prepared::from(c_aff));
            b_all.push(<Bn254 as Pairing>::G2Prepared::from(g2_gen));

            // e(−f(ζ)·G1, G2)
            let minus_f_g1 = (-g1_gen.into_group().mul_bigint(val.into_bigint())).into_affine();
            a_all.push(<Bn254 as Pairing>::G1Prepared::from(minus_f_g1));
            b_all.push(<Bn254 as Pairing>::G2Prepared::from(g2_gen));

            // e(−W, [τ]G2 − ζ·G2)
            let right_g2 =
                (g2_tau.into_group() - g2_gen.into_group().mul_bigint(pt.into_bigint())).into_affine();
            let minus_w = (-pr.witness_comm.0).into_group().into_affine();
            a_all.push(<Bn254 as Pairing>::G1Prepared::from(minus_w));
            b_all.push(<Bn254 as Pairing>::G2Prepared::from(right_g2));

            idx += 1;
        }
    }

    if a_all.is_empty() {
        return Ok(());
    }

    let mlo = <Bn254 as Pairing>::multi_miller_loop(a_all, b_all);
    if let Some(fe) = <Bn254 as Pairing>::final_exponentiation(mlo) {
        if fe.0.is_one() {
            return Ok(());
        }
    }
    Err(VerifyError::Pairing)
}
