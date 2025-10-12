# Testing Guide - Frontend-Backend Integration
**Date:** October 12, 2025  
**Version:** Post-Fix Verification

---

## Quick Start

### Option 1: Automated Test Script (Recommended)
```bash
# Start the API server locally
cargo run --bin tinyzkp_api

# In another terminal, run the test script
./scripts/test_signup_flow.sh

# Or test against production
API_URL=https://api.tinyzkp.com ./scripts/test_signup_flow.sh
```

### Option 2: Manual Testing with curl

#### Test 1: Verify /v1/signup endpoint
```bash
curl -X POST http://localhost:3030/v1/signup \
  -H "Content-Type: application/json" \
  -d '{
    "email": "test@example.com",
    "password": "SecurePass123"
  }'
```

**Expected Response (HTTP 200):**
```json
{
  "user_id": "eaa0059ef4ec747c7784f3bce48cbc06",
  "email": "test@example.com",
  "api_key": "tz_ca4c36a9f6e9b08f270375c094cd43bf...",
  "tier": "free",
  "session_token": "b5986162e988f9de366f1c60eb1a5276f1ce6b..."
}
```

**✅ Verify:**
- Response includes `email` field
- `api_key` starts with `tz_`
- `session_token` is 64 characters
- `tier` is `"free"`

---

#### Test 2: Verify duplicate email error
```bash
# Use same email again
curl -X POST http://localhost:3030/v1/signup \
  -H "Content-Type: application/json" \
  -d '{
    "email": "test@example.com",
    "password": "SecurePass123"
  }'
```

**Expected Response (HTTP 400):**
```
email already exists
```

**✅ Verify:**
- HTTP status is **400** (not 409)
- Error message contains "already exists"

---

#### Test 3: Verify /v1/me endpoint
```bash
# Use session token from signup response
curl -X GET http://localhost:3030/v1/me \
  -H "X-Session-Token: YOUR_SESSION_TOKEN_HERE"
```

**Expected Response (HTTP 200):**
```json
{
  "user_id": "eaa0059ef4ec747c7784f3bce48cbc06",
  "email": "test@example.com",
  "api_key": "tz_ca4c36a9f6e9b08f...",
  "tier": "free",
  "month": "2025-10",
  "used": 0,
  "caps": {"free": 100, "pro": 10000, "scale": 100000},
  "limits": {"free_max_rows": 1024, "pro_max_rows": 4096, "scale_max_rows": 16384}
}
```

**✅ Verify:**
- All user data matches signup response
- `month` is in `YYYY-MM` format
- `used` is 0 for new accounts

---

## Frontend Testing

### Prerequisites
1. Backend API running (locally or production)
2. Frontend deployed (Lovable AI build)
3. Environment variables configured:
   - `VITE_API_BASE_URL=http://localhost:3030/v1` (or production URL)

### Test Scenarios

#### Scenario 1: Free Tier Signup
1. **Navigate** to landing page
2. **Click** "Free API Key" button
3. **Enter** email and password:
   - Email: `yourname@example.com`
   - Password: `SecurePass123` (must have uppercase, lowercase, number)
4. **Click** "Sign Up"
5. **Expected behavior:**
   - Modal closes
   - Redirected to `/dashboard`
   - Dashboard shows:
     - ✅ Email address
     - ✅ API key (starts with `tz_`)
     - ✅ Tier: Free
     - ✅ Usage: 0/100
6. **Verify localStorage:**
   - Open browser DevTools → Application → Local Storage
   - Check keys: `tinyzkp_user` and `session_token`

**✅ Pass Criteria:**
- No console errors
- Dashboard loads within 2 seconds
- All user data displayed correctly

---

#### Scenario 2: Duplicate Email Error
1. **Try signing up** with same email again
2. **Expected behavior:**
   - Error message: *"This email is already registered. Please sign in instead."*
   - Modal stays open
   - No redirect

**✅ Pass Criteria:**
- User-friendly error message shown
- No generic "Signup failed (400)" error

---

