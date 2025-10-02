//! Domain & Transform Primitives
//!
//! Evaluation domain `H` with vanishing polynomial `Z_H(X) = X^N − zh_c`,
//! streaming barycentric evaluation, radix-2 NTT/IFFT, and **tile emitters**
//! used by commitment/opening streams (never materialize full polynomials).
//!
//! ## This revision
//! - **Blocked IFFT producers**:
//!   - `ifft_time_stream_to_coeff_tiles` (emit tiles **low→high**)
//!   - `ifft_time_stream_to_coeff_tiles_hi_to_lo` (emit tiles **high→low**)
//!   These are implemented via a stable `BlockedIfft` façade which can keep
//!   memory ≈ `O(b_blk)`.
//! - **Validation**: `ω^N = 1` and `ω^{N/p} ≠ 1` for all prime divisors `p|N`,
//!   and `zh_c ≠ 0`. (Moved here from the CLI so all call-sites benefit.)
//! - **Vanishing polynomial**: we explicitly model `Z_H(X) = X^N − zh_c` and
//!   carry `zh_c` in `Domain` so pads/cosets and extended variants are easy.
//!
//! All public APIs are conservative and production-ready; the file-backed,
//! *in-place* blocked IFFT is optional and off by default, but the façade keeps
//! the same streaming shape either way.

#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(dead_code)]
#![allow(unused_imports)]

use ark_ff::{Field, One, Zero};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use blake3::Hasher;

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::F;

/// Evaluation domain with vanishing polynomial `Z_H(X) = X^N - zh_c`.
#[derive(Debug, Clone, CanonicalSerialize, CanonicalDeserialize)]
pub struct Domain {
    /// Domain size `N`. Implementations in this crate assume `N` is a power of two.
    pub n: usize,
    /// Generator `ω` of the multiplicative subgroup `H = {1, ω, …, ω^{N-1}}`.
    pub omega: F,
    /// The constant `c` in `Z_H(X) = X^N − c`. For a pure subgroup, `c = 1`.
    pub zh_c: F,
}

/// Errors produced by domain checks / transforms.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("domain size must be positive")]
    NZero,
    #[error("zh_c must be non-zero")]
    ZhZero,
    #[error("omega^N != 1")]
    OmegaNPowNotOne,
    #[error("omega is not primitive: omega^(N/{0}) == 1")]
    OmegaNotPrimitive(usize),
    #[error("length must be positive power-of-two and divide N (len={len}, N={n})")]
    BadLen { len: usize, n: usize },
    #[error("evaluation point ζ lies in H (ζ^N == zh_c)")]
    ZetaInDomain,
    #[error("time stream length must be exactly N (got {got}, N={n})")]
    BadStream { got: usize, n: usize },
}

impl Domain {
    /// Construct a domain with explicit `zh_c`, returning a checked result.
    pub fn new_with_c_r(n: usize, omega: F, zh_c: F) -> Result<Self, DomainError> {
        let d = Self { n, omega, zh_c };
        validate_domain_r(&d)?;
        Ok(d)
    }
    /// Construct a domain with explicit `zh_c` (panics on invalid input).
    pub fn new_with_c(n: usize, omega: F, zh_c: F) -> Self {
        Self::new_with_c_r(n, omega, zh_c).expect("invalid domain")
    }

    /// Construct a coset domain: `Z_H(X) = X^N − s^N` where `s` is a coset shift.
    pub fn new_with_coset_r(n: usize, omega: F, coset_shift: F) -> Result<Self, DomainError> {
        let zh_c = pow_u64(coset_shift, n as u64);
        Self::new_with_c_r(n, omega, zh_c)
    }
    /// Construct a coset domain (panics on invalid input).
    pub fn new_with_coset(n: usize, omega: F, coset_shift: F) -> Self {
        Self::new_with_coset_r(n, omega, coset_shift).expect("invalid domain")
    }
}

/// Barycentric weights for streaming evaluation over a multiplicative subgroup.
#[derive(Debug, Clone)]
pub struct BarycentricWeights {
    inv_n: F,
    step: F, // step = ω^{-(N-1)}
}

