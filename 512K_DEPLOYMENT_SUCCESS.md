# ✅ 512K SRS Deployment - SUCCESSFUL

**Date:** October 11, 2025  
**Status:** ✅ **FULLY OPERATIONAL**

---

## 🎯 What Was Accomplished

### 1. **SRS Size Reduction: 4M → 512K** (32x smaller)
- **Old:** 4M rows (4,194,304) = 129 MB
- **New:** 512K rows (524,288) = 16 MB  
- **File format:** 8-byte header + (N+1) × 32-byte G1 points = 16,777,256 bytes
- **Impact:** Faster loading, lower memory footprint, more reliable deployments

### 2. **Docker Image Integration**
- SRS files now embedded directly in Docker image (at `/tmp/srs_image/`)
- No more manual SRS initialization required
- Resilient to Railway volume issues (old files automatically replaced)

### 3. **Intelligent Entrypoint Script**
The `entrypoint.sh` now:
- ✅ Checks SRS files on Railway volume
- ✅ Detects wrong-sized files (old 2M/4M SRS)
- ✅ Automatically replaces with correct 512K SRS from Docker image
- ✅ Ensures correct permissions for `tinyzkp` user

**Example from deployment logs:**
```
❌ SRS files on volume are wrong size (found: 134217768 bytes, expected: ~16MB)
📦 Replacing with 512K SRS from Docker image...
✓ 512K SRS files copied to volume
```

### 4. **Background SRS Loading**
- ✅ First proof request triggers background loading (returns 503 immediately)
- ✅ No HTTP timeouts during SRS initialization
- ✅ Loading completes in ~60 seconds (Railway CPU)
- ✅ Subsequent requests succeed immediately
- ✅ `/v1/health` endpoint shows loading status

**Loading Timeline:**
| Time | Status | Action |
|------|--------|--------|
| 0s | SRS not loaded | First `/v1/prove` request received |
| 0s | `srs_loading: true` | Returns 503, spawns background task |
| ~60s | `srs_initialized: true` | Loading complete, ready for proofs |
| 60s+ | Ready | All requests succeed instantly |

---

## 🧪 Verification Results

### Health Check
```json
{
  "status": "ok",
  "srs_initialized": true
}
```

### Proof Generation
- ✅ **Status:** Success
- ✅ **Proof size:** 1,012 bytes
- ✅ **SRS G1 digest:** `0x3a3ed5d2703dd09cd7ce95e9c138d7bc4c55a1f9cf9d78c3c692cf4f3a61a505`
- ✅ **SRS G2 digest:** `0x2cf0223d4b1cd1375c425e528420de13dad19143fe901d8190dbc1332591c669`

### Proof Verification
```json
{
  "status": "ok"
}
```

---

## 📊 Updated Tier Limits

### Production API Limits
| Tier | Price | Monthly Proofs | Max Circuit Size | Memory | SRS Degree |
|------|-------|---------------|-----------------|---------|-----------|
| **Free** | $0 | 250 | 32K rows | ~5 KB | 512K |
| **Pro** | $39/mo | 1,000 | 256K rows | ~10 KB | 512K |
| **Scale** | $99/mo | 2,500 | 512K rows | ~12 KB | 512K |

### Environment Variables (Railway)
```bash
# SRS Configuration
TINYZKP_MAX_ROWS=524288           # 512K rows
TINYZKP_FREE_MAX_ROWS=32768       # 32K rows (Free tier)
TINYZKP_PRO_MAX_ROWS=262144       # 256K rows (Pro tier)
TINYZKP_SCALE_MAX_ROWS=524288     # 512K rows (Scale tier)

# Monthly Caps
TINYZKP_FREE_MONTHLY_CAP=250      # Free tier
TINYZKP_PRO_MONTHLY_CAP=1000      # Pro tier
TINYZKP_SCALE_MONTHLY_CAP=2500    # Scale tier
```

---

## 🔧 Technical Implementation

### Files Modified
1. **`Dockerfile`**
   - Copies `srs/` to `/tmp/srs_image/` (prevents Railway volume shadowing)
   
2. **`entrypoint.sh`**
   - Added SRS size validation
   - Auto-replaces wrong-sized SRS files
   - Robust error handling and logging

3. **`src/bin/tinyzkp_api.rs`**
   - Background SRS loading with `tokio::spawn`
   - Immediate 503 response (no HTTP timeout)
   - `SRS_LOADING` and `SRS_INITIALIZED` flags
   - Updated `/v1/health` to show loading status

4. **`.gitignore`**
   - Removed `srs/G1.bin` and `srs/G2.bin` (now tracked in Git)

5. **`README.md`**
   - Updated to reflect 512K row capacity
   - Updated pricing tiers and zkML capabilities

### SRS File Format
```
Offset | Size | Content
-------|------|--------
0x00   | 8    | Point count (524,289 = 0x00080001 little-endian)
0x08   | 32   | G1 point #0 (identity or base)
0x28   | 32   | G1 point #1
...    | ...  | ...
EOF    | -    | Total: 16,777,256 bytes (16 MB)
```

