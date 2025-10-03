#!/bin/sh
set -e

echo "=== TinyZKP Entrypoint (running as $(whoami)) ==="

# Check if source files exist
echo "Checking for SRS files in /tmp/srs-init..."
ls -lh /tmp/srs-init/ || echo "ERROR: /tmp/srs-init not found!"

# Ensure volume directory exists and fix permissions
mkdir -p /app/srs
chown tinyzkp:tinyzkp /app/srs

# Initialize SRS volume if empty OR if files have changed (running as root to access volume)
NEEDS_UPDATE=false

if [ ! -f "/app/srs/G1.bin" ]; then
    echo "SRS files not found in volume - will initialize..."
    NEEDS_UPDATE=true
else
    # Check if file sizes differ (indicates SRS upgrade)
    SOURCE_SIZE=$(stat -c%s /tmp/srs-init/G1.bin 2>/dev/null || stat -f%z /tmp/srs-init/G1.bin)
    VOLUME_SIZE=$(stat -c%s /app/srs/G1.bin 2>/dev/null || stat -f%z /app/srs/G1.bin)
    
    if [ "$SOURCE_SIZE" != "$VOLUME_SIZE" ]; then
        echo "SRS files have changed (source: $SOURCE_SIZE bytes, volume: $VOLUME_SIZE bytes)"
        echo "Updating volume with new SRS..."
        NEEDS_UPDATE=true
    else
        echo "SRS files already up-to-date in volume"
    fi
fi

if [ "$NEEDS_UPDATE" = "true" ]; then
    echo "Copying SRS files to volume..."
    cp -v /tmp/srs-init/G1.bin /app/srs/
    cp -v /tmp/srs-init/G2.bin /app/srs/
    # Fix ownership after copying
    chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin
    echo "✓ SRS files updated in volume"
fi

# Ensure correct ownership
chown tinyzkp:tinyzkp /app/srs/G1.bin /app/srs/G2.bin 2>/dev/null || true

# Verify files in volume
echo "Files in /app/srs:"
ls -lh /app/srs/

echo "=== Starting TinyZKP API as tinyzkp user ==="
# Switch to tinyzkp user and start the application
exec su tinyzkp -c 'exec tinyzkp_api'

