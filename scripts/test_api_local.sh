#!/bin/bash
set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "API Local Test Suite"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check prerequisites
if [ ! -f .env ]; then
    echo "❌ Missing .env file. Copy .env.example and fill in values."
    exit 1
fi

if [ ! -f srs/G1.bin ] || [ ! -f srs/G2.bin ]; then
    echo "⚠️  SRS files missing. Generating dev SRS..."
    ./scripts/generate_dev_srs.sh 4096 ./srs
fi

# Load environment
export $(grep -v '^#' .env | xargs)

# Build API
echo "Building API server..."
cargo build --release --bin tinyzkp_api

# Start API server
echo "Starting API server..."
./target/release/tinyzkp_api &
API_PID=$!
sleep 3

# Cleanup function
cleanup() {
    echo "Stopping API server..."
    kill $API_PID 2>/dev/null || true
}
trap cleanup EXIT

BASE_URL="http://127.0.0.1:8080"

# Test 1: Health check
echo ""
echo "Test 1: Health check"
RESPONSE=$(curl -s $BASE_URL/v1/health)
if echo "$RESPONSE" | grep -q "ok"; then
    echo "✓ Health check passed"
else
    echo "❌ Health check failed: $RESPONSE"
    exit 1
fi

# Test 2: Version info
echo ""
echo "Test 2: Version info"
RESPONSE=$(curl -s $BASE_URL/v1/version)
if echo "$RESPONSE" | grep -q "tinyzkp-api"; then
    echo "✓ Version endpoint passed"
else
    echo "❌ Version endpoint failed: $RESPONSE"
    exit 1
fi

# Test 3: Initialize SRS
echo ""
echo "Test 3: SRS initialization"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/admin/srs/init \
    -H "X-Admin-Token: $TINYZKP_ADMIN_TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"max_degree": 4096, "validate_pairing": false}')

if echo "$RESPONSE" | grep -q "initialized"; then
    echo "✓ SRS initialization passed"
    echo "  G1 digest: $(echo $RESPONSE | grep -o 'g1_digest_hex":"[^"]*' | cut -d'"' -f3 | head -c 16)..."
else
    echo "❌ SRS initialization failed: $RESPONSE"
    exit 1
fi

# Test 4: Signup
echo ""
echo "Test 4: User signup"
TEST_EMAIL="test_$(date +%s)@example.com"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"testpass123\"}")

if echo "$RESPONSE" | grep -q "api_key"; then
    API_KEY=$(echo $RESPONSE | grep -o '"api_key":"[^"]*' | cut -d'"' -f4)
    echo "✓ Signup passed"
    echo "  API Key: ${API_KEY:0:20}..."
else
    echo "❌ Signup failed: $RESPONSE"
    exit 1
fi

# Test 5: Login
echo ""
echo "Test 5: User login"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/login \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"testpass123\"}")

if echo "$RESPONSE" | grep -q "session_token"; then
    SESSION_TOKEN=$(echo $RESPONSE | grep -o '"session_token":"[^"]*' | cut -d'"' -f4)
    echo "✓ Login passed"
else
    echo "❌ Login failed: $RESPONSE"
    exit 1
fi

# Test 6: Get account info
echo ""
echo "Test 6: Account info"
RESPONSE=$(curl -s $BASE_URL/v1/me \
    -H "Authorization: Bearer $SESSION_TOKEN")

if echo "$RESPONSE" | grep -q "$TEST_EMAIL"; then
    echo "✓ Account info passed"
else
    echo "❌ Account info failed: $RESPONSE"
    exit 1
fi

# Test 7: Prove (small circuit)
echo ""
echo "Test 7: Generate proof"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "air": {"k": 3},
        "domain": {"rows": 64, "b_blk": 8},
        "pcs": {"basis_wires": "eval"},
        "witness": {
            "format": "json_rows",
            "rows": [
                [1,2,3], [4,5,6], [7,8,9], [10,11,12],
                [1,2,3], [4,5,6], [7,8,9], [10,11,12],
                [1,2,3], [4,5,6], [7,8,9], [10,11,12]
            ]
        },
        "return_proof": true
    }')

if echo "$RESPONSE" | grep -q "proof_b64"; then
    PROOF_B64=$(echo $RESPONSE | grep -o '"proof_b64":"[^"]*' | cut -d'"' -f4)
    echo "✓ Proof generation passed"
    echo "  Proof size: ${#PROOF_B64} bytes (base64)"
else
    echo "❌ Proof generation failed: $RESPONSE"
    exit 1
fi

# Test 8: Verify proof
echo ""
echo "Test 8: Verify proof"
# Save proof to file
echo "$PROOF_B64" | base64 -d > /tmp/test_proof.bin

# Verify using multipart
RESPONSE=$(curl -s -X POST $BASE_URL/v1/verify \
    -H "X-API-Key: $API_KEY" \
    -F "proof=@/tmp/test_proof.bin")

if echo "$RESPONSE" | grep -q '"status":"ok"'; then
    echo "✓ Proof verification passed"
else
    echo "❌ Proof verification failed: $RESPONSE"
    exit 1
fi

# Test 9: Usage cap check
echo ""
echo "Test 9: Usage tracking"
RESPONSE=$(curl -s $BASE_URL/v1/me \
    -H "Authorization: Bearer $SESSION_TOKEN")

USED=$(echo $RESPONSE | grep -o '"used":[0-9]*' | cut -d':' -f2)
if [ "$USED" -ge 2 ]; then
    echo "✓ Usage tracking working (used: $USED)"
else
    echo "⚠️  Usage tracking may not be working (used: $USED)"
fi

# Test 10: Domain planning
echo ""
echo "Test 10: Domain planning"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/domain/plan \
    -H "Content-Type: application/json" \
    -d '{"rows": 1024, "b_blk": 32}')

if echo "$RESPONSE" | grep -q '"omega_ok":true'; then
    echo "✓ Domain planning passed"
else
    echo "❌ Domain planning failed: $RESPONSE"
    exit 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ All tests passed!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"