#[inline]
fn pow_u64(mut base: F, mut exp: u64) -> F {
    let mut acc = F::one();
    while exp > 0 {
        if (exp & 1) == 1 {
            acc *= base;
        }
        base.square_in_place();
        exp >>= 1;
    }
    acc
}

// ------------------------- Hygiene / Validation -------------------------

fn prime_factors(mut n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut p = 2usize;
    while p * p <= n {
        if n % p == 0 {
            out.push(p);
            while n % p == 0 {
                n /= p;
            }
        }
        p += if p == 2 { 1 } else { 2 }; // 2,3,5,7,...
    }
    if n > 1 {
        out.push(n);
    }
    out
}

fn validate_domain_r(d: &Domain) -> Result<(), DomainError> {
    if d.n == 0 {
        return Err(DomainError::NZero);
    }
    if d.zh_c.is_zero() {
        return Err(DomainError::ZhZero);
    }

    // ω^N == 1 (fast pow)
    let w_n = pow_u64(d.omega, d.n as u64);
    if !w_n.is_one() {
        return Err(DomainError::OmegaNPowNotOne);
    }

    // ω is primitive: for each prime p|N, ω^{N/p} != 1
    for p in prime_factors(d.n) {
        let w_np = pow_u64(d.omega, (d.n / p) as u64);
        if w_np.is_one() {
            return Err(DomainError::OmegaNotPrimitive(p));
        }
    }
    Ok(())
}
fn validate_domain(d: &Domain) {
    validate_domain_r(d).expect("invalid domain");
}

// ------------------------- Digest (for debugging/metadata) -------------------------

/// Stable 32-byte digest of a `Domain` used in logs and sanity checks.
pub fn domain_digest(d: &Domain) -> [u8; 32] {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(d.n as u64).to_be_bytes());
    d.omega
        .serialize_compressed(&mut bytes)
        .expect("serialize omega");
    d.zh_c
        .serialize_compressed(&mut bytes)
        .expect("serialize zh_c");
    let mut h = Hasher::new();
    h.update(b"SSZKP.domain.v1");
    h.update(&bytes);
    *h.finalize().as_bytes()
}

// ------------------------- Vanishing helpers -------------------------

#[inline]
pub fn vanishing_at(d: &Domain, z: F) -> F {
    pow_u64(z, d.n as u64) - d.zh_c
}
#[inline]
pub fn is_in_domain(d: &Domain, z: F) -> bool {
    vanishing_at(d, z).is_zero()
}
#[inline]
pub fn assert_not_in_domain_r(d: &Domain, z: F) -> Result<(), DomainError> {
    if is_in_domain(d, z) {
        Err(DomainError::ZetaInDomain)
    } else {
        Ok(())
    }
}

// ------------------------- Barycentric (streaming) -------------------------

pub fn bary_weights_r(d: &Domain) -> Result<BarycentricWeights, DomainError> {
    validate_domain_r(d)?;
    let inv_n = F::from(d.n as u64).inverse().expect("N non-zero");
    // ω^{N-1} then invert -> step = ω^{-(N-1)}
    let omega_pow_n_minus_1 = pow_u64(d.omega, (d.n as u64).saturating_sub(1));
    let step = omega_pow_n_minus_1.inverse().expect("non-zero");
    Ok(BarycentricWeights { inv_n, step })
}
pub fn bary_weights(d: &Domain) -> BarycentricWeights {
    bary_weights_r(d).expect("invalid domain")
}

