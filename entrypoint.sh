#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint (running as $(whoami)) ==="

# Ensure volume directory exists and fix permissions
mkdir -p /app/srs
chown tinyzkp:tinyzkp /app/srs

# Check if SRS exists on volume (and is the correct 512K version)
if [ -f "/app/srs/G1.bin" ] && [ -f "/app/srs/G2.bin" ]; then
    G1_SIZE=$(stat -f%z "/app/srs/G1.bin" 2>/dev/null || stat -c%s "/app/srs/G1.bin" 2>/dev/null)
    EXPECTED_SIZE=16777344  # 512K SRS = ~16MB
    
    if [ "$G1_SIZE" -eq "$EXPECTED_SIZE" ] || [ "$G1_SIZE" -lt 17000000 -a "$G1_SIZE" -gt 16500000 ]; then
        echo "✓ SRS files found on Railway volume (correct 512K version)"
        echo "Files in /app/srs:"
        ls -lh /app/srs/
        chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin 2>/dev/null || true
    else
        echo "❌ SRS files on volume are wrong size (found: $G1_SIZE bytes, expected: ~16MB)"
        echo "📦 Replacing with 512K SRS from Docker image..."
        cp /tmp/srs_image/G1.bin /app/srs/G1.bin
        cp /tmp/srs_image/G2.bin /app/srs/G2.bin
        chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin
        echo "✓ 512K SRS files copied to volume"
        echo "Files in /app/srs:"
        ls -lh /app/srs/
    fi
else
    echo "❌ No SRS files on volume"
    if [ -f "/tmp/srs_image/G1.bin" ]; then
        echo "📦 Copying 512K SRS from Docker image to volume..."
        cp /tmp/srs_image/G1.bin /app/srs/G1.bin
        cp /tmp/srs_image/G2.bin /app/srs/G2.bin
        chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin
        echo "✓ 512K SRS files copied to volume"
        echo "Files in /app/srs:"
        ls -lh /app/srs/
    else
        echo "⚠️  No SRS files in Docker image or on volume"
        echo "Files in /app/srs:"
        ls -lh /app/srs/ || echo "  (empty)"
    fi
fi

echo "=== Starting TinyZKP API as tinyzkp user ==="
# Switch to tinyzkp user and start the application
exec su tinyzkp -c 'exec tinyzkp_api'

