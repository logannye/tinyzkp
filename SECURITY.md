# Security Policy

## Supported Versions

We release patches for security vulnerabilities in the following versions:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**⚠️ DO NOT open a public GitHub issue for security vulnerabilities.**

We take security seriously. If you discover a security vulnerability, please follow these steps:

### 1. Contact Us Privately

Create a private issue via GitHub Security Advisories

### 2. Include the Following Information:

- **Type of vulnerability** (e.g., authentication bypass, injection, cryptographic flaw)
- **Affected component** (API endpoint, library function, configuration)
- **Impact assessment** (who is affected, severity, potential damage)
- **Steps to reproduce** (detailed instructions, proof-of-concept if available)
- **Suggested fix** (if you have one)
- **Your contact information** (for follow-up)

### 3. Our Response Timeline

- **Initial response**: Within 48 hours
- **Fix timeline**: Within 7 days for critical issues
- **Public disclosure**: 90 days after fix deployment (coordinated disclosure)

### 4. Recognition

We believe in recognizing security researchers:
- We will credit you in our CHANGELOG (unless you prefer to remain anonymous)
- For critical findings, we may offer rewards (case-by-case basis)

---

## Security Best Practices

### For Developers Integrating TinyZKP

1. **Never use dev-srs in production**
   ```bash
   # ❌ INSECURE - only for local development
   cargo build --features dev-srs
   
   # ✅ SECURE - production build without dev-srs
   cargo build --release
   ```

2. **Always use HTTPS**
   - Set `CORS_ALLOWED_ORIGINS` to your actual domains
   - Never use `CORS_ALLOWED_ORIGINS=*` in production

3. **Protect API keys**
   - Store API keys in environment variables, never in code
   - Rotate keys if compromised via `/v1/keys/rotate`
   - Use separate keys for dev/staging/production

4. **Validate SRS digests**
   ```bash
   # Compare against known-good ceremony values
   sha256sum srs/G1.bin srs/G2.bin
   ```

5. **Monitor security logs**
   - Set `RUST_LOG=info` to capture security events
   - Alert on unusual patterns (failed logins, rate limit hits)

---

## Known Security Considerations

### 1. SRS Security (CRITICAL)

**Risk**: If the trusted setup parameter τ (tau) is known, the entire ZKP system is compromised.

**Mitigation**:
- Always use SRS from multi-party computation (MPC) ceremonies
- Verify SRS digests against ceremony transcripts
- **NEVER** use `dev-srs` feature in production (τ is publicly known)

**Recommended Sources**:
- [Perpetual Powers of Tau](https://github.com/privacy-scaling-explorations/perpetualpowersoftau)
- [CDK-Erigon/Polygon Ceremony](https://github.com/0xPolygon/cdk-erigon)
- [Aztec Ignition]([https://aztec-ignition.s3.amazonaws.com/](https://github.com/AztecProtocol/ignition-verification))

### 2. API Key Management

**Risk**: Stolen API keys allow unauthorized access to paid features.

**Mitigation**:
- API keys are cryptographically generated (blake3 + OsRng)
- Keys can be rotated without losing usage history
- Usage caps enforced per-tier
- Keys can be disabled by marking tier as "disabled"

### 3. Stripe Webhook Security

**Risk**: Forged webhooks could manipulate billing tiers.

**Mitigation** (as of v0.1.0):
- ✅ Webhook signatures verified using `STRIPE_WEBHOOK_SECRET`
- ✅ All webhook events logged
- ✅ Metadata validated before tier updates

### 4. Rate Limiting

**Risk**: DDoS attacks could exhaust server resources.

**Mitigation**:
- ✅ Per-IP rate limiting (10 req/sec, burst 30)
- ✅ Monthly usage caps per API key
- ✅ Request size limits (32MB max)

### 5. Authentication

**Risk**: Credential stuffing, password attacks.

**Mitigation**:
- ✅ Passwords hashed with Argon2id (industry standard)
- ✅ Minimum password length: 8 characters
- ✅ Failed login attempts logged with IP
- ✅ Session tokens expire after 30 days

---

## Security Features

### ✅ Implemented

- **Password Hashing**: Argon2id with per-user salts
- **API Key Generation**: Cryptographic RNG (OsRng) + blake3
- **Session Management**: Secure bearer tokens with expiration
- **Stripe Webhooks**: Signature verification enforced
- **CORS**: Configurable origin whitelist
- **Rate Limiting**: Per-IP throttling
- **Input Validation**: Email format, password strength, tier limits
- **SRS Validation**: Digest verification prevents mismatches
- **Proof Integrity**: Tampered proofs rejected
- **Security Logging**: Failed auth, webhook events, tier changes

### 🚧 Future Enhancements

- **Two-Factor Authentication (2FA)** - Planned for v0.2.0
- **API Key Scopes** - Read-only vs full access
- **Webhook Replay Attack Prevention** - Timestamp validation
- **IP Whitelisting** - Per API key
- **Audit Logs** - Complete action history

---

## Compliance

### Data Protection

- **Passwords**: Never stored in plaintext (Argon2id hashed)
- **API Keys**: Stored as opaque identifiers
- **Personal Data**: Email addresses only (minimal collection)
- **Data Retention**: User data retained while account active

### GDPR Compliance

- Users can request data deletion (contact support)
- Data minimization: only email + password hash stored
- Right to access: Users can query their data via `/v1/me`

---

## Security Audit History

| Date | Type | Severity | Status |
|------|------|----------|--------|
| 2024-10-02 | Internal Review | Critical | ✅ Fixed (Stripe webhooks) |
| 2024-10-02 | Internal Review | Critical | ✅ Fixed (CORS policy) |
| 2024-10-02 | Internal Review | High | ✅ Fixed (Rate limiting) |

*Third-party security audit: Pending*

---

## Responsible Disclosure Examples

We appreciate responsible security researchers. Here are some examples of vulnerabilities we'd like to know about:

### Critical
- Authentication bypass
- Privilege escalation
- Remote code execution
- SQL/NoSQL injection
- Cryptographic flaws in ZKP system

### High
- Unauthorized data access
- Session hijacking
- Stripe webhook manipulation
- DDoS amplification

### Medium
- Information disclosure
- CSRF (despite CORS protections)
- Open redirects

### Low
- Version disclosure
- Missing security headers

---

## Security Configuration Checklist

Before deploying to production, ensure:

- [ ] `STRIPE_WEBHOOK_SECRET` is set
- [ ] `TINYZKP_ADMIN_TOKEN` is a strong random value (not "changeme-admin")
- [ ] `CORS_ALLOWED_ORIGINS` is set to your actual domains (not "*")
- [ ] `SSZKP_SRS_G1_PATH` and `SSZKP_SRS_G2_PATH` point to trusted ceremony files
- [ ] `dev-srs` feature is NOT enabled
- [ ] HTTPS is enforced (Railway does this automatically)
- [ ] Environment variables are not committed to git
- [ ] Security logging is enabled (`RUST_LOG=info`)

---

## Contact

- **Security Email**: security@tinyzkp.com
- **General Support**: support@tinyzkp.com
- **GitHub Issues**: For non-security bugs only

---

**Last Updated**: October 2, 2024  
**Version**: 0.1.0

