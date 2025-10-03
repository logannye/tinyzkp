#!/bin/sh
set -e

# Initialize SRS volume if empty
if [ ! -f "/app/srs/G1.bin" ]; then
    echo "Initializing SRS volume from /tmp/srs-init..."
    cp -v /tmp/srs-init/* /app/srs/
    echo "SRS files copied to volume"
else
    echo "SRS files already exist in volume"
fi

# Start the application
exec tinyzkp_api