#### Scenario 3: Pro Tier Signup (Stripe Integration)
1. **Navigate** to pricing section
2. **Click** "Get Pro" button
3. **Enter** email and password (new email)
4. **Click** "Sign Up"
5. **Expected behavior:**
   - Account created
   - Brief loading state (500ms)
   - Redirected to **Stripe Checkout**
   - Stripe URL: `https://checkout.stripe.com/c/pay/...`

**✅ Pass Criteria:**
- No errors before Stripe redirect
- Stripe checkout session loads
- Session metadata includes API key

---

#### Scenario 4: Password Validation
1. **Try weak passwords:**
   - `abc` → "Password must be at least 8 characters"
   - `abcdefgh` → "Must contain at least one uppercase letter"
   - `Abcdefgh` → "Must contain at least one number"
2. **Expected behavior:**
   - Frontend validates **before** sending request
   - Error messages shown instantly
   - Submit button disabled until valid

**✅ Pass Criteria:**
- All validation errors caught client-side
- No 400 errors from backend for these cases

---

#### Scenario 5: Session Persistence
1. **Sign up** successfully
2. **Refresh** the page (F5)
3. **Expected behavior:**
   - Still logged in
   - Dashboard loads immediately
   - No redirect to login

**✅ Pass Criteria:**
- Session persists across page refreshes
- localStorage contains valid session token

---

## Backend Testing (Unit Tests)

### Test Response Schema
```bash
# Create a test file: tests/signup_response_test.rs

#[tokio::test]
async fn test_signup_response_includes_email() {
    let response = signup_test_user().await;
    
    assert!(response.contains_key("email"));
    assert!(response.contains_key("user_id"));
    assert!(response.contains_key("api_key"));
    assert!(response.contains_key("tier"));
    assert!(response.contains_key("session_token"));
    
    assert_eq!(response["tier"], "free");
}

#[tokio::test]
async fn test_duplicate_email_returns_400() {
    let email = "duplicate@test.com";
    
    // First signup
    let response1 = signup(email, "Pass123").await;
    assert_eq!(response1.status(), 200);
    
    // Second signup (duplicate)
    let response2 = signup(email, "Pass123").await;
    assert_eq!(response2.status(), 400);
    
    let error_text = response2.text().await.unwrap();
    assert!(error_text.contains("already exists"));
}
```

---

## Production Testing Checklist

### Pre-Deployment
- [ ] All automated tests pass (`./scripts/test_signup_flow.sh`)
- [ ] Backend builds without errors (`cargo build --release`)
- [ ] Environment variables configured on Railway:
  - [ ] `UPSTASH_REDIS_URL`
  - [ ] `STRIPE_SECRET_KEY`
  - [ ] `STRIPE_PRICE_PRO`
  - [ ] `STRIPE_PRICE_SCALE`
  - [ ] `STRIPE_WEBHOOK_SECRET`
  - [ ] `CORS_ALLOWED_ORIGINS`

### Post-Deployment
- [ ] API health check: `curl https://api.tinyzkp.com/v1/health`
- [ ] Test signup with real email (Free tier)
- [ ] Test duplicate email error
- [ ] Test login with new account
- [ ] Test dashboard data loading
- [ ] Test Stripe checkout (Pro tier) - use test mode
- [ ] Check Railway logs for errors

---

## Performance Testing

### Load Test (Signup Endpoint)
```bash
# Install hey: https://github.com/rakyll/hey
brew install hey

# Test 100 concurrent signups
hey -n 100 -c 10 -m POST \
  -H "Content-Type: application/json" \
  -d '{"email":"load_test_USER_ID@example.com","password":"SecurePass123"}' \
  https://api.tinyzkp.com/v1/signup
```

**Expected Results:**
- 95% of requests < 500ms
- Rate limiting kicks in after 30 requests in 3 seconds (HTTP 429)
- No 500 errors

---

## Security Testing

