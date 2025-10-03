#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint ==="

# Check if source files exist
echo "Checking for SRS files in /tmp/srs-init..."
ls -lh /tmp/srs-init/ || echo "ERROR: /tmp/srs-init not found!"

# Ensure volume directory exists
mkdir -p /app/srs

# Initialize SRS volume if empty
if [ ! -f "/app/srs/G1.bin" ]; then
    echo "Initializing SRS volume from /tmp/srs-init..."
    cp -v /tmp/srs-init/G1.bin /app/srs/
    cp -v /tmp/srs-init/G2.bin /app/srs/
    echo "SRS files copied to volume"
else
    echo "SRS files already exist in volume"
fi

# Verify files in volume
echo "Files in /app/srs:"
ls -lh /app/srs/

echo "=== Starting TinyZKP API ==="
# Start the application
exec tinyzkp_api

