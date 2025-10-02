// src/api.rs
//! tinyzkp.com — “happy-path” crate API
//!
//! This module wraps the protocol core with a small, ergonomic surface area:
//! - `ProverBuilder` / `VerifierBuilder` hide PCS/domain wiring (safe defaults)
//! - one-shot `prove_from_rows` / `prove_from_stream` (sublinear path)
//! - adapters: `VecRows`, `CsvRows` (streamed) for easy integration
//! - v2 proof I/O helpers: `io::write_proof` / `io::read_proof`
//! - simple `Tuning` & `estimate_peak_memory`
//!
//! Everything delegates to the existing `scheduler::{Prover,Verifier}` and
//! respects the whitepaper’s streaming discipline. No protocol changes.

#![forbid(unsafe_code)]

use std::{fs, io::{BufRead, BufReader}, path::{Path, PathBuf}};

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};

use crate::{
    air::{self, Row, AirSpec},
    domain::{self, Domain},
    pcs::{self, Basis, PcsParams},
    scheduler,
    stream::{RowIdx},
    F, Proof, ProveParams, VerifyParams,
};

// ===============================================================================================
// Builders
// ===============================================================================================

/// Ergonomic constructor for a `scheduler::Prover`.
///
/// Defaults:
/// - wires basis: `Basis::Evaluation`
/// - `zh_c`: taken from the `Domain` (header-authoritative)
/// - `b_blk`: 128 (override as needed)
pub struct ProverBuilder {
    domain: Domain,
    air: AirSpec,
    b_blk: usize,
    basis_wires: Basis,
}
impl ProverBuilder {
    pub fn new(domain: Domain, air: AirSpec) -> Self {
        Self { domain, air, b_blk: 128, basis_wires: Basis::Evaluation }
    }
    /// Set the tile/block length used across Blocked-IFFT and openings.
    pub fn b_blk(mut self, b: usize) -> Self { self.b_blk = b.max(1); self }
    /// Choose the basis used for wire commitments (coeff or eval).
    pub fn wires_basis(mut self, basis: Basis) -> Self { self.basis_wires = basis; self }

    /// Build the prover with consistent PCS params (Q is always coefficient-basis).
    pub fn build(self) -> scheduler::Prover<'static> {
        let pcs_wires = PcsParams {
            max_degree: self.domain.n - 1,
            basis: self.basis_wires,
            srs_placeholder: (),
        };
        let pcs_coeff = PcsParams {
            max_degree: self.domain.n - 1,
            basis: Basis::Coefficient,
            srs_placeholder: (),
        };
        let params = ProveParams { domain: self.domain.clone(), pcs_wires, pcs_coeff, b_blk: self.b_blk };
        scheduler::Prover { air: Box::leak(Box::new(self.air)), params: Box::leak(Box::new(params)) }
    }
}

/// Ergonomic constructor for a `scheduler::Verifier`.
pub struct VerifierBuilder {
    domain: Domain,
    basis_wires: Basis,
}
impl VerifierBuilder {
    pub fn new(domain: Domain) -> Self {
        Self { domain, basis_wires: Basis::Evaluation }
    }
    /// Choose the basis used for *wire* commitments. The proof header remains authoritative.
    pub fn wires_basis(mut self, basis: Basis) -> Self { self.basis_wires = basis; self }
    pub fn build(self) -> scheduler::Verifier<'static> {
        let pcs_wires = PcsParams {
            max_degree: self.domain.n - 1,
            basis: self.basis_wires,
            srs_placeholder: (),
        };
        let pcs_coeff = PcsParams {
            max_degree: self.domain.n - 1,
            basis: Basis::Coefficient,
            srs_placeholder: (),
        };
        let params = VerifyParams { domain: self.domain, pcs_wires, pcs_coeff };
        scheduler::Verifier { params: Box::leak(Box::new(params)) }
    }
}

// ===============================================================================================
/* One-shot helpers */
// ===============================================================================================

/// Prove from an in-memory witness (small/medium traces).
///
/// Internally wraps the `Vec<Row>` with the repo’s built-in `Restreamer` impl
/// and delegates to `scheduler::Prover::prove_with_restreamer`.
pub fn prove_from_rows(
    prover: &scheduler::Prover,
    rows: Vec<Row>,
) -> anyhow::Result<Proof> {
    prover.prove_with_restreamer(&rows).map_err(|e| anyhow::anyhow!("prover failed: {e}"))
}

/// Prove from a streaming witness (sublinear path).
///
/// Pass any `Restreamer<Item=Row>` source (e.g., `CsvRows` adapter below).
pub fn prove_from_stream(
    prover: &scheduler::Prover,
    restreamer: &impl crate::stream::Restreamer<Item = Row>,
) -> anyhow::Result<Proof> {
    prover.prove_with_restreamer(restreamer).map_err(|e| anyhow::anyhow!("prover failed: {e}"))
}

