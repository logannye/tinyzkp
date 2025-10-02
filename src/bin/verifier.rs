//! Minimal CLI verifier (v2 format)
//!
//! Reads a strict, versioned proof file:
//!   magic: b"SSZKPv2\0" (8 bytes) + u16 version (=2) + ark-compressed `Proof`
//!
//! Updates in this revision (format unchanged):
//! - **SRS digest enforcement**: compare loaded SRS (G1/G2) digests against the
//!   proof header and error clearly on mismatch.
//! - **Production SRS validation**: uses `srs_setup` module for comprehensive validation.
//! - **Header authority**: the verifier *trusts the proof header* for domain
//!   parameters. Any `--zh-c` CLI flag is politely ignored (we print a note).
//! - **Basis override policy**: the **header's wire basis** is used. If the CLI
//!   provided `--basis`, we warn on divergence and proceed with the header basis.
//! - **Feature-aware shape checks**: expected openings are computed in a way that
//!   matches feature flags (e.g., `zeta-shift` adds Z@ω·ζ).
//! - Delegation to `scheduler::Verifier` is unchanged; this wrapper only handles
//!   IO, basic shape sanity, and environment/header consistency.

#![forbid(unsafe_code)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::{env, fs, io::Read, path::Path};

use ark_ff::{fields::Field, FftField, One, Zero};
use ark_serialize::CanonicalDeserialize;
use myzkp::{
    domain::{self, domain_digest},
    pcs::{self, Basis, PcsParams},
    scheduler::Verifier,
    VerifyParams, F,
};

// 8-byte magic: "SSZKPv2" + NUL terminator to match the 8-byte read/write.
const FILE_MAGIC: &[u8; 8] = b"SSZKPv2\0";
const FILE_VERSION_SUPPORTED: u16 = 2;

