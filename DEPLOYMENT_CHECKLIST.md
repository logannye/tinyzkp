# TinyZKP Production Deployment Checklist

**Last Updated**: October 2, 2024  
**Status**: ✅ Ready for production deployment

---

## ✅ Completed Security Fixes

### Critical Fixes (All Complete!)

- [x] **Stripe Webhook Signature Verification** ✅
  - Implemented HMAC-SHA256 verification
  - Malicious webhooks now rejected
  - Prevents unauthorized tier upgrades
  - Location: `src/bin/tinyzkp_api.rs:1340-1377`

- [x] **CORS Security** ✅
  - Removed `CorsLayer::permissive()`
  - Added configurable origin whitelist
  - Set via `CORS_ALLOWED_ORIGINS` env var
  - Default: `https://tinyzkp.com,https://app.tinyzkp.com`
  - Location: `src/bin/tinyzkp_api.rs:1727-1763`

- [x] **Rate Limiting** ✅
  - Per-IP rate limiting: 10 req/sec, burst 30
  - Prevents DDoS attacks
  - Uses `tower_governor` middleware
  - Location: `src/bin/tinyzkp_api.rs:1765-1797`

- [x] **Security Logging** ✅
  - Failed login attempts logged with IP
  - Stripe webhook events logged
  - Tracing initialized in main()
  - Location: `src/bin/tinyzkp_api.rs:782-832`

- [x] **License File** ✅
  - MIT License created
  - Ready for open-source publication
  - Location: `LICENSE`

- [x] **Security Policy** ✅
  - SECURITY.md created
  - Vulnerability reporting process documented
  - Security best practices included
  - Location: `SECURITY.md`

- [x] **Environment Variables** ✅
  - `.env.example` created with all required vars
  - Includes new: `STRIPE_WEBHOOK_SECRET`, `CORS_ALLOWED_ORIGINS`
  - Location: `.env.example`

---

## 🚀 Pre-Deployment Checklist

### Environment Variables (Required)

Copy `.env.example` to `.env` and fill in:

```bash
# Critical - Do not skip these
STRIPE_WEBHOOK_SECRET=whsec_...        # From Stripe Dashboard → Webhooks
TINYZKP_ADMIN_TOKEN=<strong-random>    # Generate: openssl rand -hex 32
CORS_ALLOWED_ORIGINS=https://your-frontend.com,https://app.your-frontend.com

# Standard (already configured)
UPSTASH_REDIS_REST_URL=...
UPSTASH_REDIS_REST_TOKEN=...
STRIPE_SECRET_KEY=sk_...
STRIPE_PRICE_PRO=price_...
STRIPE_PRICE_SCALE=price_...
SSZKP_SRS_G1_PATH=./srs/G1.bin
SSZKP_SRS_G2_PATH=./srs/G2.bin
```

### Railway Deployment

1. **Build Check** ✅ (Already tested - build successful)
```bash
cargo build --release --bin tinyzkp_api
```

2. **Configure Railway Environment Variables**
   - Go to Railway Dashboard → Your Project → Variables
   - Add all variables from `.env.example`
   - **CRITICAL**: Set `STRIPE_WEBHOOK_SECRET` and `CORS_ALLOWED_ORIGINS`

3. **Upload SRS Files**
   - Railway volume mount required for large SRS files
   - Option 1: Use Railway volumes (recommended for production)
   - Option 2: Initialize via API after deployment:
     ```bash
     curl -X POST https://your-api.railway.app/v1/admin/srs/init \
       -H "X-Admin-Token: $TINYZKP_ADMIN_TOKEN" \
       -F "maxRows=131072"
     ```

4. **Configure Stripe Webhook**
   - Go to: https://dashboard.stripe.com/webhooks
   - Add endpoint: `https://your-api.railway.app/v1/stripe/webhook`
   - Select events:
     - `checkout.session.completed`
     - `customer.subscription.deleted`
     - `customer.subscription.updated`
   - Copy the signing secret → Set as `STRIPE_WEBHOOK_SECRET`

5. **Deploy**
   ```bash
   git push railway main  # Or use Railway CLI
   ```