/// Verify a proof with the given verifier params (header/domain enforced by scheduler).
pub fn verify(
    verifier: &scheduler::Verifier,
    proof: &Proof,
) -> anyhow::Result<()> {
    verifier.verify(proof).map_err(|e| anyhow::anyhow!("verification failed: {e}"))
}

// ===============================================================================================
/* Adapters: easy witness sources */
// ===============================================================================================

pub mod adapters {
    //! Data-source adapters that implement the repo’s `Restreamer<Item=Row>` trait.
    //!
    //! - `VecRows`: trivial adapter for in-memory data.
    //! - `CsvRows`: streamed CSV (one row per line, comma/whitespace delimited).
    //!
    //! Both adapters support *re-streaming*; `CsvRows` re-opens the file for
    //! each block request (cheap, predictable RAM).

    use super::*;
    use crate::stream::Restreamer;

    /// Trivial in-memory adapter.
    pub struct VecRows(pub Vec<Row>);
    impl Restreamer for VecRows {
        type Item = Row;
        fn len_rows(&self) -> usize { self.0.len() }
        fn stream_rows(&self, start: RowIdx, end: RowIdx) -> Box<dyn Iterator<Item = Row>> {
            let s = start.0.min(self.0.len());
            let e = end.0.min(self.0.len());
            Box::new(self.0[s..e].iter().cloned())
        }
    }

    /// Streamed CSV adapter.
    ///
    /// Format assumptions:
    /// - one witness row per line
    /// - values separated by comma **or** ASCII whitespace
    /// - exactly `k` registers per row (extra tokens are rejected; missing tokens error)
    ///
    /// Rows can be greater than the domain size; scheduler will pad/truncate as usual.
    pub struct CsvRows {
        path: PathBuf,
        k: usize,
        rows: usize, // cached count for len_rows()
    }

    impl CsvRows {
        /// Create a CSV adapter. `k` is the number of registers per row.
        pub fn new_from_path(path: impl Into<PathBuf>, k: usize) -> anyhow::Result<Self> {
            let path = path.into();
            let rows = Self::count_rows(&path)?;
            Ok(Self { path, k, rows })
        }

        fn count_rows(path: &Path) -> anyhow::Result<usize> {
            let f = fs::File::open(path)
                .map_err(|e| anyhow::anyhow!("open witness csv {}: {e}", path.display()))?;
            let mut n = 0usize;
            for line in BufReader::new(f).lines() {
                let l = line?;
                if !l.trim().is_empty() { n += 1; }
            }
            Ok(n)
        }

        fn parse_slice<'a>(&self, start: usize, end: usize) -> anyhow::Result<Vec<Row>> {
            let f = fs::File::open(&self.path)
                .map_err(|e| anyhow::anyhow!("open witness csv {}: {e}", self.path.display()))?;
            let mut out = Vec::with_capacity(end.saturating_sub(start));
            let mut cur = 0usize;

            for line in BufReader::new(f).lines() {
                let l = line?;
                if l.trim().is_empty() { continue; }
                if cur >= end { break; }
                if cur >= start {
                    let mut regs = Vec::with_capacity(self.k);
                    for tok in l.split(|c: char| c == ',' || c.is_ascii_whitespace()) {
                        if tok.is_empty() { continue; }
                        if regs.len() == self.k {
                            return Err(anyhow::anyhow!(
                                "csv row {} has more than k={} fields", cur, self.k
                            ));
                        }
                        // Parse as u128→F. Adjust here if you want hex, etc.
                        let v = tok.parse::<u128>().map_err(|e| {
                            anyhow::anyhow!("csv parse error at row {} token `{}`: {}", cur, tok, e)
                        })?;
                        regs.push(F::from(v as u64));
                    }
                    if regs.len() != self.k {
                        return Err(anyhow::anyhow!(
                            "csv row {} has {} fields, expected k={}",
                            cur, regs.len(), self.k
                        ));
                    }
                    out.push(Row { regs: regs.into_boxed_slice() });
                }
                cur += 1;
            }
            // It is valid for (end > actual_rows); we just return fewer rows.
            Ok(out)
        }
    }

    impl Restreamer for CsvRows {
        type Item = Row;
        fn len_rows(&self) -> usize { self.rows }
        fn stream_rows(&self, start: RowIdx, end: RowIdx) -> Box<dyn Iterator<Item = Row>> {
            // Convert the parsing error into a panic-free iterator (the scheduler never expects Err here).
            let s = start.0;
            let e = end.0;
            let parsed = self.parse_slice(s, e)
                .unwrap_or_else(|e| panic!("CsvRows::stream_rows parse error: {e}"));
            Box::new(parsed.into_iter())
        }
    }
}

