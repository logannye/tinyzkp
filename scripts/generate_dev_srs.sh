#!/usr/bin/env bash
set -euo pipefail

DEGREE=${1:-16384}
OUTPUT_DIR=${2:-.}

echo "Generating dev SRS for degree $DEGREE..."
echo "⚠️  WARNING: This SRS is NOT SECURE - for testing only!"

# Create output directory if it doesn't exist
mkdir -p "$OUTPUT_DIR"

cargo run --release --features dev-srs --bin generate_dev_srs -- \
    --degree "$DEGREE" \
    --output-g1 "$OUTPUT_DIR/G1.bin" \
    --output-g2 "$OUTPUT_DIR/G2.bin"

echo "✓ Generated:"
echo "  $OUTPUT_DIR/G1.bin ($(du -h "$OUTPUT_DIR/G1.bin" | cut -f1))"
echo "  $OUTPUT_DIR/G2.bin ($(du -h "$OUTPUT_DIR/G2.bin" | cut -f1))"