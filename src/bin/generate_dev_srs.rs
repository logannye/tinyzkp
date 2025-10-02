//! Generate development SRS files (NOT FOR PRODUCTION)

use anyhow::Result;
use myzkp::srs_setup::{generate_dev_srs, save_g1_srs, save_g2_srs};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    
    // Parse --degree (supports both --degree=4096 and --degree 4096)
    let degree = if let Some(pos) = args.iter().position(|s| s == "--degree") {
        args.get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(16384)
    } else if let Some(arg) = args.iter().find(|s| s.starts_with("--degree=")) {
        arg.strip_prefix("--degree=")
            .and_then(|s| s.parse().ok())
            .unwrap_or(16384)
    } else {
        16384
    };
    
    let g1_path = args.iter()
        .position(|s| s == "--output-g1")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("G1.bin"));
    
    let g2_path = args.iter()
        .position(|s| s == "--output-g2")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("G2.bin"));
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("⚠️  WARNING: Generating DEVELOPMENT SRS (seed=42, τ is PUBLIC)");
    println!("⚠️  This SRS is NOT SECURE and must NEVER be used in production!");
    println!("⚠️  Anyone can forge proofs with this setup.");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    println!("Generating SRS: degree={}, tau=<deterministic>", degree);
    
    let (g1_powers, tau_g2) = generate_dev_srs(degree);
    
    println!("  Computed {}/{} G1 powers...", degree, degree + 1);
    println!("✓ Dev SRS generated successfully");
    println!("  G1 powers: {}", g1_powers.len());
    println!("  G2 element: τ·G₂");
    
    save_g1_srs(&g1_path, &g1_powers)?;
    save_g2_srs(&g2_path, tau_g2)?;
    
    println!("✓ Saved SRS files:");
    println!("  {}", g1_path.display());
    println!("  {}", g2_path.display());
    
    Ok(())
}