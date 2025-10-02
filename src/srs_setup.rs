//! Structured Reference String (SRS) Setup and Management
//!
//! # What is an SRS?
//!
//! A **Structured Reference String (SRS)** is the public parameter set required for
//! KZG polynomial commitments. It consists of:
//!
//! - **G1 powers**: `[τ⁰·G₁, τ¹·G₁, τ²·G₁, ..., τᵈ·G₁]` where τ is a secret scalar
//! - **G2 elements**: At minimum `[τ·G₂]` for verification (some ceremonies include `[G₂, τ·G₂]`)
//!
//! The security of KZG commitments relies on τ being **unknown and destroyed** after
//! the trusted setup ceremony. If an attacker learns τ, they can forge proofs.
//!
//! # Trusted Setup Ceremonies
//!
//! Production systems MUST use SRS from a **multi-party computation (MPC)** ceremony
//! where multiple independent participants contribute randomness. As long as ONE
//! participant honestly destroys their contribution, the final τ remains secret.
//!
//! ## Recommended Sources for BN254
//!
//! 1. **Perpetual Powers of Tau** (Ethereum Foundation)
//!    - URL: <https://github.com/privacy-scaling-explorations/perpetualpowersoftau>
//!    - Format: `.ptau` files (needs conversion)
//!    - Security: High (100+ participants)
//!
//! 2. **Hermez/Polygon Ceremony**
//!    - URL: `https://hermez.s3-eu-west-1.amazonaws.com/powersOfTau28_hez_final_NN.ptau`
//!    - Format: Direct download
//!    - Security: High (trusted by Polygon zkEVM)
//!
//! 3. **Aztec Ignition Ceremony**
//!    - URL: <https://aztec-ignition.s3.amazonaws.com/>
//!    - Format: `.dat` files
//!    - Security: High (176 participants)
//!
//! # Security Model
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  SECURITY ASSUMPTION: τ is unknown and destroyed            │
//! │                                                             │
//! │  IF attacker knows τ:                                       │
//! │    → Can forge commitments for any polynomial               │
//! │    → Can create false proofs that verify                    │
//! │    → COMPLETE BREAK of the ZKP system                       │
//! │                                                             │
//! │  MITIGATION: Use MPC ceremony with ≥1 honest participant    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # File Format
//!
//! This module expects Arkworks-serialized (compressed) affine points:
//!
//! **G1 SRS (`G1.bin`)**:
//! ```text
//! [G1Affine; degree+1]  // Compressed arkworks serialization
//! ```
//!
//! **G2 SRS (`G2.bin`)**:
//! ```text
//! [G2Affine; 1 or 2]    // Either [τ·G₂] or [G₂, τ·G₂]
//! ```
//!
//! # Usage
//!
//! ## Production (with trusted SRS)
//!
//! ```no_run
//! use myzkp::srs_setup::{load_and_validate_g1_srs, load_and_validate_g2_srs};
//! use myzkp::pcs;
//!
//! // Load from files (obtained from a trusted ceremony)
//! let g1_powers = load_and_validate_g1_srs("G1.bin", 16384)?;
//! let tau_g2 = load_and_validate_g2_srs("G2.bin")?;
//!
//! // Install into global PCS state
//! pcs::load_srs_g1(&g1_powers);
//! pcs::load_srs_g2(tau_g2);
//!
//! // Verify digests for audit trail
//! let g1_digest = pcs::srs_g1_digest();
//! let g2_digest = pcs::srs_g2_digest();
//! println!("SRS loaded. G1 digest: {:?}", g1_digest);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Development (deterministic SRS, NOT SECURE)
//!
//! ```no_run
//! # #[cfg(feature = "dev-srs")]
//! # {
//! use myzkp::srs_setup::generate_dev_srs;
//! use myzkp::pcs;
//!
//! // ⚠️  WARNING: For testing only! Deterministic τ is PUBLICLY KNOWN
//! let (g1_powers, tau_g2) = generate_dev_srs(1024);
//! pcs::load_srs_g1(&g1_powers);
//! pcs::load_srs_g2(tau_g2);
//! # }
//! ```
//!
//! # Validation Layers
//!
//! This module performs **four layers** of validation:
//!
//! 1. **Format validation**: Can deserialize without corruption
//! 2. **Structural validation**: First element is generator, sufficient degree
//! 3. **Cryptographic validation**: Optional pairing checks (expensive)
//! 4. **Digest verification**: Compare against known-good ceremony outputs
//!
//! # Performance Notes
//!
//! - **Loading**: O(degree) deserialization, typically <1s for degree=16384
//! - **Validation**: Optional pairing checks add ~0.1s per check
//! - **Memory**: Loaded SRS persists in global state (≈1MB per 16384 powers)

