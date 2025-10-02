#!/bin/bash

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Production Readiness Assessment"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

ISSUES=0

# Check 1: Required files
echo "Checking required files..."
REQUIRED_FILES=(.env .gitignore README.md DEPLOYMENT.md Dockerfile railway.toml LICENSE)
for file in "${REQUIRED_FILES[@]}"; do
    if [ ! -f "$file" ]; then
        echo "  ❌ Missing: $file"
        ((ISSUES++))
    else
        echo "  ✓ $file"
    fi
done

# Check 2: Gitignore protects secrets
echo ""
echo "Checking .gitignore..."
if grep -q "^.env$" .gitignore; then
    echo "  ✓ .env is gitignored"
else
    echo "  ❌ .env not in .gitignore!"
    ((ISSUES++))
fi

# Check 3: No secrets in git
echo ""
echo "Checking for secrets in tracked files..."
if git ls-files | xargs grep -l "sk_live_\|sk_test_\|whsec_" 2>/dev/null; then
    echo "  ❌ Found secrets in tracked files!"
    ((ISSUES++))
else
    echo "  ✓ No secrets found in tracked files"
fi

# Check 4: Environment variables
echo ""
echo "Checking environment variables..."
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
    REQUIRED_VARS=(UPSTASH_REDIS_REST_URL UPSTASH_REDIS_REST_TOKEN TINYZKP_ADMIN_TOKEN STRIPE_SECRET_KEY)
    for var in "${REQUIRED_VARS[@]}"; do
        if [ -z "${!var}" ]; then
            echo "  ❌ Missing: $var"
            ((ISSUES++))
        else
            echo "  ✓ $var is set"
        fi
    done
else
    echo "  ❌ .env file missing"
    ((ISSUES++))
fi

# Check 5: SRS files
echo ""
echo "Checking SRS files..."
if [ -f srs/G1.bin ] && [ -f srs/G2.bin ]; then
    G1_SIZE=$(stat -f%z srs/G1.bin 2>/dev/null || stat -c%s srs/G1.bin 2>/dev/null)
    G2_SIZE=$(stat -f%z srs/G2.bin 2>/dev/null || stat -c%s srs/G2.bin 2>/dev/null)
    echo "  ✓ G1.bin: $G1_SIZE bytes"
    echo "  ✓ G2.bin: $G2_SIZE bytes"
else
    echo "  ⚠️  SRS files missing (generate with ./scripts/generate_dev_srs.sh)"
fi

# Check 6: Build production binary
echo ""
echo "Checking production build..."
if cargo build --release --bin tinyzkp_api 2>&1 | grep -q "Finished"; then
    echo "  ✓ Production build succeeds"
else
    echo "  ❌ Production build failed"
    ((ISSUES++))
fi

# Check 7: Run test suites
echo ""
echo "Running test suites..."

if [ -f scripts/test_api_local.sh ]; then
    chmod +x scripts/test_api_local.sh
    if ./scripts/test_api_local.sh > /tmp/test_api.log 2>&1; then
        echo "  ✓ API tests passed"
    else
        echo "  ❌ API tests failed (see /tmp/test_api.log)"
        ((ISSUES++))
    fi
else
    echo "  ⚠️  API test script missing"
fi

if [ -f scripts/test_security.sh ]; then
    chmod +x scripts/test_security.sh
    ./target/release/tinyzkp_api &
    API_PID=$!
    sleep 3
    curl -s -X POST http://127.0.0.1:8080/v1/admin/srs/init \
        -H "X-Admin-Token: $TINYZKP_ADMIN_TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"max_degree":4096,"validate_pairing":false}' > /dev/null
    
    if ./scripts/test_security.sh > /tmp/test_security.log 2>&1; then
        echo "  ✓ Security tests passed"
    else
        echo "  ❌ Security tests failed (see /tmp/test_security.log)"
        ((ISSUES++))
    fi
    kill $API_PID 2>/dev/null || true
else
    echo "  ⚠️  Security test script missing"
fi

# Summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ $ISSUES -eq 0 ]; then
    echo "✓ Production Ready: All checks passed!"
    echo ""
    echo "Next steps:"
    echo "  1. Obtain production SRS from trusted ceremony"
    echo "  2. Deploy to Railway: railway up"
    echo "  3. Initialize SRS via admin endpoint"
    echo "  4. Configure Stripe webhook"
    echo "  5. Test end-to-end on production URL"
else
    echo "❌ Found $ISSUES issue(s) - address before production"
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"