#!/bin/bash
# Test script for frontend-backend signup flow verification
# Tests all 3 critical fixes:
# 1. /v1/signup endpoint
# 2. Email field in response
# 3. Duplicate email error handling

set -e

API_URL="${API_URL:-http://localhost:3030}"
TEST_EMAIL="test_$(date +%s)@example.com"
TEST_PASSWORD="SecurePass123"

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  🧪 Testing Frontend-Backend Signup Flow                 ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "API URL: $API_URL"
echo "Test Email: $TEST_EMAIL"
echo ""

# Test 1: Check if /v1/signup endpoint exists (FIX #1)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 1: Verify /v1/signup endpoint exists"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

SIGNUP_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/signup" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

HTTP_CODE=$(echo "$SIGNUP_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$SIGNUP_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
  echo "✅ PASS: /v1/signup endpoint exists (HTTP $HTTP_CODE)"
else
  echo "❌ FAIL: /v1/signup endpoint returned HTTP $HTTP_CODE"
  echo "Response: $RESPONSE_BODY"
  exit 1
fi

echo ""

# Test 2: Verify email field is in response (FIX #2)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 2: Verify signup response includes email field"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

USER_ID=$(echo "$RESPONSE_BODY" | jq -r '.user_id // empty')
EMAIL=$(echo "$RESPONSE_BODY" | jq -r '.email // empty')
API_KEY=$(echo "$RESPONSE_BODY" | jq -r '.api_key // empty')
TIER=$(echo "$RESPONSE_BODY" | jq -r '.tier // empty')
SESSION_TOKEN=$(echo "$RESPONSE_BODY" | jq -r '.session_token // empty')

echo "Response fields:"
echo "  user_id:       ${USER_ID:-(missing)}"
echo "  email:         ${EMAIL:-(missing)}"
echo "  api_key:       ${API_KEY:0:20}... (${#API_KEY} chars)"
echo "  tier:          ${TIER:-(missing)}"
echo "  session_token: ${SESSION_TOKEN:0:20}... (${#SESSION_TOKEN} chars)"
echo ""

if [ -z "$USER_ID" ]; then
  echo "❌ FAIL: user_id missing from response"
  exit 1
fi

if [ -z "$EMAIL" ]; then
  echo "❌ FAIL: email missing from response (FIX #2 NOT APPLIED)"
  exit 1
else
  if [ "$EMAIL" = "$TEST_EMAIL" ]; then
    echo "✅ PASS: email field present and matches ($EMAIL)"
  else
    echo "❌ FAIL: email present but doesn't match (got: $EMAIL, expected: $TEST_EMAIL)"
    exit 1
  fi
fi

if [ -z "$API_KEY" ]; then
  echo "❌ FAIL: api_key missing from response"
  exit 1
elif [[ ! "$API_KEY" =~ ^tz_ ]]; then
  echo "❌ FAIL: api_key doesn't start with 'tz_' prefix (got: ${API_KEY:0:10}...)"
  exit 1
else
  echo "✅ PASS: api_key present with correct format (${#API_KEY} chars)"
fi

if [ -z "$TIER" ]; then
  echo "❌ FAIL: tier missing from response"
  exit 1
elif [ "$TIER" != "free" ]; then
  echo "❌ FAIL: tier should be 'free' for new signups (got: $TIER)"
  exit 1
else
  echo "✅ PASS: tier is 'free' (default)"
fi

if [ -z "$SESSION_TOKEN" ]; then
  echo "❌ FAIL: session_token missing from response"
  exit 1
elif [ ${#SESSION_TOKEN} -ne 64 ]; then
  echo "❌ FAIL: session_token should be 64 chars (got: ${#SESSION_TOKEN})"
  exit 1
else
  echo "✅ PASS: session_token present (64 chars)"
fi

echo ""

# Test 3: Verify duplicate email returns 400 BAD_REQUEST (FIX #3)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 3: Verify duplicate email error (HTTP 400 + correct message)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

DUPLICATE_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/signup" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

DUP_HTTP_CODE=$(echo "$DUPLICATE_RESPONSE" | tail -n1)
DUP_BODY=$(echo "$DUPLICATE_RESPONSE" | sed '$d')

if [ "$DUP_HTTP_CODE" = "400" ]; then
  echo "✅ PASS: Duplicate email returns HTTP 400 (was 409 before fix)"
else
  echo "❌ FAIL: Duplicate email returned HTTP $DUP_HTTP_CODE (expected 400)"
  echo "Response: $DUP_BODY"
  exit 1
fi

if echo "$DUP_BODY" | grep -qi "already exists"; then
  echo "✅ PASS: Error message contains 'already exists' (frontend expects this)"
else
  echo "⚠️  WARNING: Error message doesn't contain 'already exists'"
  echo "Message: $DUP_BODY"
  echo "(Frontend looks for 'already exists' to show user-friendly error)"
fi

echo ""

# Test 4: Verify /v1/me endpoint works with session token
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 4: Verify /v1/me endpoint (dashboard data loading)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ME_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_URL/me" \
  -H "X-Session-Token: $SESSION_TOKEN")

ME_HTTP_CODE=$(echo "$ME_RESPONSE" | tail -n1)
ME_BODY=$(echo "$ME_RESPONSE" | sed '$d')

if [ "$ME_HTTP_CODE" = "200" ]; then
  echo "✅ PASS: /v1/me endpoint accessible (HTTP 200)"
else
  echo "❌ FAIL: /v1/me returned HTTP $ME_HTTP_CODE"
  echo "Response: $ME_BODY"
  exit 1
fi

ME_EMAIL=$(echo "$ME_BODY" | jq -r '.email // empty')
ME_USER_ID=$(echo "$ME_BODY" | jq -r '.user_id // empty')
ME_API_KEY=$(echo "$ME_BODY" | jq -r '.api_key // empty')
ME_TIER=$(echo "$ME_BODY" | jq -r '.tier // empty')
ME_USED=$(echo "$ME_BODY" | jq -r '.used // empty')

echo "Dashboard data:"
echo "  email:    $ME_EMAIL"
echo "  user_id:  $ME_USER_ID"
echo "  api_key:  ${ME_API_KEY:0:20}..."
echo "  tier:     $ME_TIER"
echo "  used:     $ME_USED"
echo ""

if [ "$ME_EMAIL" != "$TEST_EMAIL" ]; then
  echo "❌ FAIL: /v1/me email doesn't match signup email"
  exit 1
fi

if [ "$ME_USER_ID" != "$USER_ID" ]; then
  echo "❌ FAIL: /v1/me user_id doesn't match signup user_id"
  exit 1
fi

if [ "$ME_API_KEY" != "$API_KEY" ]; then
  echo "❌ FAIL: /v1/me api_key doesn't match signup api_key"
  exit 1
fi

echo "✅ PASS: Dashboard data consistent with signup response"
echo ""

# Test 5: Test backward compatibility (/v1/auth/signup still works)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 5: Verify backward compatibility (/v1/auth/signup)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

TEST_EMAIL_2="test_compat_$(date +%s)@example.com"
COMPAT_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/auth/signup" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL_2\",\"password\":\"$TEST_PASSWORD\"}")

COMPAT_HTTP_CODE=$(echo "$COMPAT_RESPONSE" | tail -n1)

if [ "$COMPAT_HTTP_CODE" = "200" ]; then
  echo "✅ PASS: /v1/auth/signup endpoint still works (backward compatibility)"
else
  echo "❌ FAIL: /v1/auth/signup returned HTTP $COMPAT_HTTP_CODE"
  exit 1
fi

echo ""

# Final summary
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  ✅ ALL TESTS PASSED                                      ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Summary:"
echo "  ✅ Fix #1: /v1/signup endpoint exists"
echo "  ✅ Fix #2: Email field included in signup response"
echo "  ✅ Fix #3: Duplicate email returns HTTP 400"
echo "  ✅ Bonus:  /v1/me endpoint works correctly"
echo "  ✅ Bonus:  Backward compatibility maintained"
echo ""
echo "🎉 Frontend-backend integration is working correctly!"
echo ""
echo "Test user created:"
echo "  Email:      $TEST_EMAIL"
echo "  Password:   $TEST_PASSWORD"
echo "  User ID:    $USER_ID"
echo "  API Key:    ${API_KEY:0:20}..."
echo "  Session:    ${SESSION_TOKEN:0:20}..."
echo ""
echo "Next steps:"
echo "  1. Test frontend signup flow in browser"
echo "  2. Verify Stripe checkout redirect (Pro/Scale tiers)"
echo "  3. Deploy to production"
echo ""