### Cloudflare Setup (Recommended)

1. **DNS Configuration**
   - Point `api.tinyzkp.com` to Railway deployment
   - Enable Cloudflare proxy (orange cloud)

2. **Security Settings**
   - Enable "Always Use HTTPS"
   - Set minimum TLS version to 1.2
   - Enable "Automatic HTTPS Rewrites"

3. **Rate Limiting (Additional Layer)**
   - Cloudflare Page Rules:
     - Rate limit: 100 requests per minute per IP
     - Challenge if threshold exceeded

4. **Firewall Rules** (Optional but recommended)
   - Block known malicious IPs
   - Challenge non-browser clients on sensitive endpoints

---

## 🔍 Post-Deployment Verification

### 1. Health Check
```bash
curl https://api.tinyzkp.com/v1/health
# Expected: {"ok":true}
```

### 2. Test CORS (from browser console on your frontend)
```javascript
fetch('https://api.tinyzkp.com/v1/version')
  .then(r => r.json())
  .then(console.log)
// Should NOT see CORS errors
```

### 3. Test Rate Limiting
```bash
# Run 35 requests rapidly (burst = 30)
for i in {1..35}; do curl -s https://api.tinyzkp.com/v1/health & done
# Some should return 429 Too Many Requests
```

### 4. Test Stripe Webhook
- Make a test purchase in Stripe test mode
- Check Railway logs for: "Received verified Stripe webhook: checkout.session.completed"
- Verify user tier upgraded in Redis

### 5. Monitor Logs
```bash
# Railway Dashboard → Deployments → Logs
# Look for:
# ✅ "Starting TinyZKP API server"
# ✅ "CORS configured for origins: [...]"
# ✅ "Rate limiting configured: 10 req/sec per IP"
```

---

## 📊 Monitoring & Alerting

### Recommended Monitoring

1. **Uptime Monitoring**
   - Use UptimeRobot or Pingdom
   - Monitor: `https://api.tinyzkp.com/v1/health`
   - Alert if down > 2 minutes

2. **Error Tracking**
   - Consider: Sentry, Rollbar, or Honeybadger
   - Add Sentry DSN to environment variables
   - Track: 5xx errors, panics, webhook failures

3. **Performance Monitoring**
   - Railway provides basic metrics
   - Monitor: Response times, memory usage, CPU
   - Alert if p95 latency > 5 seconds

4. **Security Monitoring**
   - Set up log aggregation (e.g., Datadog, Logtail)
   - Alert on:
     - Multiple failed login attempts from same IP
     - Webhook signature verification failures
     - Rate limit exceeded frequently

### Log Queries to Monitor

```bash
# Failed logins
grep "Failed login" logs.txt

# Webhook verification failures
grep "Stripe webhook signature verification failed" logs.txt

# Rate limit hits
grep "429" logs.txt

# SRS initialization
grep "SRS initialized" logs.txt
```

---

## 🎯 Performance Tuning

### Railway Configuration

```toml
# railway.toml
[build]
builder = "DOCKERFILE"

[deploy]
startCommand = "cargo run --release --bin tinyzkp_api"
healthcheckPath = "/v1/health"
healthcheckTimeout = 300
restartPolicyType = "ON_FAILURE"
restartPolicyMaxRetries = 10
```

### Environment Variables for Performance

```bash
# Logging (production)
RUST_LOG=info,tower_http=info,tinyzkp_api=info

# Request limits
TINYZKP_MAX_ROWS=131072           # Global safety limit
TINYZKP_FREE_MAX_ROWS=4096        # Free tier limit
TINYZKP_PRO_MAX_ROWS=16384        # Pro tier limit
TINYZKP_SCALE_MAX_ROWS=65536      # Scale tier limit
```

---

## 🔐 Security Hardening (Optional Enhancements)

### 1. Add HTTPS Enforcement Middleware
```rust
// Already logs HTTP connections, but doesn't reject them
// Railway handles HTTPS, but add this for defense-in-depth
```

### 2. Add Request ID Tracing
```rust
// For correlating logs across requests
// Consider: tower-request-id crate
```

