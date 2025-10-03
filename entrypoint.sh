#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint (running as $(whoami)) ==="

# Check if source files exist
echo "Checking for SRS files in /tmp/srs-init..."
ls -lh /tmp/srs-init/ || echo "ERROR: /tmp/srs-init not found!"

# Ensure volume directory exists and fix permissions
mkdir -p /app/srs
chown tinyzkp:tinyzkp /app/srs

# Initialize SRS volume if empty (running as root to access volume)
if [ ! -f "/app/srs/G1.bin" ]; then
    echo "Initializing SRS volume from /tmp/srs-init..."
    cp -v /tmp/srs-init/G1.bin /app/srs/
    cp -v /tmp/srs-init/G2.bin /app/srs/
    # Fix ownership after copying
    chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin
    echo "SRS files copied to volume and ownership set"
else
    echo "SRS files already exist in volume"
    # Ensure correct ownership even if files exist
    chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin 2>/dev/null || true
fi

# Verify files in volume
echo "Files in /app/srs:"
ls -lh /app/srs/

echo "=== Starting TinyZKP API as tinyzkp user ==="
# Switch to tinyzkp user and start the application
exec su tinyzkp -c 'exec tinyzkp_api'

