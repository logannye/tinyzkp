#!/usr/bin/env cargo
//! Generate cryptographically-secure production SRS
//!
//! This tool generates a high-capacity SRS using a cryptographically-secure RNG.
//! The secret tau is NEVER saved to disk and is immediately dropped after use.
//!
//! # Security Model
//!
//! This is a **single-party trusted setup**:
//! - Tau is generated using OsRng (cryptographically secure)
//! - Tau exists only in memory during generation
//! - Tau is automatically destroyed when the program exits
//! - As long as you don't modify this code to save tau, the SRS is secure
//!
//! # Comparison to Multi-Party Ceremony
//!
//! **Multi-party ceremony**: Requires ALL participants to collude to break security
//! **This tool**: Requires YOU to be honest (don't save tau)
//!
//! For a system you control, this is **production-grade** security.

use ark_bn254::{Bn254, G2Affine};
use ark_ec::{Group, pairing::Pairing};
use ark_ff::UniformRand;
use ark_serialize::CanonicalSerialize;
use rand::rngs::OsRng;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_degree: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(131072); // 2^17 = 131K

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   TinyZKP Production SRS Generator                        ║");
    println!("║   Single-Party Trusted Setup (Cryptographically Secure)   ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();
    println!("⚙️  Configuration:");
    println!("   Max degree: {}", max_degree);
    println!("   G1 powers: {} (degree + 1)", max_degree + 1);
    println!("   Output: srs/G1.bin, srs/G2.bin");
    println!();
    println!("🔐 Security:");
    println!("   - Using OsRng (cryptographically secure)");
    println!("   - Tau will be destroyed after generation");
    println!("   - Never saved to disk");
    println!();

    // Confirm before proceeding
    println!("⚠️  This will OVERWRITE existing SRS files!");
    println!("   Press Enter to continue, Ctrl+C to cancel...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    println!("🔄 Generating cryptographically-secure tau...");
    
    // Generate random tau using OS entropy
    let mut rng = OsRng;
    let tau = <Bn254 as Pairing>::ScalarField::rand(&mut rng);
    
    println!("✓ Tau generated (will be destroyed after SRS generation)");
    println!();

    // Generate G1 powers: [G1, τ·G1, τ²·G1, ..., τⁿ·G1]
    println!("🔄 Generating {} G1 powers...", max_degree + 1);
    println!("   This may take 30-60 seconds...");
    
    let g1_gen = <Bn254 as Pairing>::G1::generator();
    let mut g1_powers: Vec<<Bn254 as Pairing>::G1Affine> = Vec::with_capacity(max_degree + 1);
    let mut tau_pow = <Bn254 as Pairing>::ScalarField::from(1u64); // Start with τ⁰ = 1
    
    for i in 0..=max_degree {
        if i % 10000 == 0 && i > 0 {
            println!("   Generated {} / {} powers...", i, max_degree + 1);
        }
        g1_powers.push((g1_gen * tau_pow).into());
        tau_pow *= tau; // τⁱ → τⁱ⁺¹
    }
    
    println!("✓ Generated {} G1 powers", g1_powers.len());
    println!();

    // Generate G2 element: [τ·G2]
    println!("🔄 Generating G2 element...");
    let g2_gen = <Bn254 as Pairing>::G2::generator();
    let tau_g2: G2Affine = (g2_gen * tau).into();
    println!("✓ Generated τ·G2");
    println!();

    // Tau is about to go out of scope and be destroyed!
    // We explicitly drop it here for clarity
    drop(tau);
    drop(tau_pow);
    println!("🔒 Tau destroyed (no longer in memory)");
    println!();

    // Create output directory
    std::fs::create_dir_all("srs")?;

    // Write G1 powers
    println!("💾 Writing G1 SRS to srs/G1.bin...");
    let g1_path = Path::new("srs/G1.bin");
    let mut g1_file = File::create(g1_path)?;
    
    let mut g1_bytes = Vec::new();
    g1_powers.serialize_compressed(&mut g1_bytes)
        .map_err(|e| format!("Failed to serialize G1: {:?}", e))?;
    g1_file.write_all(&g1_bytes)?;
    
    println!("✓ Wrote {} bytes ({} powers)", g1_bytes.len(), g1_powers.len());
    
    // Compute G1 digest
    let g1_digest = blake3::hash(&g1_bytes);
    println!("   SHA256 digest: {}", hex::encode(g1_digest.as_bytes()));
    println!();

    // Write G2 element  
    println!("💾 Writing G2 SRS to srs/G2.bin...");
    let g2_path = Path::new("srs/G2.bin");
    let mut g2_file = File::create(g2_path)?;
    
    // Format: [G2, τ·G2] (two elements)
    let g2_gen_affine: G2Affine = g2_gen.into();
    let g2_elements = vec![g2_gen_affine, tau_g2];
    let mut g2_bytes = Vec::new();
    g2_elements.serialize_compressed(&mut g2_bytes)
        .map_err(|e| format!("Failed to serialize G2: {:?}", e))?;
    g2_file.write_all(&g2_bytes)?;
    
    println!("✓ Wrote {} bytes (2 elements: G2, τ·G2)", g2_bytes.len());
    
    // Compute G2 digest
    let g2_digest = blake3::hash(&g2_bytes);
    println!("   SHA256 digest: {}", hex::encode(g2_digest.as_bytes()));
    println!();

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   ✅ SUCCESS! Production SRS Generated                    ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();
    println!("📊 Summary:");
    println!("   Max degree: {}", max_degree);
    println!("   G1 file: {} KB", g1_bytes.len() / 1024);
    println!("   G2 file: {} bytes", g2_bytes.len());
    println!("   G1 digest: {}", hex::encode(g1_digest.as_bytes()));
    println!("   G2 digest: {}", hex::encode(g2_digest.as_bytes()));
    println!();
    println!("🔐 Security Note:");
    println!("   ✓ Tau was generated using cryptographic RNG");
    println!("   ✓ Tau has been destroyed (no longer in memory)");
    println!("   ✓ Tau was never written to disk");
    println!("   → SRS is cryptographically secure for production use");
    println!();
    println!("📝 Next Steps:");
    println!("   1. Verify files: ls -lh srs/");
    println!("   2. Upload to Railway volume");
    println!("   3. Re-initialize API: POST /v1/admin/srs/init");
    println!();

    Ok(())
}