pub fn eval_stream_barycentric_r(
    d: &Domain,
    it: impl Iterator<Item = F>,
    zeta: F,
    w: &BarycentricWeights,
) -> Result<F, DomainError> {
    validate_domain_r(d)?;
    assert_not_in_domain_r(d, zeta)?;
    let mut omega_i = F::one();
    let mut w_i = w.inv_n;
    let mut num = F::zero();
    let mut den = F::zero();
    for f_i in it {
        if zeta == omega_i {
            return Ok(f_i);
        }
        let denom_term = (zeta - omega_i).inverse().expect("ζ ∉ H");
        num += w_i * f_i * denom_term;
        den += w_i * denom_term;
        omega_i *= d.omega;
        w_i *= w.step;
    }
    Ok(num * den.inverse().expect("den != 0"))
}
pub fn eval_stream_barycentric(
    d: &Domain,
    it: impl Iterator<Item = F>,
    zeta: F,
    w: &BarycentricWeights,
) -> F {
    eval_stream_barycentric_r(d, it, zeta, w).expect("barycentric failed")
}

// ------------------------- FFT / IFFT (fixed-size blocks) -------------------------

#[inline]
fn validate_len_r(d: &Domain, len: usize) -> Result<(), DomainError> {
    validate_domain_r(d)?;
    if !(len > 0 && len.is_power_of_two() && d.n % len == 0) {
        return Err(DomainError::BadLen { len, n: d.n });
    }
    Ok(())
}
#[inline]
fn primitive_len_root_r(d: &Domain, len: usize) -> Result<F, DomainError> {
    validate_len_r(d, len)?;
    Ok(pow_u64(d.omega, (d.n / len) as u64))
}

fn ntt_in_place(a: &mut [F], root: F) {
    let n = a.len();
    debug_assert!(n.is_power_of_two());

    // bit-reversal
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            a.swap(i, j);
        }
    }

    // Cooley–Tukey
    let mut len = 2;
    while len <= n {
        let w_len = pow_u64(root, (n / len) as u64);
        for start in (0..n).step_by(len) {
            let mut w = F::one();
            let half = len / 2;
            for i in 0..half {
                let u = a[start + i];
                let v = a[start + i + half] * w;
                a[start + i] = u + v;
                a[start + i + half] = u - v;
                w *= w_len;
            }
        }
        len <<= 1;
    }
}
fn intt_in_place(a: &mut [F], root: F) {
    let n = a.len();
    debug_assert!(n.is_power_of_two());
    let inv_root = root.inverse().expect("root non-zero");
    ntt_in_place(a, inv_root);
    let inv_n = F::from(n as u64).inverse().expect("n != 0");
    for x in a.iter_mut() {
        *x *= inv_n;
    }
}

pub fn ifft_block_evals_to_coeffs_r(d: &Domain, evals: &[F]) -> Result<Vec<F>, DomainError> {
    let m = evals.len();
    primitive_len_root_r(d, m)?; // validates len too
    let mut a = evals.to_vec();
    let root = primitive_len_root_r(d, m)?;
    intt_in_place(&mut a, root);
    Ok(a)
}
pub fn ifft_block_evals_to_coeffs(d: &Domain, evals: &[F]) -> Vec<F> {
    ifft_block_evals_to_coeffs_r(d, evals).expect("bad IFFT length")
}
pub fn ntt_block_coeffs_to_evals_r(d: &Domain, coeffs: &[F]) -> Result<Vec<F>, DomainError> {
    let m = coeffs.len();
    primitive_len_root_r(d, m)?; // validates len too
    let mut a = coeffs.to_vec();
    let root = primitive_len_root_r(d, m)?;
    ntt_in_place(&mut a, root);
    Ok(a)
}
pub fn ntt_block_coeffs_to_evals(d: &Domain, coeffs: &[F]) -> Vec<F> {
    ntt_block_coeffs_to_evals_r(d, coeffs).expect("bad NTT length")
}

// -----------------------------------------------------------------------------
// Coefficient tile emission order tag (consumed by PCS)
// -----------------------------------------------------------------------------

/// Public emission order for coefficient **tiles** produced by blocked IFFT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoeffTileOrder {
    LowToHigh,
    HighToLow,
}

// -----------------------------------------------------------------------------
// File-backed tape for blocked transforms (optional path)
// -----------------------------------------------------------------------------

