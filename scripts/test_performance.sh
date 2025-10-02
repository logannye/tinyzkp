#!/bin/bash
set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Performance & Limits Test Suite"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

export $(grep -v '^#' .env | xargs)
BASE_URL="http://127.0.0.1:8080"

# Create test user
SIGNUP_RESP=$(curl -s -X POST $BASE_URL/v1/auth/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"perf_$(date +%s)@test.com\",\"password\":\"password123\"}")
API_KEY=$(echo $SIGNUP_RESP | grep -o '"api_key":"[^"]*' | cut -d'"' -f4)

# Test 1: Maximum rows for free tier
echo ""
echo "Test 1: Free tier row limit (4096)"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "air":{"k":3},
        "domain":{"rows":4096,"b_blk":64},
        "pcs":{"basis_wires":"eval"},
        "witness":{"format":"json_rows","rows":[[1,2,3]]}
    }')

if echo "$RESPONSE" | grep -q "header"; then
    echo "✓ Accepted 4096 rows for free tier"
else
    echo "❌ Should accept 4096 rows: $RESPONSE"
fi

# Test 2: Exceed free tier limit
echo ""
echo "Test 2: Free tier row limit exceeded (4097)"
RESPONSE=$(curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "air":{"k":3},
        "domain":{"rows":4097,"b_blk":64},
        "pcs":{"basis_wires":"eval"},
        "witness":{"format":"json_rows","rows":[[1,2,3]]}
    }')

if echo "$RESPONSE" | grep -q "exceeds tier limit"; then
    echo "✓ Correctly rejected rows exceeding free tier"
else
    echo "❌ Should reject 4097 rows for free tier: $RESPONSE"
fi

# Test 3: Small circuit performance
echo ""
echo "Test 3: Small circuit timing (N=256)"
START=$(date +%s%N)
curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "air":{"k":3},"domain":{"rows":256,"b_blk":16},"pcs":{"basis_wires":"eval"},
        "witness":{"format":"json_rows","rows":[[1,2,3]]}
    }' > /dev/null
END=$(date +%s%N)
ELAPSED_MS=$(( (END - START) / 1000000 ))
echo "  Elapsed: ${ELAPSED_MS}ms"
if [ $ELAPSED_MS -lt 5000 ]; then
    echo "✓ Small circuit performance acceptable"
else
    echo "⚠️  Small circuit slower than expected"
fi

# Test 4: Memory test (enable diagnostics)
echo ""
echo "Test 4: Memory diagnostics (N=1024, b_blk=32)"
echo "  (Check logs for O(b_blk) memory usage)"

# Generate witness rows programmatically
ROWS="["
for i in {1..50}; do
    ROWS="$ROWS[1,2,3],"
done
ROWS="${ROWS%,}]"

SSZKP_MEMLOG=1 cargo run --release --bin tinyzkp_api 2>&1 | tee /tmp/api_memlog.txt &
API_PID=$!
sleep 3

curl -s -X POST $BASE_URL/v1/admin/srs/init \
    -H "X-Admin-Token: $TINYZKP_ADMIN_TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"max_degree":4096,"validate_pairing":false}' > /dev/null

curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d "{
        \"air\":{\"k\":3},\"domain\":{\"rows\":1024,\"b_blk\":32},\"pcs\":{\"basis_wires\":\"eval\"},
        \"witness\":{\"format\":\"json_rows\",\"rows\":$ROWS}
    }" > /dev/null

kill $API_PID 2>/dev/null || true

if grep -q "tile\|peak" /tmp/api_memlog.txt; then
    echo "✓ Memory diagnostics available"
else
    echo "⚠️  No memory diagnostics in logs"
fi

# Test 5: Usage cap enforcement
echo ""
echo "Test 5: Monthly usage cap"
INITIAL_RESP=$(curl -s $BASE_URL/v1/me -H "Authorization: Bearer $(echo $SIGNUP_RESP | grep -o '"session_token":"[^"]*' | cut -d'"' -f4)")
INITIAL_USED=$(echo $INITIAL_RESP | grep -o '"used":[0-9]*' | cut -d':' -f2)

# Make another request
curl -s -X POST $BASE_URL/v1/prove \
    -H "X-API-Key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{"air":{"k":3},"domain":{"rows":64,"b_blk":8},"pcs":{},"witness":{"format":"json_rows","rows":[[1,2,3]]}}' > /dev/null

AFTER_RESP=$(curl -s $BASE_URL/v1/me -H "Authorization: Bearer $(echo $SIGNUP_RESP | grep -o '"session_token":"[^"]*' | cut -d'"' -f4)")
AFTER_USED=$(echo $AFTER_RESP | grep -o '"used":[0-9]*' | cut -d':' -f2)

if [ "$AFTER_USED" -gt "$INITIAL_USED" ]; then
    echo "✓ Usage counter incrementing ($INITIAL_USED → $AFTER_USED)"
else
    echo "❌ Usage counter not working"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Performance tests complete"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"