#!/bin/bash
# Deploy 4M SRS to Railway by generating it directly on the Railway container

set -e

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║   Deploy 4M SRS to Railway                                ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "This script will:"
echo "  1. Access your Railway container"
echo "  2. Generate the 4M degree SRS (takes 10-20 minutes)"
echo "  3. Verify the files"
echo "  4. Restart your Railway service"
echo "  5. Check logs to confirm SRS loaded"
echo ""
echo "⚠️  Note: The generation step takes 10-20 minutes."
echo "    Your terminal will show progress every 10,000 powers."
echo ""

# Check Railway CLI is available
if ! command -v railway &> /dev/null; then
    echo "❌ Railway CLI not found. Install it first:"
    echo "   npm install -g @railway/cli"
    exit 1
fi

# Check we're logged in
if ! railway whoami &> /dev/null; then
    echo "❌ Not logged in to Railway. Run: railway login"
    exit 1
fi

# Check we're linked to a project
if ! railway status &> /dev/null; then
    echo "❌ Not linked to a Railway project. Run: railway link"
    exit 1
fi

echo "✅ Railway CLI ready"
echo "✅ Logged in: $(railway whoami | head -1)"
echo "✅ Project: $(railway status 2>&1 | grep 'Project:' | cut -d' ' -f2-)"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 1: Generate SRS on Railway (10-20 minutes)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Running: railway run 'cd /app && cargo run --release --bin generate_production_srs -- 4194304'"
echo ""

railway run 'cd /app && cargo run --release --bin generate_production_srs -- 4194304'

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 2: Verify SRS files were created"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

railway run 'ls -lh /app/srs/'

echo ""
echo "Expected output:"
echo "  -rw-r--r-- 128M G1.bin"
echo "  -rw-r--r-- 136  G2.bin"
echo ""

read -p "Do the files look correct? (y/n): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Files don't look right. Check the output above."
    exit 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 3: Restart Railway service"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

railway restart

echo ""
echo "Waiting 10 seconds for service to restart..."
sleep 10

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Step 4: Check logs for SRS initialization"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

railway logs --tail 50

echo ""
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║   ✅ Deployment Complete!                                 ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Your Railway API now supports:"
echo "  • Scale tier: 4,194,304 rows per circuit"
echo "  • Memory usage: ~32 KB per proof"
echo "  • zkML models: ResNet-34, BERT-base, GPT-2-medium"
echo ""
echo "Look for this in the logs above:"
echo "  ✓ Production SRS loaded successfully"
echo "  SRS initialized successfully:"
echo "    Max degree: 4194304"
echo ""
echo "Test with: curl https://api.tinyzkp.com/v1/health"
echo ""
