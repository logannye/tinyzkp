#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint (running as $(whoami)) ==="

# Ensure SRS directory exists with correct permissions
mkdir -p /app/srs
chown tinyzkp:tinyzkp /app/srs

# Check if SRS files exist
if [ -f "/app/srs/G1.bin" ] && [ -f "/app/srs/G2.bin" ]; then
    echo "✓ SRS files found in volume"
    echo "Files in /app/srs:"
    ls -lh /app/srs/
    
    # Ensure correct ownership
    chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin 2>/dev/null || true
else
    echo "⚠️  SRS files not found in /app/srs/"
    echo "   Generate SRS using: railway ssh 'generate_production_srs 4194304'"
    echo "   The API will start but proofs will fail until SRS is generated."
fi

echo "=== Starting TinyZKP API as tinyzkp user ==="
# Switch to tinyzkp user and start the application
exec su tinyzkp -c 'exec tinyzkp_api'