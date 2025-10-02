//! Fiat–Shamir transcript with domain separation (production-hardened)
//!
//! This module provides a **deterministic, label-stable** Fiat–Shamir (FS)
//! transform built on top of BLAKE3 with explicit domain-separation tags and
//! length-delimited absorbs, as required by the whitepaper’s FS discipline.
//!
//! ### Design highlights (whitepaper-aligned)
//! - **Stable DSTs.** Every absorb is prefixed by a fixed *domain separation
//!   tag* (DST) and a human-readable label. This guarantees the prover and
//!   verifier replay the exact same byte schedule.
//! - **Length-delimited items.** All absorbs include an explicit byte-length
//!   prefix to avoid ambiguity and concatenation pitfalls.
//! - **Clone-before-challenge.** Challenge derivation clones the running hash
//!   state and uses the BLAKE3 XOF, so deriving challenges does not mutate nor
//!   “consume” the transcript state.
//!
//! ### New in this revision
//! - `absorb_counter[_l]`: small helper to bind monotonically increasing
//!   counters or sizes (encoded big-endian).
//! - `absorb_vec_commitments[_l]`: helper to bind a *sequence* of PCS
//!   commitments in a single, length-delimited item.
//!
//! ### Rustdoc examples
//! The FS labels are **deterministic**: changing the label changes the
//! transcript and thus the derived challenges.
//!
//! ```
//! use myzkp::transcript::{Transcript, FsLabel};
//!
//! let mut t1 = Transcript::new("example");
//! t1.absorb_bytes_l(FsLabel::ProtocolHeader, b"hdr");
//! let a = t1.challenge_f_l(FsLabel::Alpha);
//!
//! let mut t2 = Transcript::new("example");
//! // Same data but a *different* label ⇒ different challenge.
//! t2.absorb_bytes_l(FsLabel::WireCommit, b"hdr");
//! let b = t2.challenge_f_l(FsLabel::Alpha);
//!
//! assert_ne!(a, b);
//! ```
//!
//! Binding the same sequence yields the same challenge:
//!
//! ```
//! use myzkp::transcript::{Transcript, FsLabel};
//!
//! let mut t1 = Transcript::new("example");
//! t1.absorb_counter_l(FsLabel::Beta, 42);
//! let a = t1.challenge_f_l(FsLabel::Gamma);
//!
//! let mut t2 = Transcript::new("example");
//! t2.absorb_counter_l(FsLabel::Beta, 42);
//! let b = t2.challenge_f_l(FsLabel::Gamma);
//!
//! assert_eq!(a, b);
//! ```

#![forbid(unsafe_code)]
#![allow(missing_docs)] // This module is heavily documented but kept permissive for internal items.

use ark_ff::PrimeField; // needed for from_le_bytes_mod_order
use ark_serialize::CanonicalSerialize;
use blake3::Hasher;
use std::io::Read; // needed for OutputReader::read

use crate::{pcs, F, ProofHeader};

/// Canonical labels to avoid typos across prover/verifier.
///
/// These stringified labels are part of the transcript’s **stable** domain
/// separation. Adding new variants is backward-compatible; reordering or
/// renaming existing ones is **not**.
#[derive(Clone, Copy, Debug)]
pub enum FsLabel {
    ProtocolHeader,
    SelectorCommit,
    WireCommit,
    PermZCommit,
    QuotientCommit,
    Beta,
    Gamma,
    Alpha,
    EvalPoints,
}

impl FsLabel {
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            FsLabel::ProtocolHeader => "protocol_header",
            FsLabel::SelectorCommit => "selector_commit",
            FsLabel::WireCommit => "wire_commit",
            FsLabel::PermZCommit => "perm_z_commit",
            FsLabel::QuotientCommit => "quotient_commit",
            FsLabel::Beta => "beta",
            FsLabel::Gamma => "gamma",
            FsLabel::Alpha => "alpha",
            FsLabel::EvalPoints => "eval_points",
        }
    }
}

/// Fiat–Shamir transcript with domain separation (BLAKE3-based).
pub struct Transcript {
    /// Domain-separation label for this transcript instance.
    label: &'static str,
    /// Running hash state (BLAKE3).
    hasher: Hasher,
    /// Monotone counter for challenge derivations.
    ctr: u64,
}

impl Transcript {
    /// Create a new transcript with a domain-separation `label`.
    ///
    /// The label distinguishes independent FS domains (e.g., proof types).
    pub fn new(label: &'static str) -> Self {
        let mut hasher = Hasher::new();
        // Domain separation preamble: fixed prefix + label.
        hasher.update(b"SSZKP.transcript.v1");
        hasher.update(label.as_bytes());
        Self { label, hasher, ctr: 0 }
    }

    // ---------------------------- Absorb (public) -----------------------------

    /// Absorb a PCS commitment using **compressed G1** encoding (enum label).
    #[inline]
    pub fn absorb_commitment_l(&mut self, label: FsLabel, c: &pcs::Commitment) {
        self.absorb_commitment(label.as_str(), c)
    }

    /// Absorb a PCS commitment using **compressed G1** encoding (legacy string).
    pub fn absorb_commitment(&mut self, label: &'static str, c: &pcs::Commitment) {
        let mut bytes = Vec::with_capacity(48); // BN254 compressed G1 is ~48 bytes
        c.0.serialize_compressed(&mut bytes).expect("serialize G1");
        self.absorb_bytes(label, &bytes);
    }

