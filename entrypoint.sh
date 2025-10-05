#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint (running as $(whoami)) ==="

# Ensure volume directory exists and fix permissions
mkdir -p /app/srs
chown tinyzkp:tinyzkp /app/srs

# Check if SRS exists on volume
if [ -f "/app/srs/G1.bin" ] && [ -f "/app/srs/G2.bin" ]; then
    echo "✓ SRS files found on Railway volume"
    echo "Files in /app/srs:"
    ls -lh /app/srs/
    
    # Ensure correct ownership
    chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin 2>/dev/null || true
else
    echo "⚠️  No SRS files on volume yet"
    echo ""
    echo "To generate production SRS (4M degree, 128 MB):"
    echo "  railway run generate_production_srs 4194304"
    echo ""
    echo "This will create:"
    echo "  /app/srs/G1.bin (128 MB) - persists on Railway volume"
    echo "  /app/srs/G2.bin (136 bytes)"
    echo ""
    echo "Files in /app/srs:"
    ls -lh /app/srs/ || echo "  (empty)"
fi

echo "=== Starting TinyZKP API as tinyzkp user ==="
# Switch to tinyzkp user and start the application
exec su tinyzkp -c 'exec tinyzkp_api'

