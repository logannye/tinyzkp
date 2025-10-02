//! Crate root: public surface, core aliases, and protocol-wide invariants
//!
//! This module is the **single canonical entry-point** for downstream users of
//! the library. It centralizes the scalar field alias, the small index newtypes,
//! shared error categories, and re-exports the main submodules that implement
//! the whitepaper’s design.
//!
//! ## Invariants (whitepaper-aligned)
//!
//! - **Field & Curve.** Unless explicitly configured otherwise, the scalar field
//!   is `ark_bn254::Fr` (`F` in this crate). Commitments use KZG on BN254
//!   (`G1 = ark_bn254::G1Affine`). All arithmetic is constant-time as provided
//!   by Arkworks; we **forbid unsafe** throughout the crate.
//!
//! - **Evaluation domain.** The vanishing polynomial is
//!   `Z_H(X) = X^N − c` where `N` is a power of two and `ω` is a generator of
//!   the size-`N` multiplicative subgroup (`ω^N = 1`, `ω^{N/2} ≠ 1` when
//!   `N ≥ 2`). The constant `c = zh_c` is carried in the header and used by the
//!   quotient construction and the algebraic check at `ζ`.
//!
//! - **Streaming discipline.** All core builders (wires, Z, Q) are wired so that
//!   peak memory is `O(b_blk)`, with tiles flowing time-ordered through bounded
//!   buffers. No API in this crate requires materializing a full polynomial when
//!   a streamed form exists.
//!
//! - **Fiat–Shamir (FS).** We use BLAKE3 with **explicit domain separation**
//!   tags, length-delimited absorbs, and an **XOF** to derive challenges. The
//!   prover and verifier replay the exact same sequence of absorbs/challenges.
//!
//! These invariants are enforced by design across the submodules and are
//! serialized into the `ProofHeader`. If any invariant is violated at runtime,
//! the failure mode is a **precise error** (never UB).

#![forbid(unsafe_code)]
#![deny(missing_docs, rust_2018_idioms)]

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};

/// Domain & transforms (vanishing polynomial X^N − c, blocked IFFT/NTT).
pub mod domain;
/// Polynomial commitment scheme interface and linear aggregator (KZG by default).
pub mod pcs;
/// Fiat–Shamir transcript (domain-separated hashing, hash→field).
pub mod transcript;
/// AIR template & block evaluator (local transitions / locals tuple).
pub mod air;
/// Permutation & lookup accumulators (multiplicative, time-ordered).
pub mod perm_lookup;
/// Streaming/blocking utilities and O(b_blk) workspace.
pub mod stream;
/// Quotient builder (blocked IFFT + X^N − c coefficient recurrence).
pub mod quotient;
/// Streaming polynomial evaluation (barycentric / Horner).
pub mod opening;
/// Five-phase scheduler orchestrating A–E with aggregate-only FS discipline.
pub mod scheduler;
/// SRS setup and management (trusted ceremony integration)
pub mod srs_setup;

// ============================================================================
// Canonical aliases and root-level re-exports (centralization)
// ============================================================================

/// Scalar field used across the crate (BN254 by default).
pub type F = ark_bn254::Fr;

/// G1 affine group element used for commitments (KZG default).
pub type G1 = ark_bn254::G1Affine;

/// Security parameter λ. In the manuscript, λ = Θ(log T) is implicit;
/// we **do not** hardwire T here.
pub const SECURITY_LAMBDA: usize = 128;

/// Centralized index newtypes used across the crate.
///
/// These are re-exported from `stream` to avoid duplication and to keep a
/// single definition site. Downstream code should import them from the crate
/// root (e.g., `use myzkp::{BlockIdx, RowIdx, RegIdx};`).
pub use crate::stream::{BlockIdx, RegIdx, RowIdx};

/// Streaming/shape errors that are shared by helpers across modules.
///
/// For now, we unify on the error used by the streaming utilities and expose it
/// at the crate root. Additional common error categories can be added here
/// (e.g., parameter validation) without breaking downstream callers.
pub use crate::stream::StreamError;

// ---------------------- Back-compat shims (doc-hidden) -----------------------

/// Compatibility namespace with doc-hidden re-exports to **avoid churn** in
/// downstream code during the refactor. Prefer importing from the crate root.
///
/// ```ignore
/// // New (preferred)
/// use myzkp::{BlockIdx, RowIdx, RegIdx, StreamError};
///
/// // Old (still works):
/// use myzkp::compat::{BlockIdx, RowIdx, RegIdx, StreamError};
/// ```
#[doc(hidden)]
pub mod compat {
    pub use crate::stream::{BlockIdx, RegIdx, RowIdx, StreamError};
}