#![forbid(unsafe_code)]
#![allow(unused_imports)]

use ark_bn254::{Bn254, Fr, G1Affine, G2Affine};
use ark_ec::{pairing::Pairing, AffineRepr, CurveGroup, Group};
use ark_ff::{Field, One, PrimeField, UniformRand, Zero};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use std::path::Path;

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during SRS setup and validation.
#[derive(Debug, thiserror::Error)]
pub enum SrsSetupError {
    /// File I/O error (file not found, permissions, etc.)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to deserialize SRS from binary format
    #[error("deserialization error: {0}")]
    Deserialize(String),

    /// SRS failed cryptographic or structural validation
    #[error("SRS validation failed: {0}")]
    Validation(String),

    /// Network error during SRS download (feature-gated)
    #[error("download error: {0}")]
    Download(String),

    /// Pairing check failed (indicates corrupted or malicious SRS)
    #[error("pairing check failed: {0}")]
    PairingCheck(String),
}

// ============================================================================
// G1 SRS Loading and Validation
// ============================================================================

/// Load G1 SRS from a binary file and perform comprehensive validation.
///
/// # Format
///
/// The file must contain an Arkworks-serialized `Vec<G1Affine>` in compressed format.
///
/// # Validation Performed
///
/// 1. **Deserialization**: Ensures binary data is well-formed
/// 2. **Degree check**: Verifies `powers.len() >= expected_degree + 1`
/// 3. **Generator check**: Confirms `powers[0]` equals the BN254 G1 generator
/// 4. **Point validity**: All points are on-curve (automatic via Arkworks)
///
/// # Security Note
///
/// This function does NOT verify the SRS came from a trusted ceremony. Callers
/// should compare the resulting digest against known-good values:
///
/// ```no_run
/// use myzkp::{srs_setup, pcs};
///
/// let powers = srs_setup::load_and_validate_g1_srs("G1.bin", 16384)?;
/// pcs::load_srs_g1(&powers);
///
/// let digest = pcs::srs_g1_digest();
/// const EXPECTED: [u8; 32] = /* from ceremony transcript */;
/// assert_eq!(digest, EXPECTED, "SRS digest mismatch - possible corruption or wrong file");
/// # Ok::<(), srs_setup::SrsSetupError>(())
/// ```
///
/// # Errors
///
/// - [`SrsSetupError::Io`] if file cannot be read
/// - [`SrsSetupError::Deserialize`] if binary format is invalid
/// - [`SrsSetupError::Validation`] if structural checks fail
pub fn load_and_validate_g1_srs(
    path: impl AsRef<Path>,
    expected_degree: usize,
) -> Result<Vec<G1Affine>, SrsSetupError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let mut slice = bytes.as_slice();

    // Layer 1: Deserialization
    let powers: Vec<G1Affine> = CanonicalDeserialize::deserialize_compressed(&mut slice)
        .map_err(|e| SrsSetupError::Deserialize(format!("G1 SRS: {}", e)))?;

    // Layer 2: Degree validation
    if powers.len() < expected_degree + 1 {
        return Err(SrsSetupError::Validation(format!(
            "G1 SRS has {} powers, need at least {} for degree {} (file may be for smaller circuit)",
            powers.len(),
            expected_degree + 1,
            expected_degree
        )));
    }

    // Layer 3: Generator validation
    // The first element MUST be [1]G₁ = G₁ (the identity scalar times the generator)
    let g1_gen = <Bn254 as Pairing>::G1::generator();
    if powers[0] != g1_gen {
        return Err(SrsSetupError::Validation(
            "G1 SRS first element is not the generator (possible corruption or wrong curve)".into(),
        ));
    }

    // Layer 4: Point validity is automatic via Arkworks deserialization
    // (Invalid points cause deserialization to fail)

    Ok(powers)
}