#[derive(Debug)]
struct SpillTape {
    path: PathBuf,
    file: File,
    elem_size: usize,
    len: usize, // number of elements written (logical)
}

impl SpillTape {
    fn create() -> std::io::Result<Self> {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "sszkp_tape_{}_{}.bin",
            std::process::id(),
            blake3::hash(format!("{:?}", std::time::SystemTime::now()).as_bytes())
        );
        path.push(unique);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self { path, file, elem_size: 0, len: 0 })
    }

    fn remove(&mut self) {
        // Best-effort cleanup.
        let _ = std::fs::remove_file(&self.path);
    }

    #[inline]
    fn ensure_elem_size(&mut self, first: &F) -> std::io::Result<()> {
        if self.elem_size == 0 {
            let mut buf = Vec::with_capacity(64);
            first
                .serialize_compressed(&mut buf)
                .expect("field serialize");
            self.elem_size = buf.len();
        }
        Ok(())
    }

    fn append_block(&mut self, block: &[F]) -> std::io::Result<()> {
        if block.is_empty() {
            return Ok(());
        }
        self.ensure_elem_size(&block[0])?;
        self.file.seek(SeekFrom::End(0))?;
        let mut tmp = Vec::with_capacity(self.elem_size);
        for x in block {
            tmp.clear();
            x.serialize_compressed(&mut tmp).expect("field serialize");
            debug_assert_eq!(tmp.len(), self.elem_size);
            self.file.write_all(&tmp)?;
            self.len += 1;
        }
        Ok(())
    }

    #[inline]
    fn seek_elem(&mut self, idx: usize) -> std::io::Result<()> {
        debug_assert!(self.elem_size > 0);
        self.file
            .seek(SeekFrom::Start((idx as u64) * (self.elem_size as u64)))?;
        Ok(())
    }

    fn read_elem(&mut self, idx: usize) -> std::io::Result<F> {
        self.seek_elem(idx)?;
        let mut buf = vec![0u8; self.elem_size];
        self.file.read_exact(&mut buf)?;
        let mut rd = &buf[..];
        let v = F::deserialize_compressed(&mut rd).expect("field deserialize");
        Ok(v)
    }

    fn write_elem(&mut self, idx: usize, v: &F) -> std::io::Result<()> {
        self.seek_elem(idx)?;
        let mut buf = Vec::with_capacity(self.elem_size);
        v.serialize_compressed(&mut buf).expect("field serialize");
        debug_assert_eq!(buf.len(), self.elem_size);
        self.file.write_all(&buf)?;
        Ok(())
    }
}

impl Drop for SpillTape {
    fn drop(&mut self) {
        self.remove();
    }
}

// -----------------------------------------------------------------------------
// BlockedIfft façade (stable API) + optional mem logs
// -----------------------------------------------------------------------------

/// Streaming façade that turns a **time-ordered** evaluation stream into
/// coefficient tiles. Feed time blocks as they arrive, then call one of the
/// `finish_*` methods to obtain tiles in the requested order.
///
/// - If `SSZKP_BLOCKED_IFFT=1`, a file-backed in-place GS-INTT is used and
///   peak live memory remains ≈ `O(b_blk)`.
/// - Otherwise, we collect in-memory (back-compat) and still emit tiles.
pub struct BlockedIfft<'d> {
    domain: &'d Domain,
    b_blk: usize,

    // Two modes:
    // - legacy: collect all evals in-memory (Vec<F>) then IFFT
    // - blocked: spill to disk and run GS-INTT in-place on the tape
    legacy_collect: bool,

    // Legacy buffer
    evals: Vec<F>,

    // Spill tape (present only in blocked mode)
    tape: Option<SpillTape>,

    finished: bool,

    // diagnostics
    memlog: bool,
    peak_buffered: usize, // tracks max in-memory buffered evals (≈ b_blk in blocked mode)
}

