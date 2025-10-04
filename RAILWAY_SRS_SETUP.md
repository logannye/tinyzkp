# Railway SRS Setup Guide

## Problem: Large SRS Files and GitHub Limits

GitHub has a 100 MB file size limit. Our production SRS files exceed this:
- **2M degree SRS**: G1.bin = 64 MB ✅ (under limit)
- **4M degree SRS**: G1.bin = 128 MB ❌ (exceeds limit)

## Solution: Generate SRS Directly on Railway

Instead of storing large SRS files in git, we generate them directly on the Railway volume where they persist across deployments.

---

## 🚀 Initial Setup (One-Time)

### **Step 1: Deploy the Application**

The application will deploy with a minimal/dev SRS or auto-load from the Railway volume if it exists.

```bash
git push origin main
# Railway automatically builds and deploys
```

### **Step 2: Generate Production SRS on Railway**

Use Railway CLI to access the container and generate the SRS:

```bash
# Option A: Using Railway CLI (recommended)
railway run bash
cd /app
cargo run --release --bin generate_production_srs -- 4194304

# Option B: Using the helper script
railway run ./scripts/generate_production_srs_railway.sh
```

**Note:** This takes 10-20 minutes. The SRS is written directly to `/app/srs/` on the Railway volume.

### **Step 3: Restart the Service**

After generation completes, restart the Railway service to load the new SRS:

```bash
# From Railway dashboard: Click "Restart" on your service
# Or via CLI:
railway restart
```

### **Step 4: Verify SRS is Loaded**

Check the deployment logs for:

```
Loading production SRS from files...
  G1: /app/srs/G1.bin
  G2: /app/srs/G2.bin
✓ Production SRS loaded successfully
SRS initialized successfully:
  G1 digest: [...]
  G2 digest: [...]
```

---

## 📊 SRS Specifications

| Degree | Max Rows | G1.bin Size | G2.bin Size | Memory Usage | Use Case |
|--------|----------|-------------|-------------|--------------|----------|
| 131K | 131,072 | 4 MB | 136 bytes | ~5 KB | Small ML models |
| 2M | 2,097,152 | 64 MB | 136 bytes | ~23 KB | MobileNet, ResNet-18 |
| **4M** | **4,194,304** | **128 MB** | **136 bytes** | **~32 KB** | **ResNet-34, BERT-base** |

---

## 🔄 Upgrading SRS

To upgrade from 2M to 4M (or any larger SRS):

1. **Access Railway container:**
   ```bash
   railway run bash
   ```

2. **Generate new SRS:**
   ```bash
   cargo run --release --bin generate_production_srs -- 4194304
   ```

3. **Verify new files:**
   ```bash
   ls -lh /app/srs/
   # Should show:
   # -rw-r--r-- 128M G1.bin
   # -rw-r--r-- 136  G2.bin
   ```

4. **Restart service:**
   ```bash
   railway restart
   ```

5. **Check logs for successful load:**
   Look for "SRS initialized successfully" in deployment logs.

---

## ⚠️ Important Notes

### **Volume Persistence**

- The Railway volume at `/app/srs/` persists across deployments
- You only need to generate the SRS **once per upgrade**
- Subsequent deployments will use the existing SRS from the volume

### **entrypoint.sh Behavior**

The entrypoint script (`entrypoint.sh`) automatically:
1. Checks if SRS files in `/tmp/srs-init` (from Docker image) are newer/different
2. If different, copies them to `/app/srs/` (Railway volume)
3. If same or no image files, uses existing volume files
4. This allows both git-based (small SRS) and manual (large SRS) workflows

### **Local Development**

For local development, continue using the dev SRS generator:

```bash
./scripts/generate_dev_srs.sh
```

**Never use dev SRS in production** - parameters are publicly known and insecure.

---

## 🧪 Testing

After generating the new SRS, test with a large circuit:

```bash
curl -X POST https://api.tinyzkp.com/v1/prove \
  -H "X-API-Key: tz_your_key_here" \
  -H "Content-Type: application/json" \
  -d '{
    "air": {
      "num_columns": 3,
      "num_public_inputs": 2,
      "k": 4,
      "transition_constraints": [],
      "boundary_constraints": []
    },
    "witness": {
      "format": "json_rows",
      "rows": [[1,2,3], ..., [4M rows total]]
    },
    "return_proof": true
  }'
```

---

## 📝 Troubleshooting

### **Problem: "SRS not initialized"**

**Solution:** The SRS wasn't loaded on startup. Check:
1. Files exist: `railway run ls -lh /app/srs/`
2. Restart service: `railway restart`
3. Check logs for loading errors

### **Problem: "Out of memory" during generation**

**Solution:** Railway's default instance might be too small. Upgrade to a higher-memory instance temporarily:
1. Generate SRS on higher-memory instance
2. Downgrade after generation completes (SRS is on volume, persists)

### **Problem: "Cargo not found"**

**Solution:** Make sure you're running in the built Railway container:
```bash
railway run bash  # Not railway shell
```

---

## 🎯 Summary Workflow

```bash
# 1. Deploy application
git push origin main

# 2. Generate SRS on Railway (one-time)
railway run bash
cargo run --release --bin generate_production_srs -- 4194304
exit

# 3. Restart to load new SRS
railway restart

# 4. Verify in logs
railway logs

# Done! SRS persists on volume for all future deployments
```

---

**The SRS is now ready for production use supporting circuits up to 4,194,304 rows!** 🚀