/// Perform an optional cryptographic pairing check on G1 SRS.
///
/// **WARNING**: This is computationally expensive (~100ms for two pairings).
/// Use sparingly, typically only once at initialization or in tests.
///
/// # What This Checks
///
/// Verifies the relationship: `e([τ]G₁, G₂) = e(G₁, [τ]G₂)`
///
/// If this holds, the SRS is **algebraically consistent** (though not necessarily
/// from a trusted ceremony—compare digests for that).
///
/// # Errors
///
/// - [`SrsSetupError::PairingCheck`] if the pairing equation does not hold
pub fn validate_g1_pairing(
    g1_powers: &[G1Affine],
    tau_g2: G2Affine,
) -> Result<(), SrsSetupError> {
    if g1_powers.len() < 2 {
        return Err(SrsSetupError::Validation(
            "Need at least 2 G1 powers for pairing check".into(),
        ));
    }

    let g1_gen = <Bn254 as Pairing>::G1::generator();
    let g2_gen = <Bn254 as Pairing>::G2::generator();

    // Check: e(τ·G₁, G₂) =?= e(G₁, τ·G₂)
    let lhs = Bn254::pairing(g1_powers[1], g2_gen);
    let rhs = Bn254::pairing(g1_gen, tau_g2);

    if lhs != rhs {
        return Err(SrsSetupError::PairingCheck(
            "G1 powers do not satisfy pairing equation e(τG₁, G₂) = e(G₁, τG₂)".into(),
        ));
    }

    Ok(())
}

// ============================================================================
// G2 SRS Loading and Validation
// ============================================================================

/// Load G2 SRS from a binary file and extract `[τ]G₂`.
///
/// # Format
///
/// The file must contain an Arkworks-serialized `Vec<G2Affine>` with either:
///
/// - **Two elements**: `[G₂, τ·G₂]` (full ceremony output)
/// - **One element**: `[τ·G₂]` (minimal verifier key)
///
/// This function returns `τ·G₂` in both cases.
///
/// # Validation Performed
///
/// 1. **Deserialization**: Binary format is well-formed
/// 2. **Length check**: At least one element present
/// 3. **Generator check** (if two elements): First is G₂ generator
/// 4. **Non-identity check**: `τ·G₂` is not the point at infinity
///
/// # Errors
///
/// - [`SrsSetupError::Io`] if file cannot be read
/// - [`SrsSetupError::Deserialize`] if binary format is invalid
/// - [`SrsSetupError::Validation`] if structural checks fail
pub fn load_and_validate_g2_srs(path: impl AsRef<Path>) -> Result<G2Affine, SrsSetupError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let mut slice = bytes.as_slice();

    // Layer 1: Deserialization
    let elements: Vec<G2Affine> = CanonicalDeserialize::deserialize_compressed(&mut slice)
        .map_err(|e| SrsSetupError::Deserialize(format!("G2 SRS: {}", e)))?;

    // Layer 2: Length validation
    if elements.is_empty() {
        return Err(SrsSetupError::Validation(
            "G2 SRS file is empty (need at least [τ·G₂])".into(),
        ));
    }

    // Layer 3: Extract τ·G₂
    let tau_g2 = if elements.len() >= 2 {
        // Format: [G₂, τ·G₂]
        // Verify first element is generator
        let g2_gen = <Bn254 as Pairing>::G2::generator();
        if elements[0] != g2_gen {
            return Err(SrsSetupError::Validation(
                "G2 SRS first element is not the generator (expected [G₂, τ·G₂] format)".into(),
            ));
        }
        elements[1]
    } else {
        // Format: [τ·G₂]
        elements[0]
    };

    // Layer 4: Non-identity check
    // τ·G₂ should never be the point at infinity (would mean τ=0, breaking security)
    if tau_g2.is_zero() {
        return Err(SrsSetupError::Validation(
            "τ·G₂ is the point at infinity (invalid SRS)".into(),
        ));
    }

    Ok(tau_g2)
}