impl<'d> BlockedIfft<'d> {
    /// Create a new blocked-IFFT façade.
    pub fn new(domain: &'d Domain, b_blk: usize) -> Self {
        assert!(b_blk > 0, "b_blk must be positive");
        let memlog = std::env::var("SSZKP_MEMLOG").ok().as_deref() == Some("1");
        let use_blocked = std::env::var("SSZKP_BLOCKED_IFFT")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let (legacy_collect, tape) = if use_blocked {
            let t = SpillTape::create().expect("create spill tape");
            (false, Some(t))
        } else {
            (true, None)
        };

        Self {
            domain,
            b_blk,
            legacy_collect,
            evals: if legacy_collect { Vec::with_capacity(domain.n) } else { Vec::new() },
            tape,
            finished: false,
            memlog,
            peak_buffered: 0,
        }
    }

    #[inline]
    fn bump_peak(&mut self, cur: usize) {
        if cur > self.peak_buffered {
            self.peak_buffered = cur;
        }
    }

    /// Append a time-slice (block) of evaluations. Blocks must be provided
    /// in **global increasing** index order end-to-end across all calls.
    pub fn feed_eval_block(&mut self, evals: &[F]) {
        assert!(!self.finished, "blocked IFFT already finalized");

        if self.legacy_collect {
            // Legacy in-memory collection (back-compat).
            debug_assert!(
                self.evals.len().saturating_add(evals.len()) <= self.domain.n,
                "BlockedIfft received more than N evaluations"
            );
            self.evals.extend_from_slice(evals);
            self.bump_peak(self.evals.len());
        } else {
            // Blocked path: append directly to the spill tape; keep only a tiny staging buffer.
            let t = self.tape.as_mut().expect("tape");
            t.append_block(evals).expect("tape append");
            // In blocked mode, only the caller's `evals` slice is resident; track its peak.
            self.bump_peak(self.peak_buffered.max(evals.len()).max(self.b_blk).min(self.b_blk));
        }
    }

    /// Number of time evaluations fed so far (T).
    pub fn fed_len(&self) -> usize {
        if self.legacy_collect {
            self.evals.len()
        } else {
            self.tape.as_ref().expect("tape").len
        }
    }

    /// Finalize and emit **low→high** coefficient tiles (≤ `b_blk` each).
    pub fn finish_low_to_high(mut self) -> impl Iterator<Item = Vec<F>> {
        self.finished = true;
        self.finish_common(/*checked=*/false, /*hi_to_lo=*/false)
    }

    /// Finalize and emit **high→low** coefficient tiles (≤ `b_blk` each).
    pub fn finish_high_to_low(mut self) -> impl Iterator<Item = Vec<F>> {
        self.finished = true;
        self.finish_common(/*checked=*/false, /*hi_to_lo=*/true)
    }

    /// **Checked** finisher: errors if more than `N` items were fed.
    pub fn finish_low_to_high_checked(mut self) -> Result<impl Iterator<Item = Vec<F>>, DomainError>
    {
        self.finished = true;
        self.finish_common_checked(false)
    }

    /// **Checked** finisher: errors if more than `N` items were fed.
    pub fn finish_high_to_low_checked(
        mut self,
    ) -> Result<impl Iterator<Item = Vec<F>>, DomainError> {
        self.finished = true;
        self.finish_common_checked(true)
    }

    fn finish_common(
        &mut self,
        checked: bool,
        hi_to_lo: bool,
    ) -> Box<dyn Iterator<Item = Vec<F>>> {
        if let Some(it) = self.try_finish_blocked(checked, hi_to_lo) {
            return it;
        }
        // Legacy fallback:
        let mut coeffs = self.materialize_coefficients(checked).expect("coeff materialization");
        if hi_to_lo {
            coeffs.reverse();
        }
        if self.memlog {
            eprintln!(
                "[memlog] BlockedIfft: N={}, b_blk={}, peak_buffered_evals={}",
                self.domain.n, self.b_blk, self.peak_buffered
            );
        }
        if hi_to_lo {
            Box::new(TileIterHiToLo { coeffs, idx: 0, tile: self.b_blk, memlog: self.memlog, peak_tile: 0 })
        } else {
            Box::new(TileIterLoToHi { coeffs, idx: 0, tile: self.b_blk, memlog: self.memlog, peak_tile: 0 })
        }
    }