### Test Rate Limiting
```bash
# Send 35 requests rapidly (should hit 429 after 30)
for i in {1..35}; do
  curl -s -o /dev/null -w "%{http_code}\n" \
    -X POST http://localhost:3030/v1/signup \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"test$i@example.com\",\"password\":\"Pass123\"}"
  sleep 0.05
done
```

**Expected:** First 30 return 200, remaining return 429

### Test Session Token Expiry
```bash
# Get current timestamp + 31 days
FUTURE=$(date -v+31d +%s)

# Try to use session token after 30 days (will be expired)
# Manual test: Wait 30 days or modify Redis TTL for testing
```

---

## Common Issues & Solutions

### Issue: "email missing from response"
**Cause:** Old backend version deployed  
**Solution:** 
```bash
git pull origin main
cargo build --release
# Redeploy to Railway
```

### Issue: "404 on /v1/signup"
**Cause:** Old API route (`/v1/auth/signup` only)  
**Solution:** Deploy latest backend with route alias

### Issue: "HTTP 409 instead of 400 for duplicate email"
**Cause:** Old error handling code  
**Solution:** Deploy latest backend

### Issue: "CORS error in browser console"
**Cause:** Frontend origin not in `CORS_ALLOWED_ORIGINS`  
**Solution:** Add frontend URL to environment variable:
```bash
CORS_ALLOWED_ORIGINS=https://tinyzkp.com,https://app.tinyzkp.com
```

### Issue: "Session token rejected by /v1/me"
**Cause:** Header name mismatch  
**Solution:** Ensure frontend sends `X-Session-Token` (not `Authorization: Bearer`)

---

## Monitoring & Analytics

### Key Metrics to Track
1. **Signup Success Rate**
   - Target: >95%
   - Alert if: <90%

2. **Average Signup Time**
   - Target: <2 seconds
   - Alert if: >5 seconds

3. **Duplicate Email Attempts**
   - Track: Count per day
   - Purpose: Detect brute force attempts

4. **Stripe Checkout Conversion**
   - Track: Pro/Scale signups → completed payments
   - Target: >80% conversion

### Railway Logs to Monitor
```bash
# Filter for signup attempts
railway logs --filter "auth_signup"

# Filter for errors
railway logs --filter "ERROR"

# Filter for Stripe webhooks
railway logs --filter "stripe/webhook"
```

---

## Rollback Plan

If critical issues arise after deployment:

### Quick Rollback (GitHub)
```bash
# Revert to previous commit
git revert HEAD
git push origin main

# Or reset to previous commit
git reset --hard eac19f2  # Previous commit before fixes
git push --force origin main  # ⚠️ Use with caution
```

### Emergency Hotfix
```bash
# Create hotfix branch
git checkout -b hotfix/signup-issue

# Make fixes
# ...

# Deploy
git push origin hotfix/signup-issue
# Deploy from Railway dashboard
```

---

## Success Criteria

✅ **All tests pass:**
- Automated script: 5/5 tests
- Manual curl tests: 3/3 tests
- Frontend scenarios: 5/5 tests

✅ **No regressions:**
- Existing endpoints still work
- Backward compatibility maintained
- No performance degradation

✅ **Production metrics:**
- First 10 signups successful
- Zero 500 errors
- Average response time <500ms

---

## Next Steps After Successful Testing

1. **Monitor first 24 hours:**
   - Check Railway logs every hour
   - Track signup success rate
   - Watch for error patterns

2. **Marketing launch:**
   - Enable signup buttons on landing page
   - Announce on social media
   - Send email to waitlist

3. **User onboarding:**
   - Send welcome email with API docs
   - Provide example code snippets
   - Link to dashboard

4. **Iterate based on feedback:**
   - Collect user feedback
   - Monitor support requests
   - Plan UX improvements

---

## Support Resources

- **Backend Code:** `src/bin/tinyzkp_api.rs`
- **Security Audit:** `SECURITY_AUDIT_DATA_STORAGE.md`
- **Verification Report:** `FRONTEND_BACKEND_VERIFICATION.md`
- **Deployment Docs:** `DEPLOYMENT.md`

For questions or issues, check Railway logs first, then review relevant documentation above.