fn parse_flag(args: &[String], key: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == key {
            return it.next().cloned();
        }
    }
    None
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    // Optional CLI hint for wires basis — for UX only. The header is authoritative.
    let basis_str = parse_flag(&args, "--basis").unwrap_or_else(|| "eval".to_string());
    let basis_wires_cli = match basis_str.as_str() {
        "coeff" | "coefficient" => Basis::Coefficient,
        _ => Basis::Evaluation,
    };

    // Users may pass --zh-c out of habit; make it explicit we ignore it.
    if let Some(cli_zh) = parse_flag(&args, "--zh-c") {
        eprintln!("Note: Ignoring CLI --zh-c={}; verifier uses zh_c from the proof header.", cli_zh);
    }

    // ============================================================================
    // SRS loading with comprehensive validation
    // ============================================================================
    
    let srs_g1_path = parse_flag(&args, "--srs-g1");
    let srs_g2_path = parse_flag(&args, "--srs-g2");

    #[cfg(feature = "dev-srs")]
    {
        if srs_g1_path.is_none() || srs_g2_path.is_none() {
            eprintln!("(dev-srs) Using deterministic in-crate SRS.");
            eprintln!("⚠️  WARNING: Dev SRS is NOT SECURE - for testing only!");
            eprintln!("    For production, pass --srs-g1 and --srs-g2 with trusted ceremony files.");
        }
    }

    #[cfg(not(feature = "dev-srs"))]
    {
        if srs_g1_path.is_none() || srs_g2_path.is_none() {
            return Err(anyhow::anyhow!(
                "Non-dev build: --srs-g1 and --srs-g2 are REQUIRED for trusted KZG verification.\n\
                 \n\
                 For development, rebuild with --features dev-srs.\n\
                 For production, provide SRS files from a trusted ceremony:\n\
                 https://github.com/privacy-scaling-explorations/perpetualpowersoftau"
            ));
        }
    }

    // Load and validate SRS files (if provided)
    if let Some(g1_path_str) = srs_g1_path {
        let g1_path = Path::new(&g1_path_str);
        eprintln!("Loading G1 SRS from {}...", g1_path.display());

        // Note: We don't know the required degree yet (it's in the proof header),
        // so we just load and do basic validation. Degree check happens after
        // reading the proof.
        let g1_powers = myzkp::srs_setup::load_and_validate_g1_srs(g1_path, 0)
            .map_err(|e| anyhow::anyhow!("Failed to load/validate G1 SRS: {}", e))?;

        pcs::load_srs_g1(&g1_powers);
        eprintln!("✓ Loaded and validated {} G1 powers", g1_powers.len());
    }

    if let Some(g2_path_str) = srs_g2_path {
        let g2_path = Path::new(&g2_path_str);
        eprintln!("Loading G2 SRS from {}...", g2_path.display());

        let tau_g2 = myzkp::srs_setup::load_and_validate_g2_srs(g2_path)
            .map_err(|e| anyhow::anyhow!("Failed to load/validate G2 SRS: {}", e))?;

        pcs::load_srs_g2(tau_g2);
        eprintln!("✓ Loaded and validated G2 element ([τ]G₂)");
    }

    // ============================================================================
    // Read and parse proof file
    // ============================================================================

    eprintln!();
    eprintln!("Reading proof from proof.bin...");
    
    let mut file = fs::File::open("proof.bin").map_err(|e| anyhow::anyhow!("open proof.bin: {e}"))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != FILE_MAGIC {
        return Err(anyhow::anyhow!("bad proof file: missing magic header"));
    }
    let mut ver_bytes = [0u8; 2];
    file.read_exact(&mut ver_bytes)?;
    let file_ver = u16::from_be_bytes(ver_bytes);
    if file_ver != FILE_VERSION_SUPPORTED {
        return Err(anyhow::anyhow!(
            "unsupported proof version: got {}, support {}",
            file_ver, FILE_VERSION_SUPPORTED
        ));
    }
    let mut payload = Vec::new();
    file.read_to_end(&mut payload)?;
    let mut slice = payload.as_slice();
    let proof: myzkp::Proof = CanonicalDeserialize::deserialize_compressed(&mut slice)
        .map_err(|e| anyhow::anyhow!("deserialize proof: {}", e))?;

    eprintln!("✓ Proof file parsed successfully (v{}, {} bytes)", file_ver, payload.len());

    // ============================================================================
    // Verify SRS digests match proof header
    // ============================================================================

    eprintln!();
    eprintln!("Verifying cryptographic parameters...");

    // SRS digests are the *only* binding between proof and locally loaded SRS.
    let srs_g1_d = pcs::srs_g1_digest();
    let srs_g2_d = pcs::srs_g2_digest();
    
    if proof.header.srs_g1_digest != srs_g1_d {
        eprintln!("ERROR: SRS G1 digest mismatch!");
        eprintln!("  Proof expects:  {:02x?}", proof.header.srs_g1_digest);
        eprintln!("  Loaded SRS has: {:02x?}", srs_g1_d);
        eprintln!();
        eprintln!("This means the proof was generated with a different G1 SRS.");
        eprintln!("You must use the EXACT SAME SRS files that were used to generate the proof.");
        return Err(anyhow::anyhow!("SRS G1 digest mismatch vs proof header"));
    }
    
    if proof.header.srs_g2_digest != srs_g2_d {
        eprintln!("ERROR: SRS G2 digest mismatch!");
        eprintln!("  Proof expects:  {:02x?}", proof.header.srs_g2_digest);
        eprintln!("  Loaded SRS has: {:02x?}", srs_g2_d);
        eprintln!();
        eprintln!("This means the proof was generated with a different G2 SRS.");
        eprintln!("You must use the EXACT SAME SRS files that were used to generate the proof.");
        return Err(anyhow::anyhow!("SRS G2 digest mismatch vs proof header"));
    }

    eprintln!("✓ SRS digests match proof header");

    // Domain from the header (authoritative). We do not accept CLI overrides.
    let domain = myzkp::domain::Domain {
        n: proof.header.domain_n as usize,
        omega: proof.header.domain_omega,
        zh_c: proof.header.zh_c,
    };
    let dom_digest = domain_digest(&domain);

    // PCS params: prefer the **header basis** for wires; warn on CLI divergence.
    let basis_wires = proof.header.basis_wires;
    if basis_wires != basis_wires_cli {
        eprintln!(
            "Note: Ignoring CLI --basis={:?}; using header basis={:?}",
            basis_wires_cli, basis_wires
        );
    }
    let pcs_wires = PcsParams { max_degree: domain.n - 1, basis: basis_wires, srs_placeholder: () };
    let pcs_coeff = PcsParams { max_degree: domain.n - 1, basis: Basis::Coefficient, srs_placeholder: () };

    // Human-friendly summary for quick inspection.
    eprintln!();
    eprintln!("Proof parameters:");
    eprintln!("  Domain size (N): {}", proof.header.domain_n);
    eprintln!("  Registers (k):   {}", proof.header.k);
    eprintln!("  Vanishing (zh_c): {}", proof.header.zh_c);
    eprintln!("  Wire basis:      {:?}", basis_wires);
    eprintln!("  Domain digest:   {:02x?}", dom_digest);

    // ============================================================================
    // Shape sanity checks (feature-aware)
    // ============================================================================

    let k = proof.wire_comms.len();
    let s = proof.eval_points.len();
    let has_z = proof.z_comm.is_some();

    // Base expected items: [wires@ζ] + [Z@ζ?] + [Q@ζ]
    let mut expected_items = (k + usize::from(has_z)) * s + /* Q */ s;

    // If the binary is compiled with zeta-shift support, expect Z@ω·ζ as well.
    #[cfg(feature = "zeta-shift")]
    {
        if has_z {
            expected_items += s;
        }
    }

    if proof.opening_proofs.len() != expected_items || proof.evals.len() != expected_items {
        return Err(anyhow::anyhow!(
            "proof shape mismatch: k={}, s={}, has_z={}, expected items={}, got evals={}, proofs={}",
            k, s, has_z, expected_items, proof.evals.len(), proof.opening_proofs.len()
        ));
    }

    eprintln!("✓ Proof structure is valid");

    // ============================================================================
    // Delegate to protocol verifier
    // ============================================================================

    eprintln!();
    eprintln!("Running cryptographic verification...");
    
    let verify_params = VerifyParams { domain: domain.clone(), pcs_wires, pcs_coeff };
    let verifier = Verifier { params: &verify_params };

    // Replay Fiat–Shamir and enforce pairings via the scheduler.
    verifier.verify(&proof).map_err(|e| anyhow::anyhow!("verification failed: {e}"))?;

    eprintln!();
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!("✓ VERIFICATION SUCCESSFUL");
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!();
    eprintln!("The proof is cryptographically valid and was generated using");
    eprintln!("the same SRS as loaded by this verifier.");
    
    println!("Verifier result: ok");
    Ok(())
}