// ============================================================================
// Development SRS Generation (NOT FOR PRODUCTION)
// ============================================================================

/// Generate a deterministic development SRS with a **publicly known** secret.
///
/// # ⚠️  SECURITY WARNING
///
/// This function uses a **fixed seed** (42), making τ completely predictable.
/// Anyone can compute the same τ and forge proofs. **NEVER** use this in production.
///
/// # Appropriate Use Cases
///
/// - **Local development**: Testing proof generation without downloading large files
/// - **CI/CD**: Deterministic tests that don't require network access
/// - **Tutorials**: Educational examples where security is explicitly disclaimed
///
/// # Inappropriate Use Cases
///
/// - Production deployments (even internal tools)
/// - User-facing applications
/// - Any scenario where proof integrity matters
///
/// # Implementation Note
///
/// The secret τ is generated from `StdRng::from_seed([42; 32])`, making it
/// identical across all invocations. This is intentional for reproducibility
/// but catastrophic for security.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "dev-srs")]
/// # {
/// use myzkp::srs_setup::generate_dev_srs;
///
/// // Generate SRS for degree-1024 polynomials
/// let (g1_powers, tau_g2) = generate_dev_srs(1024);
///
/// assert_eq!(g1_powers.len(), 1025); // degree+1 powers
/// // τ is deterministic: all runs produce identical output
/// # }
/// ```
#[cfg(feature = "dev-srs")]
pub fn generate_dev_srs(degree: usize) -> (Vec<G1Affine>, G2Affine) {
    use rand::{rngs::StdRng, SeedableRng};

    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!("⚠️  WARNING: Generating DEVELOPMENT SRS (seed=42, τ is PUBLIC)");
    eprintln!("⚠️  This SRS is NOT SECURE and must NEVER be used in production!");
    eprintln!("⚠️  Anyone can forge proofs with this setup.");
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Fixed seed for reproducibility (THE SECURITY FLAW)
    let mut rng = StdRng::from_seed([42u8; 32]);
    let tau: Fr = Fr::rand(&mut rng);

    eprintln!("Generating SRS: degree={}, tau=<deterministic>", degree);

    let g1_gen = <Bn254 as Pairing>::G1::generator();
    let g2_gen = <Bn254 as Pairing>::G2::generator();

    // Compute [τ⁰·G₁, τ¹·G₁, ..., τᵈ·G₁]
    let mut g1_powers = Vec::with_capacity(degree + 1);
    let mut tau_pow = Fr::one();

    for i in 0..=degree {
        let point = (g1_gen * tau_pow).into_affine(); 
        g1_powers.push(point);
        tau_pow *= tau;

        if i > 0 && i % 1024 == 0 {
            eprintln!("  Computed {}/{} G1 powers...", i, degree + 1);
        }
    }

    // Compute [τ·G₂]
    let tau_g2 = (g2_gen * tau).into_affine();

    eprintln!("✓ Dev SRS generated successfully");
    eprintln!("  G1 powers: {}", g1_powers.len());
    eprintln!("  G2 element: τ·G₂");

    (g1_powers, tau_g2)
}

// ============================================================================
// SRS File I/O Helpers
// ============================================================================

/// Save G1 SRS to a binary file in compressed Arkworks format.
///
/// Useful for caching ceremony outputs or persisting dev SRS.
pub fn save_g1_srs(path: impl AsRef<Path>, powers: &[G1Affine]) -> Result<(), SrsSetupError> {
    let mut bytes = Vec::new();
    powers
        .serialize_compressed(&mut bytes)
        .map_err(|e| SrsSetupError::Validation(format!("G1 serialize: {}", e)))?;

    std::fs::write(path.as_ref(), bytes)?;
    Ok(())
}