---

## 🚀 Deployment Process

### What Happens on Railway Deploy

1. **Docker Build:**
   - Image built with 512K SRS at `/tmp/srs_image/`
   - ~500 MB total image size

2. **Container Start (`entrypoint.sh`):**
   ```bash
   === TinyZKP Entrypoint (running as root) ===
   ❌ SRS files on volume are wrong size (found: 134217768 bytes, expected: ~16MB)
   📦 Replacing with 512K SRS from Docker image...
   ✓ 512K SRS files copied to volume
   Files in /app/srs:
   -rw-r--r-- 1 tinyzkp tinyzkp 17M Oct 11 03:40 G1.bin
   -rw-r--r-- 1 tinyzkp tinyzkp 136 Oct 11 03:40 G2.bin
   ```

3. **API Startup:**
   ```
   tinyzkp API listening on http://0.0.0.0:8080
   SRS Initialization: Background loading enabled
     - Max degree: 524288 (512K rows, 16MB)
     - First request returns 503, loading in background (~60s)
   ```

4. **First Proof Request:**
   - Client sends `/v1/prove`
   - API returns `503 Service Unavailable` immediately
   - Background task loads SRS (~60 seconds)
   - Client retries after 60 seconds → Success!

---

## 🐛 Issues Resolved

### Issue #1: Railway Volume Shadowing
**Problem:** Railway persistent volume at `/app/srs` was shadowing Docker image files  
**Solution:** Copy SRS to `/tmp/srs_image/` in Dockerfile, then entrypoint copies to volume

### Issue #2: Wrong-Sized SRS on Volume
**Problem:** Old 2M/4M SRS files persisted on Railway volume  
**Solution:** Entrypoint detects size mismatch and auto-replaces with 512K SRS

### Issue #3: HTTP Timeout During Loading
**Problem:** Loading 16MB SRS took ~60s, causing HTTP request timeout  
**Solution:** Background loading with immediate 503 response

### Issue #4: Loading Time Estimate
**Problem:** Initial estimate was "~30 seconds" but actual time was ~60 seconds  
**Root cause:** Railway CPU slower for crypto operations than local testing  
**Status:** Expected behavior, docs should reflect 60s estimate

---

## ✅ Next Steps

### 1. Update Documentation
- [ ] Update website to reflect 60-second SRS loading time (not 30s)
- [ ] Add note: "First proof after deployment may take 60s to initialize"

### 2. Environment Variable Cleanup (Optional)
Current Railway variables can stay as-is, but ensure:
```bash
TINYZKP_SCALE_MONTHLY_CAP=2500  ← Should be 2500 (not 1000)
```

### 3. Monitor Production
- Watch for any SRS loading failures in Railway logs
- Confirm all users experience smooth operation after initial 60s warmup

### 4. Consider Future Optimizations
- **Parallel G1/G2 loading** (currently sequential)
- **Pre-deserialization** (load + deserialize in parallel)
- **SRS caching** (Redis/memcached for multi-instance deployments)

---

## 📋 Quick Reference

### Test API Manually
```bash
# 1. Check health
curl https://api.tinyzkp.com/v1/health

# 2. Generate proof (triggers SRS loading)
curl -X POST https://api.tinyzkp.com/v1/prove \
  -H "X-API-Key: YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "air": {"k": 3},
    "domain": {"rows": 8, "b_blk": 2, "zh_c": "1"},
    "pcs": {"basis_wires": "eval"},
    "witness": {
      "format": "json_rows",
      "rows": [
        [1,2,3], [2,4,6], [3,6,9], [4,8,12],
        [5,10,15], [6,12,18], [7,14,21], [8,16,24]
      ]
    },
    "return_proof": true
  }'

# If you get 503, wait 60 seconds and retry

# 3. Verify proof
# (save proof_b64 from step 2, decode, then:)
curl -X POST https://api.tinyzkp.com/v1/verify \
  -H "X-API-Key: YOUR_API_KEY" \
  -F "proof=@proof.bin"
```

### Railway Logs to Check
```bash
railway logs -f
```

Look for:
- ✅ `✓ 512K SRS files copied to volume`
- ✅ `SRS Initialization: Background loading enabled`
- ✅ `⏳ Starting background SRS loading (16MB, ~30 seconds)...`
- ✅ (No error messages after "Files exist, loading...")

---

## 🎉 Summary

The TinyZKP API is now successfully deployed with **512K SRS** (16 MB), supporting:

✅ **Free tier:** 32K row circuits (MNIST inference)  
✅ **Pro tier:** 256K row circuits (MNIST full, small CNNs)  
✅ **Scale tier:** 512K row circuits (image classification, small transformers)  

The system automatically handles SRS initialization, replaces outdated files, and gracefully manages background loading with no HTTP timeouts. 

**Status:** Production-ready! 🚀

---

*Generated: October 11, 2025*

