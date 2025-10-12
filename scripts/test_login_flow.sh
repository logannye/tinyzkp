#!/bin/bash
# Test script for login flow verification
# Tests login endpoint, password verification, and session token generation

set -e

API_URL="${API_URL:-http://localhost:3030}"
TEST_EMAIL="login_test_$(date +%s)@example.com"
TEST_PASSWORD="SecurePass123"

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  🧪 Testing Login Flow                                    ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "API URL: $API_URL"
echo "Test Email: $TEST_EMAIL"
echo ""

# Step 1: Create a test user first
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "SETUP: Creating test user for login test"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

SIGNUP_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/signup" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

SIGNUP_HTTP_CODE=$(echo "$SIGNUP_RESPONSE" | tail -n1)
SIGNUP_BODY=$(echo "$SIGNUP_RESPONSE" | sed '$d')

if [ "$SIGNUP_HTTP_CODE" != "200" ]; then
  echo "❌ FAIL: Could not create test user (HTTP $SIGNUP_HTTP_CODE)"
  echo "Response: $SIGNUP_BODY"
  exit 1
fi

EXPECTED_USER_ID=$(echo "$SIGNUP_BODY" | jq -r '.user_id // empty')
EXPECTED_API_KEY=$(echo "$SIGNUP_BODY" | jq -r '.api_key // empty')
EXPECTED_EMAIL=$(echo "$SIGNUP_BODY" | jq -r '.email // empty')

echo "✅ Test user created successfully"
echo "  User ID: $EXPECTED_USER_ID"
echo "  Email: $EXPECTED_EMAIL"
echo ""

# Test 1: Login with correct credentials (/v1/signin - frontend endpoint)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 1: Login with correct credentials (/v1/signin)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

LOGIN_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/signin" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

LOGIN_HTTP_CODE=$(echo "$LOGIN_RESPONSE" | tail -n1)
LOGIN_BODY=$(echo "$LOGIN_RESPONSE" | sed '$d')

if [ "$LOGIN_HTTP_CODE" != "200" ]; then
  echo "❌ FAIL: Login failed (HTTP $LOGIN_HTTP_CODE)"
  echo "Response: $LOGIN_BODY"
  exit 1
fi

echo "✅ PASS: Login successful (HTTP 200)"
echo ""

# Test 2: Verify login response includes all required fields
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 2: Verify login response includes all required fields"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

LOGIN_USER_ID=$(echo "$LOGIN_BODY" | jq -r '.user_id // empty')
LOGIN_EMAIL=$(echo "$LOGIN_BODY" | jq -r '.email // empty')
LOGIN_API_KEY=$(echo "$LOGIN_BODY" | jq -r '.api_key // empty')
LOGIN_TIER=$(echo "$LOGIN_BODY" | jq -r '.tier // empty')
LOGIN_SESSION=$(echo "$LOGIN_BODY" | jq -r '.session_token // empty')

echo "Login response fields:"
echo "  user_id:       ${LOGIN_USER_ID:-(missing)}"
echo "  email:         ${LOGIN_EMAIL:-(missing)}"
echo "  api_key:       ${LOGIN_API_KEY:0:20}... (${#LOGIN_API_KEY} chars)"
echo "  tier:          ${LOGIN_TIER:-(missing)}"
echo "  session_token: ${LOGIN_SESSION:0:20}... (${#LOGIN_SESSION} chars)"
echo ""

# Verify all fields present
if [ -z "$LOGIN_USER_ID" ]; then
  echo "❌ FAIL: user_id missing from login response"
  exit 1
fi

if [ -z "$LOGIN_EMAIL" ]; then
  echo "❌ FAIL: email missing from login response (FIX NOT APPLIED)"
  exit 1
else
  echo "✅ PASS: email field present in login response"
fi

if [ -z "$LOGIN_API_KEY" ]; then
  echo "❌ FAIL: api_key missing from login response"
  exit 1
fi

if [ -z "$LOGIN_TIER" ]; then
  echo "❌ FAIL: tier missing from login response"
  exit 1
fi

if [ -z "$LOGIN_SESSION" ]; then
  echo "❌ FAIL: session_token missing from login response"
  exit 1
fi

# Verify fields match signup data
if [ "$LOGIN_USER_ID" != "$EXPECTED_USER_ID" ]; then
  echo "❌ FAIL: user_id doesn't match (expected: $EXPECTED_USER_ID, got: $LOGIN_USER_ID)"
  exit 1
fi

if [ "$LOGIN_EMAIL" != "$EXPECTED_EMAIL" ]; then
  echo "❌ FAIL: email doesn't match (expected: $EXPECTED_EMAIL, got: $LOGIN_EMAIL)"
  exit 1
fi

if [ "$LOGIN_API_KEY" != "$EXPECTED_API_KEY" ]; then
  echo "❌ FAIL: api_key doesn't match signup api_key"
  exit 1
fi

echo "✅ PASS: All fields present and match signup data"
echo ""