/// Save G2 SRS to a binary file in compressed Arkworks format.
///
/// Saves in `[G₂, τ·G₂]` format for maximum compatibility.
pub fn save_g2_srs(path: impl AsRef<Path>, tau_g2: G2Affine) -> Result<(), SrsSetupError> {
    let g2_gen = <Bn254 as Pairing>::G2::generator();
    let elements = vec![g2_gen, tau_g2.into()];

    let mut bytes = Vec::new();
    elements
        .serialize_compressed(&mut bytes)
        .map_err(|e| SrsSetupError::Validation(format!("G2 serialize: {}", e)))?;

    std::fs::write(path.as_ref(), bytes)?;
    Ok(())
}

// ============================================================================
// Download Helpers (Future: Automatic Ceremony Downloads)
// ============================================================================

/// Download SRS from Perpetual Powers of Tau ceremony.
///
/// **NOTE**: This function is currently a stub. Implement when async HTTP
/// client is available in your environment.
///
/// # Intended Behavior
///
/// 1. Determine required `.ptau` file from `max_degree`
/// 2. Download from Hermez/Aztec/Ethereum S3 bucket
/// 3. Parse `.ptau` format (ceremony-specific)
/// 4. Convert to Arkworks binary format
/// 5. Save to `output_dir/{G1,G2}.bin`
///
/// # Security
///
/// - Verify SHA256 checksums from ceremony transcript
/// - Compare output digests against published values
/// - Fail loudly on any mismatch
#[allow(unused_variables)]
pub fn download_perpetual_powers(
    max_degree: usize,
    output_dir: impl AsRef<Path>,
) -> Result<(), SrsSetupError> {
    let degree_log2 = (max_degree as f64).log2().ceil() as usize;

    // Hermez ceremony URL pattern (example)
    let url_base = "https://hermez.s3-eu-west-1.amazonaws.com/powersOfTau28_hez_final";
    let ptau_file = format!("{}_{}.ptau", url_base, degree_log2);

    Err(SrsSetupError::Download(format!(
        "Auto-download not yet implemented. Manual download instructions:\n\
         \n\
         1. Download the .ptau file for your degree:\n\
            {}\n\
         \n\
         2. Convert to Arkworks format using `snarkjs` or our conversion script:\n\
            scripts/convert_ptau.py {} {}/G1.bin {}/G2.bin\n\
         \n\
         3. Verify digests match ceremony transcript:\n\
            sha256sum {}/G1.bin {}/G2.bin\n\
         \n\
         See https://github.com/privacy-scaling-explorations/perpetualpowersoftau\n\
         for ceremony details and verification procedures.",
        ptau_file,
        ptau_file,
        output_dir.as_ref().display(),
        output_dir.as_ref().display(),
        output_dir.as_ref().display(),
        output_dir.as_ref().display(),
    )))
}

// ============================================================================
// Digest Verification Helpers
// ============================================================================

/// Known-good SRS digests from trusted ceremonies.
///
/// Use these to verify loaded SRS matches a specific ceremony. Add entries
/// as you verify new ceremony outputs.
#[derive(Debug, Clone)]
pub struct CeremonyDigests {
    /// Human-readable ceremony name
    pub name: &'static str,
    /// Maximum degree supported
    pub max_degree: usize,
    /// Expected G1 digest (from `pcs::srs_g1_digest()`)
    pub g1_digest: [u8; 32],
    /// Expected G2 digest (from `pcs::srs_g2_digest()`)
    pub g2_digest: [u8; 32],
}

/// Registry of known ceremony digests (extend as needed).
pub const KNOWN_CEREMONIES: &[CeremonyDigests] = &[
    // Add digests here after verifying ceremony outputs
    // Example:
    // CeremonyDigests {
    //     name: "Hermez Powers of Tau 28 (degree 2^20)",
    //     max_degree: 1_048_576,
    //     g1_digest: [0xAB, 0xCD, ...],
    //     g2_digest: [0x12, 0x34, ...],
    // },
];

