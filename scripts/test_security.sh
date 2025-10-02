#!/bin/bash
set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Security Test Suite"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Load environment
export $(grep -v '^#' .env | xargs)
BASE_URL="http://127.0.0.1:8080"

# Ensure API is running
if ! curl -s $BASE_URL/v1/health > /dev/null; then
    echo "❌ API server not running. Start with: cargo run --release --bin tinyzkp_api"
    exit 1
fi

# Test 1: Missing API key
echo ""
echo "Test 1: Missing API key rejection"
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST $BASE_URL/v1/prove \
    -H "Content-Type: application/json" \
    -d '{"air":{"k":3},"domain":{"rows":64,"b_blk":8},"pcs":{},"witness":{"format":"json_rows","rows":[[1,2,3]]}}')
HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
if [ "$HTTP_CODE" = "401" ]; then
    echo "✓ Correctly rejected missing API key"
else
    echo "❌ Should reject missing API key (got: $HTTP_CODE)"
fi

# Test 2: Invalid API key
echo ""
echo "Test 2: Invalid API key rejection"
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: tz_invalid_key_12345" \
    -H "Content-Type: application/json" \
    -d '{"air":{"k":3},"domain":{"rows":64,"b_blk":8},"pcs":{},"witness":{"format":"json_rows","rows":[[1,2,3]]}}')
HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
if [ "$HTTP_CODE" = "401" ]; then
    echo "✓ Correctly rejected invalid API key"
else
    echo "❌ Should reject invalid API key (got: $HTTP_CODE)"
fi

# Test 3: Missing admin token
echo ""
echo "Test 3: Missing admin token rejection"
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST $BASE_URL/v1/admin/keys \
    -H "Content-Type: application/json")
HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
if [ "$HTTP_CODE" = "401" ]; then
    echo "✓ Correctly rejected missing admin token"
else
    echo "❌ Should reject missing admin token (got: $HTTP_CODE)"
fi

# Test 4: Invalid admin token
echo ""
echo "Test 4: Invalid admin token rejection"
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST $BASE_URL/v1/admin/keys \
    -H "X-Admin-Token: wrong_token" \
    -H "Content-Type: application/json")
HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
if [ "$HTTP_CODE" = "401" ]; then
    echo "✓ Correctly rejected invalid admin token"
else
    echo "❌ Should reject invalid admin token (got: $HTTP_CODE)"
fi

# Test 5: Weak password rejection
echo ""
echo "Test 5: Weak password rejection"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d '{"email":"weak@test.com","password":"123"}')
if echo "$RESPONSE" | grep -q "too short"; then
    echo "✓ Correctly rejected weak password"
else
    echo "❌ Should reject weak password"
fi

# Test 6: Invalid email rejection
echo ""
echo "Test 6: Invalid email rejection"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d '{"email":"not-an-email","password":"password123"}')
if echo "$RESPONSE" | grep -q "invalid email"; then
    echo "✓ Correctly rejected invalid email"
else
    echo "❌ Should reject invalid email"
fi

# Test 7: Duplicate email rejection
echo ""
echo "Test 7: Duplicate email rejection"
TEST_EMAIL="duplicate_$(date +%s)@test.com"
curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"password123\"}" > /dev/null

RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"password123\"}")

if echo "$RESPONSE" | grep -q "already registered"; then
    echo "✓ Correctly rejected duplicate email"
else
    echo "❌ Should reject duplicate email"
fi

# Test 8: Wrong password rejection
echo ""
echo "Test 8: Wrong password rejection"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/auth/login \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"wrongpassword\"}")

if echo "$RESPONSE" | grep -q "invalid credentials"; then
    echo "✓ Correctly rejected wrong password"
else
    echo "❌ Should reject wrong password"
fi

# Test 9: Tampered proof rejection
echo ""
echo "Test 9: Tampered proof rejection"
# Generate a valid proof
SIGNUP_RESP=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"tamper_$(date +%s)@test.com\",\"password\":\"password123\"}")
API_KEY=$(echo $SIGNUP_RESP | grep -o '"api_key":"[^"]*' | cut -d'"' -f4)

PROOF_RESP=$(curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "air":{"k":3},"domain":{"rows":64,"b_blk":8},"pcs":{"basis_wires":"eval"},
        "witness":{"format":"json_rows","rows":[[1,2,3],[4,5,6],[7,8,9]]},
        "return_proof":true
    }')

PROOF_B64=$(echo $PROOF_RESP | grep -o '"proof_b64":"[^"]*' | cut -d'"' -f4)
echo "$PROOF_B64" | base64 -d > /tmp/tamper_proof.bin

# Tamper with proof
dd if=/dev/urandom of=/tmp/tamper_proof.bin bs=1 count=1 seek=200 conv=notrunc 2>/dev/null

# Try to verify
VERIFY_RESP=$(curl -s -X POST $BASE_URL/v1/verify \
    -H "X-API-Key: $API_KEY" \
    -F "proof=@/tmp/tamper_proof.bin")

if echo "$VERIFY_RESP" | grep -q '"status":"failed"'; then
    echo "✓ Correctly rejected tampered proof"
else
    echo "❌ Should reject tampered proof: $VERIFY_RESP"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Security tests complete"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"