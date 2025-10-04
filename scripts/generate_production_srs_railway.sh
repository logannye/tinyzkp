#!/bin/bash
# Generate production SRS directly on Railway (for SRS files >100MB that can't be in git)

set -e

echo "=================================================="
echo "Production SRS Generator for Railway"
echo "=================================================="
echo ""
echo "This script generates large SRS files (>100MB) directly on Railway."
echo "GitHub limits files to 100MB, so we generate them server-side."
echo ""

# Check if we're on Railway
if [ -z "$RAILWAY_ENVIRONMENT" ]; then
    echo "❌ This script must be run on Railway (RAILWAY_ENVIRONMENT not set)"
    echo ""
    echo "To run on Railway:"
    echo "1. railway run bash"
    echo "2. ./scripts/generate_production_srs_railway.sh"
    exit 1
fi

echo "✓ Running on Railway environment: $RAILWAY_ENVIRONMENT"
echo ""

# Check if cargo is available
if ! command -v cargo &> /dev/null; then
    echo "❌ cargo not found. This requires the Rust toolchain."
    exit 1
fi

echo "🔧 Generating 4M degree SRS (4,194,304 rows)..."
echo "   This will take approximately 10-20 minutes..."
echo "   Output: /app/srs/G1.bin (128 MB), /app/srs/G2.bin (136 bytes)"
echo ""

# Generate directly to the Railway volume
cd /app
cargo run --release --bin generate_production_srs -- 4194304

echo ""
echo "=================================================="
echo "✅ Production SRS Generated Successfully!"
echo "=================================================="
echo ""
echo "Next steps:"
echo "1. Restart the Railway service to load the new SRS"
echo "2. The SRS will persist on the Railway volume across deployments"
echo ""
echo "To verify:"
echo "  ls -lh /app/srs/"
echo ""