/// Verify loaded SRS matches a known ceremony.
///
/// Compares the digests from `pcs::srs_g1_digest()` and `pcs::srs_g2_digest()`
/// against the provided expected values.
///
/// # Example
///
/// ```no_run
/// use myzkp::{srs_setup, pcs};
///
/// // Load SRS
/// let g1 = srs_setup::load_and_validate_g1_srs("G1.bin", 16384)?;
/// let g2 = srs_setup::load_and_validate_g2_srs("G2.bin")?;
/// pcs::load_srs_g1(&g1);
/// pcs::load_srs_g2(g2);
///
/// // Verify against known ceremony
/// let expected_g1 = [0xAB; 32]; // from ceremony docs
/// let expected_g2 = [0xCD; 32];
/// srs_setup::verify_ceremony_digests(expected_g1, expected_g2)?;
/// # Ok::<(), srs_setup::SrsSetupError>(())
/// ```
pub fn verify_ceremony_digests(
    expected_g1: [u8; 32],
    expected_g2: [u8; 32],
) -> Result<(), SrsSetupError> {
    let actual_g1 = crate::pcs::srs_g1_digest();
    let actual_g2 = crate::pcs::srs_g2_digest();

    if actual_g1 != expected_g1 {
        return Err(SrsSetupError::Validation(format!(
            "G1 digest mismatch:\n  expected: {:02x?}\n  actual:   {:02x?}\n\
             This indicates wrong SRS file or corruption.",
            expected_g1, actual_g1
        )));
    }

    if actual_g2 != expected_g2 {
        return Err(SrsSetupError::Validation(format!(
            "G2 digest mismatch:\n  expected: {:02x?}\n  actual:   {:02x?}\n\
             This indicates wrong SRS file or corruption.",
            expected_g2, actual_g2
        )));
    }

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "dev-srs")]
    fn dev_srs_generation_produces_valid_structure() {
        let degree = 128;
        let (g1_powers, tau_g2) = generate_dev_srs(degree);

        // Structural checks
        assert_eq!(g1_powers.len(), degree + 1);
        assert_eq!(g1_powers[0], <Bn254 as Pairing>::G1::generator());
        assert!(!tau_g2.is_zero());
    }

    #[test]
    #[cfg(feature = "dev-srs")]
    fn dev_srs_is_deterministic() {
        let (g1_a, g2_a) = generate_dev_srs(16);
        let (g1_b, g2_b) = generate_dev_srs(16);

        // Same seed → identical output
        assert_eq!(g1_a, g1_b);
        assert_eq!(g2_a, g2_b);
    }

    #[test]
    #[cfg(feature = "dev-srs")]
    fn dev_srs_satisfies_pairing_check() {
        let (g1_powers, tau_g2) = generate_dev_srs(64);
        validate_g1_pairing(&g1_powers, tau_g2).expect("pairing check failed");
    }

    #[test]
    #[cfg(feature = "dev-srs")]
    fn roundtrip_file_io() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let g1_path = dir.path().join("test_G1.bin");
        let g2_path = dir.path().join("test_G2.bin");

        // Generate
        let (g1_orig, g2_orig) = generate_dev_srs(32);

        // Save
        save_g1_srs(&g1_path, &g1_orig).unwrap();
        save_g2_srs(&g2_path, g2_orig).unwrap();

        // Load
        let g1_loaded = load_and_validate_g1_srs(&g1_path, 32).unwrap();
        let g2_loaded = load_and_validate_g2_srs(&g2_path).unwrap();

        // Verify
        assert_eq!(g1_orig, g1_loaded);
        assert_eq!(g2_orig, g2_loaded);
    }

    #[test]
    #[cfg(feature = "dev-srs")]
    fn rejects_insufficient_degree() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let path = dir.path().join("small.bin");

        let (g1_powers, _) = generate_dev_srs(16);
        save_g1_srs(&path, &g1_powers).unwrap();

        // Try to load for higher degree
        let result = load_and_validate_g1_srs(&path, 32);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("need at least"));
    }

    #[test]
    fn rejects_empty_g2_file() {
        use tempfile::NamedTempFile;

        let file = NamedTempFile::new().unwrap();
        let empty: Vec<G2Affine> = vec![];
        let mut bytes = Vec::new();
        empty.serialize_compressed(&mut bytes).unwrap();
        std::fs::write(file.path(), bytes).unwrap();

        let result = load_and_validate_g2_srs(file.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }
}