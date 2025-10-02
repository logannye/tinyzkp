# TinyZKP

**Sublinear-Space Zero-Knowledge Proof System with Production-Ready REST API**

TinyZKP is a high-performance ZKP prover/verifier that uses only O(√T) memory for proofs over traces of length T, rather than the typical O(T) that most systems require.

## 🌟 Features

- **Sublinear Space**: Proves traces of 65K rows using only ~256 row memory
- **Production API**: REST API with tiered pricing (Free/Pro/Scale)
- **Secure**: HMAC webhook verification, rate limiting, CORS protection
- **Fast**: Streaming Blocked-IFFT, optimized BN254 operations
- **Open Source**: MIT License

## 🚀 Quick Start

### Local Development

1. **Install Rust** (1.70+)
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. **Clone and build**
```bash
git clone https://github.com/YOUR_USERNAME/tinyzkp.git
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

| Tier | Monthly Requests | Max Rows | Price |
|------|------------------|----------|-------|
| Free | 500 | 4,096 | $0/mo |
| Pro | 5,000 | 16,384 | $29/mo |
| Scale | 50,000 | 65,536 | $99/mo |

### API Example

```bash
# Sign up for an API key at https://tinyzkp.com
curl -X POST https://api.tinyzkp.com/v1/prove \
  -H "X-API-Key: your_key_here" \
  -H "Content-Type: application/json" \
  -d @proof_request.json
```

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
- See [SECURITY.md](SECURITY.md) for details

## 📚 Documentation

- [Deployment Guide](DEPLOYMENT_CHECKLIST.md)
- [Security Policy](SECURITY.md)
- [API Reference](DEPLOYMENT.md)

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

## 📞 Support

- **Website**: https://tinyzkp.com
- **Email**: support@tinyzkp.com
- **Security Issues**: See [SECURITY.md](SECURITY.md)

## ⚠️ Important Notes

- **Never use `dev-srs` feature in production** - the trusted setup parameter is publicly known
- **Always use SRS from a trusted multi-party computation ceremony**
- See [SECURITY.md](SECURITY.md) for production security requirements