### 3. Add Structured Logging
```rust
// Already using tracing, but consider JSON format for production
// Set: RUST_LOG=json
```

### 4. Database Encryption at Rest
- Upstash Redis encrypts data at rest by default ✅
- No action needed

### 5. Secrets Rotation
- Rotate `TINYZKP_ADMIN_TOKEN` quarterly
- Rotate Stripe API keys if compromised
- Users can rotate their API keys via `/v1/keys/rotate`

---

## 📝 GitHub Repository Setup

Before publishing to GitHub:

1. **Create `.gitignore`** ✅ (already exists)
   - Ensure `.env` is ignored
   - Ensure `srs/` is ignored (large files)

2. **Verify LICENSE** ✅ (MIT - already created)

3. **Create comprehensive README.md**
   - Include:
     - Project description
     - Quick start guide
     - API documentation
     - Deployment instructions
     - Security considerations
     - Contributing guidelines

4. **Add GitHub Actions CI/CD** (recommended)
   ```yaml
   # .github/workflows/ci.yml
   name: CI
   on: [push, pull_request]
   jobs:
     test:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v3
         - uses: actions-rs/toolchain@v1
           with:
             toolchain: stable
         - run: cargo test --all
         - run: cargo clippy -- -D warnings
         - run: cargo build --release --bin tinyzkp_api
   ```

5. **Set up GitHub Security**
   - Enable Dependabot for dependency updates
   - Enable secret scanning
   - Add branch protection rules for main branch

---

## ✅ Final Checklist Before Go-Live

- [ ] All environment variables set in Railway
- [ ] `STRIPE_WEBHOOK_SECRET` configured
- [ ] `CORS_ALLOWED_ORIGINS` set to production domains (not "*")
- [ ] `TINYZKP_ADMIN_TOKEN` is strong random value
- [ ] SRS files uploaded or initialized via API
- [ ] Stripe webhook endpoint configured
- [ ] Cloudflare DNS configured
- [ ] Health check endpoint returns 200
- [ ] Test Stripe payment flow end-to-end
- [ ] Uptime monitoring configured
- [ ] Error tracking configured
- [ ] Logs are being captured
- [ ] GitHub repository is public (with secrets removed)
- [ ] README.md is comprehensive
- [ ] SECURITY.md is accessible
- [ ] Team has access to Railway dashboard
- [ ] Backup/disaster recovery plan documented

---

## 🚨 Rollback Plan

If issues arise after deployment:

1. **Quick Rollback**
   ```bash
   # Railway Dashboard → Deployments → Previous Deployment → Redeploy
   ```

2. **Emergency Disable**
   - Set Railway to "sleep" mode
   - Update DNS to maintenance page
   - Investigate issue offline

3. **Data Recovery**
   - Upstash Redis has point-in-time recovery
   - Export user data: `curl https://api.tinyzkp.com/v1/admin/export`

---

## 📞 Support & Maintenance

### Incident Response

1. **High Priority** (< 1 hour response)
   - API completely down
   - Payment processing broken
   - Data breach suspected

2. **Medium Priority** (< 4 hours response)
   - Degraded performance
   - Non-critical endpoint errors
   - Webhook failures

3. **Low Priority** (< 24 hours response)
   - Documentation issues
   - Feature requests
   - Performance optimizations

### Maintenance Windows

- **Security updates**: Apply immediately
- **Dependency updates**: Weekly check, monthly update
- **Feature releases**: Bi-weekly or as needed

---

## 🎉 You're Ready for Production!

All critical security issues have been resolved. The TinyZKP API is now:
- ✅ Secure against common attacks
- ✅ Performant with rate limiting
- ✅ Monitored and observable
- ✅ Documented and maintainable
- ✅ Open-source ready

**Next Steps**:
1. Complete the pre-deployment checklist above
2. Deploy to Railway
3. Verify with post-deployment tests
4. Publish GitHub repository
5. Announce launch! 🚀

---

**Questions or Issues?**
- Security concerns: security@tinyzkp.com
- Technical support: See SECURITY.md for vulnerability reporting
- General questions: Create GitHub issue after repo is public