// ============================================================================
// Public orchestrators
// ============================================================================

/// Re-export the real orchestrators implemented in `scheduler.rs`.
pub use scheduler::{Prover, Verifier};

/// Re-export PCS surface types so downstream code uses the **single, canonical**
/// definitions that already implement Arkworks serialization traits.
pub use crate::pcs::{Basis, Commitment, OpeningProof, PcsParams, SrsLoadError, VerifyError};

// ============================================================================
// Public parameter structs and proof types
// ============================================================================

/// Parameters required by the prover.
///
/// These parameters reflect public context (domain/SRS) as well as internal
/// streaming shape (`b_blk`). They must be coherent across modules.
#[derive(Clone, Debug)]
pub struct ProveParams {
    /// Evaluation/coset domain and vanishing polynomial descriptor.
    pub domain: crate::domain::Domain,
    /// PCS parameters used for wires (basis discipline must match scheduler).
    pub pcs_wires: crate::pcs::PcsParams,
    /// PCS parameters used for coefficient-basis commitments (e.g., Q).
    pub pcs_coeff: crate::pcs::PcsParams,
    /// Block size used by the streaming prover (`b_blk ≈ √T`).
    ///
    /// **Invariant:** `b_blk > 0`. Many streaming helpers validate this and
    /// return `StreamError::BadBlockSize` or panic in legacy wrappers.
    pub b_blk: usize,
}

/// Parameters required by the verifier.
///
/// These must match the prover’s public parameters (same domain/SRS/bases).
#[derive(Clone, Debug)]
pub struct VerifyParams {
    /// Evaluation/coset domain and vanishing polynomial descriptor.
    pub domain: crate::domain::Domain,
    /// PCS parameters used for wires (basis must match prover).
    pub pcs_wires: crate::pcs::PcsParams,
    /// PCS parameters used for coefficient-basis commitments (e.g., Q).
    pub pcs_coeff: crate::pcs::PcsParams,
}

/// Versioned, serializable **protocol header** bound into the transcript.
///
/// The header is absorbed first and includes the **domain** and **SRS digests**
/// to guarantee the prover and verifier agree on the exact context.
///
/// Serialization uses Arkworks canonical compressed encodings.
#[derive(Clone, Debug, CanonicalSerialize, CanonicalDeserialize)]
pub struct ProofHeader {
    /// Header format / protocol version.
    pub version: u16,
    /// Domain size N.
    pub domain_n: u32,
    /// Subgroup generator ω.
    pub domain_omega: F,
    /// Constant c in Z_H(X) = X^N − c.
    pub zh_c: F,
    /// Number of registers (k).
    pub k: u16,
    /// Commitment basis for wires (Eval or Coeff).
    pub basis_wires: crate::pcs::Basis,
    /// Digest of the loaded G1 SRS powers (compressed).
    pub srs_g1_digest: [u8; 32],
    /// Digest of the loaded G2 SRS element(s) (compressed).
    pub srs_g2_digest: [u8; 32],
}

/// The SSZKP proof object.
///
/// Note: `Commitment` here is re-exported from `pcs` and already implements
/// `CanonicalSerialize` / `CanonicalDeserialize`, so these derives work.
#[derive(Clone, Debug, CanonicalSerialize, CanonicalDeserialize)]
pub struct Proof {
    /// Protocol header bound into FS (domain / PCS basics + SRS digests).
    pub header: ProofHeader,

    /// Per-register wire commitments (aggregated across blocks; order `m = 0..k-1`).
    ///
    /// These are absorbed into the transcript **in order** before sampling `(β, γ)`.
    pub wire_comms: Vec<Commitment>,

    /// Optional permutation accumulator commitment `Z` (if committed by the scheme).
    ///
    /// If present, it is absorbed **after** sampling `(β, γ)` and **before** sampling `α`.
    pub z_comm: Option<Commitment>,

    /// Quotient commitment `Q` (coefficient-basis).
    ///
    /// This is absorbed **after** sampling `α` and **before** sampling the evaluation points.
    pub q_comm: Commitment,

    /// Evaluation points sampled via FS (e.g., `[ζ, …]`).
    ///
    /// The prover and verifier derive these *after* absorbing `Q`, using the same transcript state.
    pub eval_points: Vec<F>,

    /// Claimed evaluations flattened in **poly-major, point-minor** order
    /// matching the opening set `[ C_wire[0..k-1], (C_Z?), C_Q ]`.
    pub evals: Vec<F>,

    /// PCS opening proofs corresponding 1-to-1 with `evals` in the **same order**.
    pub opening_proofs: Vec<OpeningProof>,
}
