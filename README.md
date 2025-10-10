# TinyZKP

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)
[![Production](https://img.shields.io/badge/status-production-green.svg)](https://api.tinyzkp.com/v1/health)
[![API](https://img.shields.io/badge/API-live-blue.svg)](https://api.tinyzkp.com)

**Sublinear-Space Zero-Knowledge Proof System with Production-Ready REST API**

TinyZKP is a high-performance ZKP prover/verifier that uses only O(√T) memory for proofs over traces of length T, rather than the typical O(T) that most systems require.

## 🌟 Features

- **Sublinear Space**: Proves traces of 1M rows using only ~1,024 row memory (1,000× less!)
- **Production Capacity**: 1M degree SRS (supports circuits up to 1,048,576 rows)
- **Production zkML**: Enable MNIST, small CNNs, MobileNet inference proofs
- **Production API**: REST API with tiered pricing (Free/Pro/Scale)
- **Secure**: HMAC webhook verification, rate limiting, CORS protection
- **Fast**: Streaming Blocked-IFFT, optimized BN254 operations
- **Open Source**: MIT License

## 🔗 Quick Links

- 🌐 **API**: https://api.tinyzkp.com
- 📖 **Website**: https://tinyzkp.com
- 💻 **GitHub**: https://github.com/logannye/tinyzkp
- 🔐 **Security**: [SECURITY.md](SECURITY.md)
- 🚀 **Production Status**: [PRODUCTION_LAUNCH_COMPLETE.md](PRODUCTION_LAUNCH_COMPLETE.md)

## 🎯 Why TinyZKP?

**Traditional ZKP provers** require O(T) memory - proving a 1M row circuit needs 1M rows in memory (~32 MB).

**TinyZKP** uses streaming algorithms to prove with only O(√T) memory - proving a 1M row circuit needs just ~1,024 rows in memory (~16 KB).

**Result**: 
- 💾 **2,000× less memory** for large circuits (16 KB vs 32 MB)
- ⚡ **Faster proofs** on commodity hardware  
- 🧠 **zkML-ready** - MNIST, small CNNs, MobileNet
- 🌐 **REST API** - no local setup required
- 💰 **Pay as you grow** - free tier to start

## 🚀 Quick Start

### Using the Production API

**Status**: ✅ Production - Fully operational  
**Endpoint**: https://api.tinyzkp.com

1. **Sign up for free account**
```bash
curl -X POST https://api.tinyzkp.com/v1/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email":"you@example.com","password":"YourSecurePass123!"}'
```

2. **Login to get your API key**
```bash
curl -X POST https://api.tinyzkp.com/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"you@example.com","password":"YourSecurePass123!"}'
```

Response includes:
- `session_token` - For account management (dashboard, billing)
- `api_key` - For proof generation (starts with `tz_`)

3. **Generate a proof**
```bash
curl -X POST https://api.tinyzkp.com/v1/prove \
  -H "X-API-Key: tz_your_key_here" \
  -H "Content-Type: application/json" \
  -d @proof_request.json
```

### Local Development

1. **Install Rust** (1.82+)
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. **Clone and build**
```bash
git clone https://github.com/logannye/tinyzkp.git
cd tinyzkp
cargo build --release --bin tinyzkp_api
```

3. **Set up environment**
```bash
cp .env.example .env
# Edit .env with your Stripe, Redis, and other credentials
```

4. **Generate development SRS** (for testing only)
```bash
./scripts/generate_dev_srs.sh
```

5. **Run the API**
```bash
cargo run --release --bin tinyzkp_api
```

API will be available at `http://localhost:8080`

## 📡 Production API

Our hosted API is available at `https://api.tinyzkp.com`

### Pricing Tiers

| Tier | Price | Monthly Proofs | Max Circuit Size | Best For |
|------|-------|----------------|------------------|----------|
| **Free** | $0/mo | 250 | 32,768 rows | Learning, prototyping |
| **Pro** | $39/mo | 500 | 262,144 rows | Production dev use |
| **Scale** | $149/mo | 1,000 | 1,048,576 rows (1M) | Enterprise zkML |

### 📊 What You Can Prove

| Tier | Circuit Size | Example Use Cases |
|------|--------------|-------------------|
| **Free (32K rows)** | 32,768 | MNIST-class models, basic constraint systems, small ML |
| **Pro (262K rows)** | 262,144 | MobileNet (quantized), CIFAR-10 CNNs, medium ML models |
| **Scale (1M rows)** | 1,048,576 | **MNIST full, ResNet-18, small transformers** - production zkML |

### Rate Limits

- **Global**: 10 requests/second per IP (burst: 30)
- **Monthly caps**: Enforced per tier (see table above)
- **Circuit size**: Enforced per tier
- **Proof generation**: Up to 1M rows (Scale tier)

### 💳 Upgrading Your Account

1. Sign up for free tier (250 proofs/month, 32K rows)
2. Visit https://tinyzkp.com to upgrade
3. Choose Pro ($39/mo, 262K rows) or Scale ($149/mo, 1M rows)
4. Complete payment via Stripe
5. Your account is upgraded instantly
6. Same API key, new limits!

## 🛣️ API Endpoints

### Public
- `GET /v1/health` - Health check
- `GET /v1/version` - API version info
- `POST /v1/domain/plan` - Domain planning
- `POST /v1/auth/signup` - Create account
- `POST /v1/auth/login` - Get API key

### Authenticated (requires API key)
- `POST /v1/prove` - Generate ZK proof
- `POST /v1/verify` - Verify ZK proof  
- `GET /v1/me` - User profile
- `POST /v1/keys/rotate` - Rotate API key
- `POST /v1/billing/checkout` - Upgrade account
- `POST /v1/proof/inspect` - Inspect proof details

### Admin (requires admin token)
- `POST /v1/admin/keys` - Create API keys
- `POST /v1/admin/keys/:key/tier` - Set user tier
- `GET /v1/admin/keys/:key/usage` - Usage stats
- `POST /v1/admin/srs/init` - Initialize SRS

Full API reference: [DEPLOYMENT.md](DEPLOYMENT.md)

## 🏗️ Architecture

- **AIR (Algebraic Intermediate Representation)**: Define computation constraints
- **Polynomial Commitment Scheme**: KZG with BN254 curve
- **Streaming Prover**: Blocked-IFFT with configurable tile size
- **Scheduler**: Multi-phase proof generation (trace → quotient → opening)
- **REST API**: Axum-based with Redis state management

## 🔒 Security

- All webhooks verified with HMAC-SHA256
- Rate limiting: 10 req/sec per IP
- CORS: Strict origin whitelist
- Passwords: Argon2id hashing
- API Keys: Cryptographically generated (OsRng + BLAKE3)
- See [SECURITY.md](SECURITY.md) for vulnerability reporting

## 🔐 Cryptographic Setup (SRS)

### Production API

Our production API uses a cryptographically-secure **1M degree SRS**:
- **Capacity**: Supports circuits up to **1,048,576 rows**
- **File size**: 32 MB (G1.bin) + 136 bytes (G2.bin)
- **Memory usage**: Only ~16 KB for 1M row proofs (O(√T) efficiency)
- **Generation**: OS entropy (OsRng) - cryptographically secure
- **Security**: Tau destroyed after generation (never saved to disk)
- **Setup type**: Single-party trusted setup (secure if generation was honest)
- **Enables**: Production zkML (MNIST full, ResNet-18, small transformers)

### Local Development

For local testing, use the dev SRS generator:
```bash
./scripts/generate_dev_srs.sh
```

⚠️ **Dev SRS is NOT secure** - only for local development
- Limited to 4K degree
- Uses publicly-known parameters
- Never use in production

### Advanced: Multi-Party Ceremony

You can replace the SRS with output from a multi-party computation ceremony if needed. See `src/bin/generate_production_srs.rs` for reference implementation.

## 📚 Documentation

- [Production Launch Summary](PRODUCTION_LAUNCH_COMPLETE.md)
- [Production Readiness Assessment](PRODUCTION_READINESS.md)
- [Deployment Guide](DEPLOYMENT.md)
- [Security Policy](SECURITY.md)

## 🛠️ Development

### Running Tests
```bash
cargo test --all
```

### Building Optimized Binary
```bash
cargo build --release --bin tinyzkp_api
```

### Scripts
- `scripts/generate_dev_srs.sh` - Generate dev SRS (⚠️ NOT for production)
- `scripts/test_api_local.sh` - Test API endpoints locally
- `scripts/test_security.sh` - Run security checks
- `scripts/test_production_readiness.sh` - Production readiness tests
- `scripts/test_performance.sh` - Performance benchmarks

## 🤝 Contributing

We welcome contributions! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📜 License

This project is licensed under the MIT License - see [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- Built on [arkworks](https://github.com/arkworks-rs) elliptic curve library
- Inspired by PLONK, FRI, and streaming ZKP research
- Uses BN254 curve from Ethereum ecosystem

## ⚠️ Important Notes

### Security
- Always verify webhook signatures in production
- Keep your admin token secret
- Rotate API keys regularly
- See [SECURITY.md](SECURITY.md) for full security guidelines

### SRS Usage
- **Production API**: Uses cryptographically-secure **1M degree SRS** (32 MB)
- **Capacity**: Up to **1,048,576 rows** per circuit (Scale tier)
- **Memory**: Only **~16 KB** for 1M row proofs (O(√T) advantage)
- **Local Development**: Use `generate_dev_srs.sh` (max 4K degree, insecure)
- **Never use dev SRS in production** - parameters are publicly known

### Rate Limits
- Global: 10 requests/second per IP (burst: 30)
- Monthly caps enforced per tier (Free: 250, Pro: 500, Scale: 1,000)
- Circuit size limits enforced per tier (Free: 32K, Pro: 262K, Scale: 1M)
- **Scale tier enables production zkML** (MNIST full, ResNet-18, small transformers)

---

**Built with Rust 🦀 | Production-Ready 🚀 | Open Source 🔓**