    fn finish_common_checked(
        &mut self,
        hi_to_lo: bool,
    ) -> Result<Box<dyn Iterator<Item = Vec<F>>>, DomainError> {
        if let Some(it) = self.try_finish_blocked(true, hi_to_lo) {
            return Ok(it);
        }
        // Legacy path:
        let mut coeffs = self.materialize_coefficients(true)?;
        if hi_to_lo {
            coeffs.reverse();
        }
        if self.memlog {
            eprintln!(
                "[memlog] BlockedIfft: N={}, b_blk={}, peak_buffered_evals={}",
                self.domain.n, self.b_blk, self.peak_buffered
            );
        }
        Ok(if hi_to_lo {
            Box::new(TileIterHiToLo { coeffs, idx: 0, tile: self.b_blk, memlog: self.memlog, peak_tile: 0 })
        } else {
            Box::new(TileIterLoToHi { coeffs, idx: 0, tile: self.b_blk, memlog: self.memlog, peak_tile: 0 })
        })
    }

    /// Blocked path (if enabled): return an iterator over tiles, or `None` if legacy.
    fn try_finish_blocked(
        &mut self,
        checked: bool,
        hi_to_lo: bool,
    ) -> Option<Box<dyn Iterator<Item = Vec<F>>>> {
        if self.legacy_collect {
            return None;
        }
        let n = self.domain.n;
        let t = self.tape.as_mut().expect("tape");

        // bounds / padding / truncation
        let fed = t.len;
        if fed > n {
            if checked {
                // Surface an error by returning a one-shot iterator that panics; callers of
                // checked finishers should use the Result-returning variant above.
                return Some(Box::new(std::iter::once_with(|| {
                    panic!("BadStream error surfaced in checked finisher")
                })));
            } else {
                // truncate the extra by simply ignoring; we won't read past N.
            }
        } else if fed < n {
            // pad zeros to N
            let zeros = vec![F::zero(); n - fed];
            t.append_block(&zeros).expect("tape pad");
        }

        // Run in-place GS-INTT on the tape (decimation-in-frequency).
        self.gs_intt_in_place_on_tape();

        if self.memlog {
            eprintln!(
                "[memlog] BlockedIfft: N={}, b_blk={}, peak_buffered_evals={}",
                self.domain.n, self.b_blk, self.peak_buffered
            );
        }

        // Build a streaming iterator that reads tiles from the tape.
        let it: Box<dyn Iterator<Item = Vec<F>>> = if !hi_to_lo {
            Box::new(TileFromTape::new_forward(
                self.tape.take().unwrap(),
                n,
                self.b_blk,
                self.memlog,
            ))
        } else {
            Box::new(TileFromTape::new_reverse(
                self.tape.take().unwrap(),
                n,
                self.b_blk,
                self.memlog,
            ))
        };
        Some(it)
    }

    /// In-place Gentleman–Sande INTT on the file-backed tape.
    /// After completion, tape holds **coefficients** in low→high order.
    fn gs_intt_in_place_on_tape(&mut self) {
        let n = self.domain.n;
        debug_assert!(n.is_power_of_two());
        let m = n.trailing_zeros() as usize;

        // Stage loop: s = 1..=m, L = 2^s, H = L/2, ω_step = ω^{N/L}
        for s in 1..=m {
            let l = 1usize << s;
            let h = l >> 1;
            let w_step = pow_u64(self.domain.omega, (self.domain.n / l) as u64);

            // j loop (twiddle powers)
            let mut tw_j = F::one();
            for j in 0..h {
                // k loop over butterflies in this column
                let mut k = j;
                while k < n {
                    // read pair
                    let u = self.tape.as_mut().unwrap().read_elem(k).expect("tape read");
                    let v_raw = self
                        .tape
                        .as_mut()
                        .unwrap()
                        .read_elem(k + h)
                        .expect("tape read");
                    let v = v_raw * tw_j;

                    // butterfly
                    let a0 = u + v;
                    let a1 = u - v;

                    // write back
                    self.tape.as_mut().unwrap().write_elem(k, &a0).expect("tape write");
                    self.tape
                        .as_mut()
                        .unwrap()
                        .write_elem(k + h, &a1)
                        .expect("tape write");

                    k += l;
                }
                tw_j *= w_step;
            }
        }

        // Final scaling by 1/N
        let inv_n = F::from(n as u64).inverse().expect("n!=0");
        for i in 0..n {
            let mut ai = self.tape.as_mut().unwrap().read_elem(i).expect("tape read");
            ai *= inv_n;
            self.tape.as_mut().unwrap().write_elem(i, &ai).expect("tape write");
        }
    }

