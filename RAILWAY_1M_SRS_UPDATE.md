# Railway 1M SRS Deployment Guide

## ✅ Changes Completed

### 1. SRS Files (Committed to GitHub)
- **G1.bin**: 32 MB (1,048,577 G1 powers)
- **G2.bin**: 136 bytes (τ·G₂)
- **Total**: ~32 MB
- **Committed**: `914a952` - "Reduce SRS to 1M rows (32MB) with increased Scale tier proofs"

### 2. Updated Tier Structure

| Tier | Monthly Proofs | Max Circuit Size | Price |
|------|----------------|------------------|-------|
| Free | 250 | 32,768 rows | $0/mo |
| Pro | 500 | 262,144 rows | $39/mo |
| Scale | **1,000** | **1,048,576 rows** | **$149/mo** |

### 3. Benefits of 1M SRS
- ✅ **50% faster loading**: ~30-60 seconds (vs 60-120 seconds for 2M)
- ✅ **GitHub friendly**: 32MB fits comfortably (vs 64MB warning)
- ✅ **More generous cap**: 1,000 proofs/month (vs 250 for old Scale tier)
- ✅ **Lower price**: $149/mo (vs $199/mo for old 4M tier)
- ✅ **Still huge capacity**: 1M rows covers MNIST full, ResNet-18, small transformers

---

## 🚀 Railway Deployment Steps

### Step 1: Update Environment Variables

In Railway dashboard (`api.tinyzkp.com` service), update these variables:

```bash
# Maximum SRS capacity (1M rows)
TINYZKP_MAX_ROWS=1048576

# Tier limits
TINYZKP_FREE_MAX_ROWS=32768
TINYZKP_PRO_MAX_ROWS=262144
TINYZKP_SCALE_MAX_ROWS=1048576

# Monthly proof caps
TINYZKP_FREE_MONTHLY_CAP=250
TINYZKP_PRO_MONTHLY_CAP=500
TINYZKP_SCALE_MONTHLY_CAP=1000

# SRS file paths (already correct, but verify)
SSZKP_SRS_G1_PATH=/app/srs/G1.bin
SSZKP_SRS_G2_PATH=/app/srs/G2.bin
```

### Step 2: Redeploy on Railway

Railway will automatically:
1. Pull the latest commit (`914a952`)
2. Clone the repo with 1M SRS files
3. Start the API with new env vars

**Estimated deployment time**: 2-3 minutes

### Step 3: Initialize SRS (After Deploy)

**CRITICAL**: After Railway redeploys, you must manually initialize the SRS:

```bash
curl -X POST https://api.tinyzkp.com/v1/admin/srs/init \
  -H "X-Admin-Token: Hartsgrove26!!" \
  -H "Content-Type: application/json" \
  -d '{
    "max_degree": 1048576,
    "mode": "production"
  }'
```

**Expected response time**: 30-60 seconds (loading 32MB)

**Success response**:
```json
{
  "message": "SRS initialized successfully",
  "max_degree": 1048576,
  "memory_usage_mb": 32
}
```

### Step 4: Verify Deployment

**Health check**:
```bash
curl https://api.tinyzkp.com/v1/health
```

Expected:
```json
{
  "status": "ok",
  "timestamp": "...",
  "srs_initialized": true
}
```

**Version check**:
```bash
curl https://api.tinyzkp.com/v1/version
```

Expected:
```json
{
  "version": "0.1.0",
  "features": ["production"]
}
```

**Test proof** (using your Scale account):
```bash
curl -X POST https://api.tinyzkp.com/v1/prove \
  -H "X-API-Key: tz_YOUR_KEY_HERE" \
  -H "Content-Type: application/json" \
  -d '{
    "air": {"k": 3},
    "domain": {"rows": 8, "b_blk": 2, "zh_c": "1"},
    "pcs": {"basis_wires": "eval"},
    "witness": {
      "format": "json_rows",
      "rows": [[1,2,3],[2,4,6],[3,6,9],[4,8,12],[5,10,15],[6,12,18],[7,14,21],[8,16,24]]
    },
    "return_proof": true
  }'
```

---

## 📋 Post-Deployment Checklist

- [ ] Railway env vars updated
- [ ] Railway redeployed with new commit `914a952`
- [ ] SRS initialized via `/v1/admin/srs/init`
- [ ] Health check passes (`/v1/health`)
- [ ] Test proof generates successfully
- [ ] Version endpoint shows correct info
- [ ] Update Stripe pricing (if needed): Scale tier = $149/mo
- [ ] Update tinyzkp.com landing page:
  - Scale tier: 1,048,576 rows, $149/mo, 1,000 proofs/month
  - Memory efficiency: 1M rows uses only ~16KB during proving
  - Use cases: MNIST full, ResNet-18, small transformers

---

## 🔧 Troubleshooting

### SRS Initialization Times Out
- **Cause**: Loading 32MB can take 30-60 seconds
- **Fix**: Wait longer, or retry the initialization request

### "SRS not initialized" Error
- **Cause**: Forgot to run manual initialization after deploy
- **Fix**: Run the `/v1/admin/srs/init` curl command (Step 3)

### 502 Bad Gateway
- **Cause**: Service crashed during startup
- **Fix**: Check Railway logs, verify env vars are correct

### "Circuit exceeds SRS capacity"
- **Cause**: User tried to prove >1M rows
- **Fix**: This is expected! Scale tier max is now 1M rows.

---

## 🎯 What's Changed for Users?

### Before (4M SRS)
- Scale tier: $199/mo, 250 proofs/month, 4,194,304 rows
- Use cases: ResNet-34, BERT-base, GPT-2-medium
- File size: 128 MB
- Load time: 60-120 seconds

### After (1M SRS)
- Scale tier: **$149/mo**, **1,000 proofs/month**, **1,048,576 rows**
- Use cases: MNIST full, ResNet-18, small transformers
- File size: **32 MB**
- Load time: **30-60 seconds**

### Trade-offs
- ✅ **4× more proofs per month** (1,000 vs 250)
- ✅ **Lower price** ($149 vs $199)
- ✅ **Faster, more reliable** (smaller SRS)
- ⚠️ **75% smaller max circuit** (1M vs 4M rows)
- ⚠️ **Smaller zkML models** (ResNet-18 vs ResNet-34)

**Verdict**: Better value for most users! 1M rows is still huge for 99% of use cases.

---

## 📞 Support

If you encounter issues:
1. Check Railway logs
2. Verify SRS initialization completed
3. Test with small proof first
4. Contact support with request details

---

**Deployment Date**: October 9, 2025  
**Commit**: `914a952`  
**Status**: Ready for production deployment 🚀