    /// Absorb a **vector** of PCS commitments as a single, length-delimited item (enum label).
    ///
    /// The vector encoding is:
    /// `u64(len) || Σ_i [ u64(commit_i_len) || commit_i_bytes ]`.
    #[inline]
    pub fn absorb_vec_commitments_l(&mut self, label: FsLabel, v: &[pcs::Commitment]) {
        self.absorb_vec_commitments(label.as_str(), v)
    }

    /// Absorb a **vector** of PCS commitments as a single, length-delimited item (legacy string).
    pub fn absorb_vec_commitments(&mut self, label: &'static str, v: &[pcs::Commitment]) {
        let mut buf = Vec::with_capacity(8 + v.len() * 64);
        buf.extend_from_slice(&(v.len() as u64).to_be_bytes());
        for c in v {
            let mut bytes = Vec::with_capacity(48);
            c.0.serialize_compressed(&mut bytes).expect("serialize G1");
            buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            buf.extend_from_slice(&bytes);
        }
        self.absorb_bytes(label, &buf);
    }

    /// Absorb an arbitrary byte slice with a label (enum).
    #[inline]
    pub fn absorb_bytes_l(&mut self, label: FsLabel, bytes: &[u8]) {
        self.absorb_bytes(label.as_str(), bytes)
    }

    /// Absorb an arbitrary byte slice with a label (length-delimited).
    pub fn absorb_bytes(&mut self, label: &'static str, bytes: &[u8]) {
        // Item preamble: stable DST + label + length + data.
        self.hasher.update(b"item:");
        self.hasher.update(label.as_bytes());
        self.hasher.update(b":len:");
        self.hasher.update(&(bytes.len() as u64).to_be_bytes());
        self.hasher.update(b":data:");
        self.hasher.update(bytes);
    }

    /// Absorb a field element `F` using compressed canonical serialization.
    #[inline]
    pub fn absorb_scalar_l(&mut self, label: FsLabel, f: &F) {
        let mut bytes = Vec::new();
        f.serialize_compressed(&mut bytes).expect("serialize field");
        self.absorb_bytes_l(label, &bytes);
    }

    /// Absorb a big-endian counter (e.g., sizes, indices) with the given label.
    ///
    /// This is a small convenience wrapper to avoid ad-hoc endian/width choices
    /// at call sites. Encoded as `u64::to_be_bytes`.
    #[inline]
    pub fn absorb_counter_l(&mut self, label: FsLabel, ctr: u64) {
        self.absorb_counter(label.as_str(), ctr)
    }

    /// Absorb a big-endian counter (legacy string label).
    #[inline]
    pub fn absorb_counter(&mut self, label: &'static str, ctr: u64) {
        self.absorb_bytes(label, &ctr.to_be_bytes());
    }

    /// Bind the **protocol header** (domain/PCS basics + SRS digests).
    pub fn absorb_protocol_header(&mut self, header: &ProofHeader) {
        let mut bytes = Vec::new();
        header.serialize_compressed(&mut bytes).expect("serialize header");
        self.absorb_bytes_l(FsLabel::ProtocolHeader, &bytes);
    }

    // -------------------------- Challenge (public) ----------------------------

    /// Derive a single field challenge `F` (enum label).
    ///
    /// Internally this clones the running state and applies an XOF, so calls
    /// are independent and do not mutate the absorb state (only the local
    /// derivation counter advances).
    #[inline]
    pub fn challenge_f_l(&mut self, label: FsLabel) -> F {
        self.challenge_f(label.as_str())
    }

    /// Derive a single field challenge `F` (legacy string).
    pub fn challenge_f(&mut self, label: &'static str) -> F {
        let out = hash_to_field(&self.hasher, self.label, label, self.ctr, 1);
        self.ctr = self.ctr.wrapping_add(1);
        out[0]
    }

    /// Derive `k` field challenges (enum label).
    #[inline]
    pub fn challenge_points_l(&mut self, label: FsLabel, k: usize) -> Vec<F> {
        self.challenge_points(label.as_str(), k)
    }

    /// Derive `k` field challenges (legacy string).
    pub fn challenge_points(&mut self, label: &'static str, k: usize) -> Vec<F> {
        let out = hash_to_field(&self.hasher, self.label, label, self.ctr, k);
        self.ctr = self.ctr.wrapping_add(1);
        out
    }
}

// ------------------------ Internals ------------------------

/// Derive `k` field elements from (a clone of) `base` using a fixed DST.
///
/// Cloning avoids consuming the in-flight transcript state and ensures that
/// challenge derivation is a *pure function* of the absorb schedule and the
/// (label, counter) tuple.
fn hash_to_field(
    base: &Hasher,
    tlabel: &'static str,
    label: &'static str,
    ctr: u64,
    k: usize,
) -> Vec<F> {
    let mut h = base.clone();
    // Challenge DST (stable and explicit).
    h.update(b"challenge:");
    h.update(b"SSZKP.v1");
    h.update(b":tlabel:");
    h.update(tlabel.as_bytes());
    h.update(b":label:");
    h.update(label.as_bytes());
    h.update(b":ctr:");
    h.update(&ctr.to_be_bytes());

    // XOF → k * 64 bytes, then reduce to field (little-endian).
    let mut xof = h.finalize_xof();
    let mut out = Vec::with_capacity(k);
    let mut buf = [0u8; 64];
    for _ in 0..k {
        let _ = xof.read(&mut buf);
        out.push(F::from_le_bytes_mod_order(&buf));
    }
    out
}