// ===============================================================================================
/* v2 Proof I/O (magic + version + ark-compressed) */
// ===============================================================================================

pub mod io {
    use super::*;

    /// 8-byte magic used by `prover`/`verifier` CLIs.
    pub const FILE_MAGIC: &[u8; 8] = b"SSZKPv2\0";
    pub const FILE_VERSION: u16 = 2;

    /// Write a v2 proof file at `path`.
    pub fn write_proof(path: &Path, proof: &Proof) -> anyhow::Result<()> {
        let mut payload = Vec::new();
        proof.serialize_compressed(&mut payload)
            .map_err(|e| anyhow::anyhow!("serialize proof: {e}"))?;
        let mut f = fs::File::create(path)
            .map_err(|e| anyhow::anyhow!("create {}: {e}", path.display()))?;
        use std::io::Write;
        f.write_all(FILE_MAGIC)?;
        f.write_all(&FILE_VERSION.to_be_bytes())?;
        f.write_all(&payload)?;
        f.flush().ok();
        Ok(())
    }

    /// Read a v2 proof file from `path`.
    pub fn read_proof(path: &Path) -> anyhow::Result<Proof> {
        let mut f = fs::File::open(path)
            .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
        use std::io::Read;
        let mut magic = [0u8; 8];
        f.read_exact(&mut magic)?;
        if &magic != FILE_MAGIC {
            return Err(anyhow::anyhow!("bad proof file magic (expected v2)"));
        }
        let mut ver = [0u8; 2];
        f.read_exact(&mut ver)?;
        let file_ver = u16::from_be_bytes(ver);
        if file_ver != FILE_VERSION {
            return Err(anyhow::anyhow!("unsupported proof version: {file_ver}"));
        }
        let mut payload = Vec::new();
        f.read_to_end(&mut payload)?;
        let mut slice = payload.as_slice();
        let proof: Proof = CanonicalDeserialize::deserialize_compressed(&mut slice)
            .map_err(|e| anyhow::anyhow!("deserialize proof: {e}"))?;
        Ok(proof)
    }
}

// ===============================================================================================
/* Tuning & Introspection */
// ===============================================================================================

/// Non-binding knobs (compile-time features still win). Useful for dashboards.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub b_blk: usize,
    pub strict_recompute_r: bool,
    pub zeta_shift_enabled: bool,
    pub lookups_enabled: bool,
}
impl Default for Tuning {
    fn default() -> Self {
        Self {
            b_blk: 128,
            strict_recompute_r: cfg!(feature = "strict-recompute-r"),
            zeta_shift_enabled: cfg!(feature = "zeta-shift"),
            lookups_enabled: cfg!(feature = "lookups"),
        }
    }
}

/// Rough peak RSS estimate (bytes) from **b_blk** and **k**.
/// It’s deliberately conservative and only for operator guidance.
pub fn estimate_peak_memory(b_blk: usize, k_regs: usize) -> usize {
    use core::mem::size_of;
    let fsz = size_of::<F>().max(32);        // BN254 Fr ~ 32B (conservative)
    let tiles_in_flight = 2;                  // prefetch: current + next
    let wires_tile = k_regs * b_blk * fsz;    // simultaneous wire tiles
    let z_tile = b_blk * fsz;
    let quotient_tile = b_blk * fsz;
    let overhead = 64 * 1024;                 // stack/scratch
    tiles_in_flight * (wires_tile + z_tile + quotient_tile) + overhead
}

// ===============================================================================================
// Examples (doc tests)
//
// ```ignore
// // Build AIR & Domain
// let k = 3usize;
// let rows = 1024usize;
// let air = crate::air::AirSpec { k, id_table: vec![], sigma_table: vec![], selectors: vec![] };
// let n = rows.next_power_of_two();
// let omega = crate::F::get_root_of_unity(n as u64).unwrap();
// let domain = crate::domain::Domain { n, omega, zh_c: crate::F::from(1u64) };
//
// // Prover / Verifier
// let prover = crate::api::ProverBuilder::new(domain.clone(), air).b_blk(128).build();
// let verifier = crate::api::VerifierBuilder::new(domain).build();
//
// // Deterministic demo witness
// let mut w = Vec::with_capacity(rows);
// for i in 0..rows {
//     let base = crate::F::from((i as u64) + 1);
//     let mut regs = vec![crate::F::default(); k];
//     for m in 0..k { regs[m] = base.pow([(m as u64) + 1]); }
//     w.push(crate::air::Row { regs: regs.into_boxed_slice() });
// }
//
// // Prove & verify
// let proof = crate::api::prove_from_rows(&prover, w)?;
// crate::api::verify(&verifier, &proof)?;
// crate::api::io::write_proof(std::path::Path::new("proof.bin"), &proof)?;
// ```
// ===============================================================================================