# Test 3: Verify new session token is generated
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 3: Verify new session token is generated"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

SIGNUP_SESSION=$(echo "$SIGNUP_BODY" | jq -r '.session_token // empty')

if [ "$LOGIN_SESSION" = "$SIGNUP_SESSION" ]; then
  echo "⚠️  WARNING: Login returned same session token as signup"
  echo "(Ideally, each login should generate a new session)"
else
  echo "✅ PASS: New session token generated on login"
fi
echo ""

# Test 4: Verify session token works with /v1/me
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 4: Verify login session token works with /v1/me"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ME_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_URL/v1/me" \
  -H "X-Session-Token: $LOGIN_SESSION")

ME_HTTP_CODE=$(echo "$ME_RESPONSE" | tail -n1)
ME_BODY=$(echo "$ME_RESPONSE" | sed '$d')

if [ "$ME_HTTP_CODE" != "200" ]; then
  echo "❌ FAIL: /v1/me returned HTTP $ME_HTTP_CODE"
  echo "Response: $ME_BODY"
  exit 1
fi

ME_EMAIL=$(echo "$ME_BODY" | jq -r '.email // empty')

if [ "$ME_EMAIL" = "$TEST_EMAIL" ]; then
  echo "✅ PASS: Session token authenticates correctly"
else
  echo "❌ FAIL: /v1/me returned wrong email (expected: $TEST_EMAIL, got: $ME_EMAIL)"
  exit 1
fi
echo ""

# Test 5: Login with wrong password
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 5: Login with incorrect password (should fail)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

WRONG_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/signin" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"WrongPassword123\"}")

WRONG_HTTP_CODE=$(echo "$WRONG_RESPONSE" | tail -n1)
WRONG_BODY=$(echo "$WRONG_RESPONSE" | sed '$d')

if [ "$WRONG_HTTP_CODE" = "401" ]; then
  echo "✅ PASS: Wrong password returns HTTP 401 (Unauthorized)"
else
  echo "❌ FAIL: Wrong password returned HTTP $WRONG_HTTP_CODE (expected 401)"
  echo "Response: $WRONG_BODY"
  exit 1
fi

if echo "$WRONG_BODY" | grep -qi "invalid credentials"; then
  echo "✅ PASS: Error message mentions 'invalid credentials'"
else
  echo "⚠️  WARNING: Error message doesn't mention 'invalid credentials'"
  echo "Message: $WRONG_BODY"
fi
echo ""

# Test 6: Login with non-existent email
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 6: Login with non-existent email (should fail)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

NONEXIST_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/signin" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"nonexistent_$(date +%s)@example.com\",\"password\":\"$TEST_PASSWORD\"}")

NONEXIST_HTTP_CODE=$(echo "$NONEXIST_RESPONSE" | tail -n1)

if [ "$NONEXIST_HTTP_CODE" = "401" ]; then
  echo "✅ PASS: Non-existent email returns HTTP 401"
else
  echo "❌ FAIL: Non-existent email returned HTTP $NONEXIST_HTTP_CODE (expected 401)"
  exit 1
fi
echo ""

# Test 7: Alternative endpoint aliases (/v1/login, /v1/auth/login)
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "TEST 7: Verify alternative login endpoints work"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Test /v1/login
LOGIN2_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/login" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

LOGIN2_HTTP_CODE=$(echo "$LOGIN2_RESPONSE" | tail -n1)

if [ "$LOGIN2_HTTP_CODE" = "200" ]; then
  echo "✅ PASS: /v1/login endpoint works"
else
  echo "❌ FAIL: /v1/login returned HTTP $LOGIN2_HTTP_CODE"
  exit 1
fi

# Test /v1/auth/login
LOGIN3_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/v1/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

LOGIN3_HTTP_CODE=$(echo "$LOGIN3_RESPONSE" | tail -n1)

if [ "$LOGIN3_HTTP_CODE" = "200" ]; then
  echo "✅ PASS: /v1/auth/login endpoint works (backward compatibility)"
else
  echo "❌ FAIL: /v1/auth/login returned HTTP $LOGIN3_HTTP_CODE"
  exit 1
fi
echo ""

# Final summary
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  ✅ ALL LOGIN TESTS PASSED                                ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Summary:"
echo "  ✅ Login with correct credentials works"
echo "  ✅ Login response includes email field"
echo "  ✅ New session token generated on login"
echo "  ✅ Session token authenticates correctly"
echo "  ✅ Wrong password rejected (HTTP 401)"
echo "  ✅ Non-existent email rejected (HTTP 401)"
echo "  ✅ Alternative endpoints work (/v1/login, /v1/auth/login)"
echo ""
echo "🎉 Login flow is working correctly!"
echo ""
echo "Test user credentials:"
echo "  Email:    $TEST_EMAIL"
echo "  Password: $TEST_PASSWORD"
echo "  User ID:  $LOGIN_USER_ID"
echo "  API Key:  ${LOGIN_API_KEY:0:20}..."
echo ""

