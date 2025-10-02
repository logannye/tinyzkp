# Production Deployment Guide

## Prerequisites

1. **Railway Account** - https://railway.app
2. **Upstash Redis** - https://upstash.com
3. **Stripe Account** - https://stripe.com
4. **Production SRS Files** - From trusted ceremony

## Step 1: Production SRS Setup

### Option A: Download from Hermez/Polygon Ceremony
```bash
# Download degree 2^20 ceremony (supports N up to 1,048,576)
wget https://hermez.s3-eu-west-1.amazonaws.com/powersOfTau28_hez_final_20.ptau

# Convert to Arkworks format (TODO: add conversion script)
# For now, use snarkjs or manual conversion
# Output: G1.bin and G2.bin
```

### Option B: Use Existing Ceremony Files
Contact ceremony coordinators for pre-converted Arkworks files.
Verify SRS Integrity
bash# Compute digests
sha256sum G1.bin G2.bin

# Compare against ceremony transcript
# Expected values: [insert from ceremony docs]
Step 2: Railway Setup
Create Project
bash# Install Railway CLI
npm install -g @railway/cli

# Login
railway login

# Create project
railway init
Configure Environment Variables
In Railway dashboard, add these environment variables:
Required:
UPSTASH_REDIS_REST_URL=https://your-redis.upstash.io
UPSTASH_REDIS_REST_TOKEN=your-token
TINYZKP_ADMIN_TOKEN=[generate with: openssl rand -hex 32]
STRIPE_SECRET_KEY=sk_live_...
STRIPE_PRICE_PRO=price_...
STRIPE_PRICE_SCALE=price_...
SRS Configuration:
SSZKP_SRS_G1_PATH=/app/srs/G1.bin
SSZKP_SRS_G2_PATH=/app/srs/G2.bin
Optional but Recommended:
RUST_LOG=warn
TINYZKP_FREE_MONTHLY_CAP=500
TINYZKP_PRO_MONTHLY_CAP=5000
TINYZKP_SCALE_MONTHLY_CAP=50000
TINYZKP_MAX_ROWS=131072
Upload SRS Files
Railway doesn't support file uploads directly, so use volume mounts:
bash# Create Railway volume
railway volume create srs

# Mount volume at /app/srs
railway volume attach srs /app/srs

# Upload files (Railway CLI)
railway run bash -c "cat > /app/srs/G1.bin" < G1.bin
railway run bash -c "cat > /app/srs/G2.bin" < G2.bin
Alternative: Cloudflare R2 + Download at Startup
If Railway volumes are problematic, modify Dockerfile:
dockerfile# Add to Dockerfile before CMD
RUN apt-get install -y wget
COPY scripts/download_srs.sh /app/
RUN chmod +x /app/download_srs.sh

CMD ["/app/download_srs.sh && /usr/local/bin/tinyzkp_api"]
Create scripts/download_srs.sh:
bash#!/bin/bash
set -e
mkdir -p /app/srs
wget -O /app/srs/G1.bin "$SRS_G1_URL"
wget -O /app/srs/G2.bin "$SRS_G2_URL"
Set environment variables:
SRS_G1_URL=https://your-r2-bucket.com/G1.bin
SRS_G2_URL=https://your-r2-bucket.com/G2.bin
Step 3: Deploy
bash# Deploy to Railway
railway up

# Check logs
railway logs

# Get public URL
railway domain
Step 4: Initialize SRS
bash# Call initialization endpoint
curl -X POST https://your-app.railway.app/v1/admin/srs/init \
  -H "X-Admin-Token: $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "max_degree": 1048576,
    "validate_pairing": true
  }'
Expected response:
json{
  "status": "initialized",
  "g1_powers": 1048577,
  "g2_loaded": true,
  "g1_digest_hex": "...",
  "g2_digest_hex": "..."
}
Verify digests match your SRS files.
Step 5: Configure Stripe Webhooks

Go to Stripe Dashboard → Developers → Webhooks
Add endpoint: https://your-app.railway.app/v1/stripe/webhook
Select events:

checkout.session.completed
customer.subscription.deleted
customer.subscription.updated


Copy webhook signing secret to Railway env: STRIPE_WEBHOOK_SECRET

Step 6: Test End-to-End
bash# Health check
curl https://your-app.railway.app/v1/health

# Version info
curl https://your-app.railway.app/v1/version

# Test signup
curl -X POST https://your-app.railway.app/v1/auth/signup \
  -H "Content-Type: application/json" \
  -d '{
    "email": "test@example.com",
    "password": "securepassword123"
  }'

# Test prove (requires API key from signup)
curl -X POST https://your-app.railway.app/v1/prove \
  -H "X-API-Key: tz_..." \
  -H "Content-Type: application/json" \
  -d '{
    "air": {"k": 3},
    "domain": {"rows": 256, "b_blk": 16},
    "pcs": {"basis_wires": "eval"},
    "witness": {
      "format": "json_rows",
      "rows": [[1,2,3], [4,5,6], [7,8,9]]
    }
  }'
Monitoring
Railway Metrics

CPU/Memory usage
Request latency
Error rates

Custom Alerts
bash# Set up Railway alerts for:
# - CPU > 80%
# - Memory > 90%
# - Error rate > 5%
Logs
bash# View real-time logs
railway logs --follow
Troubleshooting
"SRS not initialized"

Call /v1/admin/srs/init endpoint
Check SRS files exist at configured paths

"SRS digest mismatch"

Verify SRS files match ceremony files
Recalculate digests: sha256sum srs/*.bin

Out of memory

Increase Railway plan memory
Check TINYZKP_MAX_ROWS is set appropriately
Review b_blk sizes in user requests

High latency

Consider Redis connection pooling
Check Railway region vs Upstash region
Profile with SSZKP_MEMLOG=1


#### 6. `LICENSE` (Open Source)
MIT License
Copyright (c) 2024 [Your Name/Organization]
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:
The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR DEALINGS IN THE
SOFTWARE.
```

#### 7. `SECURITY.md` (GitHub Security Policy)
```markdown
# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**DO NOT** open a public GitHub issue for security vulnerabilities.

Instead:
1. Email: security@[yourdomain].com
2. Include: Detailed description, impact assessment, reproduction steps
3. Expect: Response within 48 hours

## Security Considerations

### SRS Security
- **Never** use dev SRS (seed=42) in production
- Always verify SRS digests match ceremony transcripts
- Store production SRS files securely with restricted access

### API Security
- All endpoints use HTTPS (enforced by Railway)
- API keys are cryptographically generated (blake3)
- Passwords hashed with Argon2id
- Rate limiting enforced via Redis

### Known Limitations
- Dev SRS intentionally insecure (for testing only)
- Stripe webhook signature verification not implemented (v0.1.x)
  - Planned for v0.2.x

## Audit Trail
- SRS digests embedded in all proofs
- Verifier enforces digest matching
- All authentication attempts logged to Redis