    /// Legacy: ensure exactly N items (pad/truncate), run a single IFFT, and return
    /// **low→high** coefficients.
    fn materialize_coefficients(&mut self, checked: bool) -> Result<Vec<F>, DomainError> {
        if self.evals.len() > self.domain.n {
            if checked {
                return Err(DomainError::BadStream { got: self.evals.len(), n: self.domain.n });
            }
            self.evals.truncate(self.domain.n);
        } else if self.evals.len() < self.domain.n {
            self.evals.resize(self.domain.n, F::zero());
        }
        Ok(ifft_block_evals_to_coeffs(self.domain, &self.evals))
    }
}

// ------------------ Tile iterators (legacy, in-memory) ------------------

struct TileIterLoToHi {
    coeffs: Vec<F>,
    idx: usize,
    tile: usize,
    // mem diagnostics
    memlog: bool,
    peak_tile: usize,
}
impl Iterator for TileIterLoToHi {
    type Item = Vec<F>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.coeffs.len() {
            if self.memlog {
                eprintln!("[memlog] BlockedIfft tiles: peak_tile_len={}", self.peak_tile);
            }
            return None;
        }
        let end = (self.idx + self.tile).min(self.coeffs.len());
        let out = self.coeffs[self.idx..end].to_vec();
        if out.len() > self.peak_tile {
            self.peak_tile = out.len();
        }
        self.idx = end;
        Some(out)
    }
}
struct TileIterHiToLo {
    coeffs: Vec<F>,
    idx: usize,
    tile: usize,
    // mem diagnostics
    memlog: bool,
    peak_tile: usize,
}
impl Iterator for TileIterHiToLo {
    type Item = Vec<F>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.coeffs.len() {
            if self.memlog {
                eprintln!("[memlog] BlockedIfft tiles: peak_tile_len={}", self.peak_tile);
            }
            return None;
        }
        let end = (self.idx + self.tile).min(self.coeffs.len());
        let out = self.coeffs[self.idx..end].to_vec();
        if out.len() > self.peak_tile {
            self.peak_tile = out.len();
        }
        self.idx = end;
        Some(out)
    }
}

// ------------------ Tile iterators (blocked, from tape) ------------------

struct TileFromTape {
    tape: SpillTape,
    next_idx: isize,
    end_idx_exclusive: isize,
    step: isize, // +1 forward, -1 reverse
    n: usize,
    tile: usize,
    memlog: bool,
    peak_tile: usize,
    done: bool,
}

impl TileFromTape {
    fn new_forward(mut tape: SpillTape, n: usize, tile: usize, memlog: bool) -> Self {
        // set file cursor somewhere neutral; reads always seek.
        tape.file.seek(SeekFrom::Start(0)).ok();
        Self {
            tape,
            next_idx: 0,
            end_idx_exclusive: n as isize,
            step: 1,
            n,
            tile,
            memlog,
            peak_tile: 0,
            done: false,
        }
    }
    fn new_reverse(mut tape: SpillTape, n: usize, tile: usize, memlog: bool) -> Self {
        tape.file.seek(SeekFrom::Start(0)).ok();
        Self {
            tape,
            next_idx: (n as isize) - 1,
            end_idx_exclusive: -1, // stop when <0
            step: -1,
            n,
            tile,
            memlog,
            peak_tile: 0,
            done: false,
        }
    }
}

impl Iterator for TileFromTape {
    type Item = Vec<F>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.step > 0 {
            if self.next_idx >= self.end_idx_exclusive {
                if self.memlog {
                    eprintln!("[memlog] BlockedIfft tiles: peak_tile_len={}", self.peak_tile);
                }
                self.done = true;
                return None;
            }
            let start = self.next_idx as usize;
            let end = ((start + self.tile).min(self.n)) as usize;
            let mut out = Vec::with_capacity(end - start);
            for i in start..end {
                let v = self.tape.read_elem(i).expect("tape read");
                out.push(v);
            }
            self.next_idx = end as isize;
            if out.len() > self.peak_tile {
                self.peak_tile = out.len();
            }
            Some(out)
        } else {
            if self.next_idx <= self.end_idx_exclusive {
                if self.memlog {
                    eprintln!("[memlog] BlockedIfft tiles: peak_tile_len={}", self.peak_tile);
                }
                self.done = true;
                return None;
            }
            let end_inclusive = self.next_idx as usize;
            let start_inclusive = end_inclusive.saturating_sub(self.tile - 1);
            let mut out = Vec::with_capacity(end_inclusive - start_inclusive + 1);
            for i in (start_inclusive..=end_inclusive).rev() {
                let v = self.tape.read_elem(i).expect("tape read");
                out.push(v);
            }
            // Move prev
            if start_inclusive == 0 {
                self.next_idx = -1;
            } else {
                self.next_idx = (start_inclusive - 1) as isize;
            }
            if out.len() > self.peak_tile {
                self.peak_tile = out.len();
            }
            Some(out)
        }
    }
}

// -----------------------------------------------------------------------------
// Streaming helpers — preserved public API used throughout the repo
// -----------------------------------------------------------------------------

/// Emit **low→high** coefficient tiles (each length ≤ `b_blk`) from a time-ordered
/// stream of evaluations. Never materializes the full polynomial.
///
/// Internally uses `BlockedIfft`. The iterator is *single-use* and consumes
/// the underlying staging buffers as it yields.
pub fn ifft_time_stream_to_coeff_tiles<'a>(
    domain: &Domain,
    b_blk: usize,
    evals: impl Iterator<Item = F> + 'a,
) -> impl Iterator<Item = Vec<F>> + 'a {
    // Feed in blocks of size ≤ b_blk.
    let mut bifft = BlockedIfft::new(domain, b_blk);
    let mut buf: Vec<F> = Vec::with_capacity(b_blk);
    let mut iter = evals;
    while let Some(x) = iter.next() {
        buf.push(x);
        if buf.len() == b_blk {
            bifft.feed_eval_block(&buf);
            buf.clear();
        }
    }
    if !buf.is_empty() {
        bifft.feed_eval_block(&buf);
    }
    bifft.finish_low_to_high()
}

/// Emit **high→low** coefficient tiles (each length ≤ `b_blk`) from a time-ordered
/// stream of evaluations. Never materializes the full polynomial.
///
/// Internally uses `BlockedIfft`. The iterator is *single-use* and consumes
/// the underlying staging buffers as it yields.
pub fn ifft_time_stream_to_coeff_tiles_hi_to_lo<'a>(
    domain: &Domain,
    b_blk: usize,
    evals: impl Iterator<Item = F> + 'a,
) -> impl Iterator<Item = Vec<F>> + 'a {
    let mut bifft = BlockedIfft::new(domain, b_blk);
    let mut buf: Vec<F> = Vec::with_capacity(b_blk);
    let mut iter = evals;
    while let Some(x) = iter.next() {
        buf.push(x);
        if buf.len() == b_blk {
            bifft.feed_eval_block(&buf);
            buf.clear();
        }
    }
    if !buf.is_empty() {
        bifft.feed_eval_block(&buf);
    }
    bifft.finish_high_to_low()
}
