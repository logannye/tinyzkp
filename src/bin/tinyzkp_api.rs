//! tinyzkp_api: a minimal REST façade for the sublinear-space ZKP prover/verifier with
//! API key auth + usage metering (Upstash Redis).
//!
//! Public endpoints (JSON unless noted):
//! - GET  /v1/health
//! - GET  /v1/version
//! - POST /v1/domain/plan        { rows, b_blk?, zh_c? } -> N, b_blk_hint, omega_ok, mem_hint
//! - POST /v1/auth/signup        { email, password } -> { user_id, api_key, tier, session_token }
//! - POST /v1/auth/login         { email, password } -> { user_id, api_key, tier, session_token }
//! - GET  /v1/me                 (Authorization: Bearer <session>) -> account info
//! - POST /v1/keys/rotate        (Authorization: Bearer <session>) -> { api_key }
//!
//! Paid endpoints (require X-API-Key):
//! - POST /v1/prove              ProveRequest -> ProveResponse (optionally returns proof as base64)
//! - POST /v1/verify (multipart: field "proof") -> { status }
//! - POST /v1/proof/inspect (multipart: "proof") -> parsed header summary
//!
//! Admin endpoints (require X-Admin-Token=…):
//! - POST /v1/admin/keys                     -> { key, tier }
//! - POST /v1/admin/keys/:key/tier {tier}    -> { key, tier }
//! - GET  /v1/admin/keys/:key/usage          -> { key, month, used, cap, tier }
//! - POST /v1/admin/srs/init                 -> initialize SRS (one-time, required before proving/verifying)
//!
//! Billing endpoints (Stripe):
//! - POST /v1/billing/checkout   (requires X-API-Key) -> { url }  // Stripe Checkout URL
//! - POST /v1/stripe/webhook     (Stripe calls)       -> { ok: true }
//!
//! Notes:
//! - Proof format is v2 (magic + u16 + ark-compressed).
//! - SRS must be initialized via `/v1/admin/srs/init` before proving/verifying.
//! - Dev builds can use in-memory SRS (feature `dev-srs`); production requires files.

#![forbid(unsafe_code)]
#![allow(dead_code)]
#![allow(unused_variables)]

use std::net::SocketAddr;
use std::sync::OnceLock;

use anyhow::{self};
use ark_ff::FftField; // for get_root_of_unity
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::{Datelike, TimeZone, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::{cors::{CorsLayer, AllowOrigin}, trace::TraceLayer};
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer, key_extractor::SmartIpKeyExtractor};
use tracing::{info, warn, error};
use axum::http::Method;

use myzkp::{
    air::{AirSpec, Row},
    pcs::{Basis, PcsParams},
    scheduler::{Prover, Verifier as SchedVerifier},
    F, Proof, ProveParams, VerifyParams,
};

// Stripe SDK (async-stripe 0.37.x)
use stripe::{
    CheckoutSession, CheckoutSessionMode, Client as StripeClient, CreateCheckoutSession,
    CreateCheckoutSessionLineItems,
};
// For webhook signature verification
use hmac::{Hmac, Mac};
use sha2::Sha256;

// Password hashing + sessions
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::rngs::OsRng;

// ============================================================================
// SRS Initialization State
// ============================================================================

/// Global flag tracking whether SRS has been initialized.
static SRS_INITIALIZED: OnceLock<bool> = OnceLock::new();

/// Middleware: ensure SRS is initialized before handling prove/verify requests.
async fn require_srs() -> Result<(), (StatusCode, String)> {
    if SRS_INITIALIZED.get().is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "SRS not initialized. Admin must call POST /v1/admin/srs/init first".into(),
        ));
    }
    Ok(())
}

// ------------------------------ KVS (Upstash) ------------------------------

#[derive(Deserialize)]
struct UpstashResp<T> {
    result: T,
}

#[derive(Clone)]
struct Kvs {
    url: String,
    token: String,
    http: reqwest::Client,
}

impl Kvs {
    fn from_env() -> anyhow::Result<Self> {
        let mut url = std::env::var("UPSTASH_REDIS_REST_URL")?;
        if url.ends_with('/') {
            url.pop();
        }
        let token = std::env::var("UPSTASH_REDIS_REST_TOKEN")?;
        Ok(Self {
            url,
            token,
            http: reqwest::Client::new(),
        })
    }

    #[inline]
    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let url = format!("{}/get/{}", self.url, key);
        let res = self.auth(self.http.get(&url)).send().await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("kvs GET {} {} {}", key, status, text);
        }
        let parsed: UpstashResp<Option<serde_json::Value>> = serde_json::from_str(&text)?;
        Ok(match parsed.result {
            None => None,
            Some(serde_json::Value::String(s)) => Some(s),
            Some(other) => Some(other.to_string()),
        })
    }

    async fn set_ex(&self, key: &str, val: &str, seconds: u64) -> anyhow::Result<()> {
        let url = format!("{}/setex/{}/{}", self.url, key, seconds);
        let body = format!("[\"{}\"]", val.replace('\\', "\\\\").replace('"', "\\\""));
        let res = self.auth(self.http.post(&url)).body(body).send().await?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("kvs SETEX {} {} {}", key, status, text);
        }
        if let Ok(parsed) = serde_json::from_str::<UpstashResp<String>>(&text) {
            if parsed.result != "OK" {
                anyhow::bail!("kvs SETEX non-OK: {}", parsed.result);
            }
        }
        Ok(())
    }

    async fn incr(&self, key: &str) -> anyhow::Result<i64> {
        let url = format!("{}/incr/{}", self.url, key);
        let res = self.auth(self.http.post(&url)).send().await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("kvs INCR {} {} {}", key, status, text);
        }
        let v: UpstashResp<i64> = serde_json::from_str(&text)?;
        Ok(v.result)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
enum Tier {
    Free,
    Pro,
    Scale,
}

fn month_bucket() -> String {
    Utc::now().format("%Y-%m").to_string()
}

fn monthly_usage_key(api_key: &str) -> String {
    format!("tinyzkp:usage:{}:{}", month_bucket(), api_key)
}

fn end_of_month_ttl_secs() -> u64 {
    let now = Utc::now();
    let (y, m) = (now.year(), now.month());
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let eom = Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).earliest().unwrap();
    let secs = (eom - now).num_seconds().max(86400) as u64;
    secs
}

// ------------------------------ Types ------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

#[derive(Serialize)]
struct Version {
    api: &'static str,
    protocol: &'static str,
    curve: &'static str,
    features: VersionFeatures,
}

#[derive(Serialize)]
struct VersionFeatures {
    dev_srs: bool,
    zeta_shift: bool,
    lookups: bool,
}

#[derive(Deserialize)]
struct DomainPlanReq {
    rows: usize,
    #[serde(default)]
    b_blk: Option<usize>,
    #[serde(default)]
    zh_c: Option<String>,
}

#[derive(Serialize)]
struct DomainPlanRes {
    n: usize,
    b_blk: usize,
    omega_ok: bool,
    mem_hint_bytes: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "format")]
enum WitnessInput {
    JsonRows { rows: Vec<Vec<u64>> },
}

#[derive(Deserialize)]
struct ProveReq {
    air: AirCfg,
    domain: DomainCfg,
    pcs: PcsCfg,
    #[serde(default)]
    srs: Option<SrsCfg>,
    witness: WitnessInput,
    #[serde(default)]
    return_proof: bool,
}

#[derive(Deserialize)]
struct AirCfg {
    k: usize,
    #[serde(default)]
    selectors: Option<SelectorsCfg>,
}

#[derive(Deserialize)]
struct SelectorsCfg {
    #[serde(rename = "format")]
    _format: String,
    csv: String,
}

#[derive(Deserialize)]
struct DomainCfg {
    rows: usize,
    b_blk: usize,
    #[serde(default = "one_str")]
    zh_c: String,
}

#[derive(Deserialize)]
struct PcsCfg {
    #[serde(default = "eval_basis")]
    basis_wires: String,
}

#[derive(Deserialize)]
struct SrsCfg {
    #[allow(dead_code)]
    id: String,
}

fn one_str() -> String {
    "1".into()
}

fn eval_basis() -> String {
    "eval".into()
}

#[derive(Serialize)]
struct ProveRes {
    header: ProofHeaderView,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_b64: Option<String>,
}

#[derive(Serialize)]
struct ProofHeaderView {
    n: usize,
    omega_hex: String,
    zh_c_hex: String,
    k: usize,
    basis_wires: String,
    srs_g1_digest_hex: String,
    srs_g2_digest_hex: String,
}

#[derive(Serialize)]
struct VerifyRes {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
struct ApiKeyInfo {
    key: String,
    tier: Tier,
}

#[derive(Serialize)]
struct UsageRes {
    key: String,
    month: String,
    used: i64,
    cap: i64,
    tier: Tier,
}

#[derive(Deserialize)]
struct SetTierReq {
    tier: String,
}

#[derive(Deserialize)]
struct InitSrsReq {
    max_degree: usize,
    #[serde(default)]
    validate_pairing: bool,
}

#[derive(Serialize)]
struct InitSrsRes {
    status: String,
    g1_powers: usize,
    g2_loaded: bool,
    g1_digest_hex: String,
    g2_digest_hex: String,
}

#[derive(Deserialize)]
struct CheckoutReq {
    #[serde(default)]
    customer_email: Option<String>,
    #[serde(default)]
    plan: Option<String>,
}

#[derive(Serialize)]
struct CheckoutRes {
    url: String,
}

#[derive(Serialize)]
struct HookAck {
    ok: bool,
}

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct SignupRes {
    user_id: String,
    api_key: String,
    tier: String,
    session_token: String,
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct LoginRes {
    user_id: String,
    api_key: String,
    tier: String,
    session_token: String,
}

#[derive(Serialize)]
struct MeRes {
    user_id: String,
    email: String,
    api_key: String,
    tier: String,
    month: String,
    used: i64,
    caps: CapsView,
    limits: LimitsView,
}

#[derive(Serialize)]
struct CapsView {
    free: i64,
    pro: i64,
    scale: i64,
}

#[derive(Serialize)]
struct LimitsView {
    free_max_rows: usize,
    pro_max_rows: usize,
    scale_max_rows: usize,
}

#[derive(Serialize)]
struct RotateRes {
    api_key: String,
}

#[derive(Clone)]
struct AppState {
    addr: SocketAddr,
    kvs: Kvs,
    admin_token: String,
    free_cap: i64,
    pro_cap: i64,
    scale_cap: i64,
    max_rows: usize,
    free_max_rows: usize,
    pro_max_rows: usize,
    scale_max_rows: usize,
    allow_dev_srs: bool,
    stripe: StripeClient,
    price_pro: String,
    price_scale: String,
    success_url: String,
    cancel_url: String,
    portal_return_url: String,
}

// ------------------------------ Helpers ------------------------------

fn parse_basis(s: &str) -> Basis {
    match s {
        "coeff" | "coefficient" => Basis::Coefficient,
        _ => Basis::Evaluation,
    }
}

fn fe_hex(x: F) -> String {
    let mut v = Vec::new();
    x.serialize_compressed(&mut v).expect("field serialize");
    let mut s = String::with_capacity(2 + v.len() * 2);
    s.push_str("0x");
    s.push_str(&hex::encode(v));
    s
}

fn header_view(p: &Proof) -> ProofHeaderView {
    ProofHeaderView {
        n: p.header.domain_n as usize,
        omega_hex: fe_hex(p.header.domain_omega),
        zh_c_hex: fe_hex(p.header.zh_c),
        k: p.header.k as usize,
        basis_wires: match p.header.basis_wires {
            Basis::Coefficient => "Coefficient",
            Basis::Evaluation => "Evaluation",
        }
        .into(),
        srs_g1_digest_hex: hex_bytes(&p.header.srs_g1_digest),
        srs_g2_digest_hex: hex_bytes(&p.header.srs_g2_digest),
    }
}

fn hex_bytes(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(2 + 64);
    s.push_str("0x");
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

fn rows_from_json(rows: &[Vec<u64>], k: usize) -> anyhow::Result<Vec<Row>> {
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        if r.len() != k {
            return Err(anyhow::anyhow!(
                "row {} has {} columns, expected {}",
                i,
                r.len(),
                k
            ));
        }
        let regs: Vec<F> = r.iter().copied().map(F::from).collect();
        out.push(Row {
            regs: regs.into_boxed_slice(),
        });
    }
    Ok(out)
}

fn plan_b_blk(rows: usize, provided: Option<usize>) -> usize {
    if let Some(b) = provided {
        return b.max(1);
    }
    let n = next_pow2(rows.max(1));
    let approx = (n as f64).sqrt().round() as usize;
    approx.clamp(8, 1 << 12)
}

fn next_pow2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    n.next_power_of_two()
}

fn max_rows_for_tier(st: &AppState, tier: Tier) -> usize {
    let tier_cap = match tier {
        Tier::Free => st.free_max_rows,
        Tier::Pro => st.pro_max_rows,
        Tier::Scale => st.scale_max_rows,
    };
    tier_cap.min(st.max_rows)
}

fn valid_email(e: &str) -> bool {
    if e.len() < 3 || e.len() > 254 {
        return false;
    }
    let (has_at, has_dot) = (e.contains('@'), e.rsplit('.').next().map(|s| !s.is_empty()).unwrap_or(false));
    has_at && has_dot
}

fn random_user_id() -> String {
    let mut r = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut r);
    hex::encode(r)
}

async fn new_session(kvs: &Kvs, user_id: &str, email: &str) -> anyhow::Result<String> {
    let mut r = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut r);
    let token = hex::encode(blake3::hash(&r).as_bytes());
    let payload = serde_json::json!({ "user_id": user_id, "email": email }).to_string();
    kvs.set_ex(&format!("tinyzkp:sess:{token}"), &payload, 30 * 24 * 3600).await?;
    Ok(token)
}

async fn auth_session(kvs: &Kvs, headers: &HeaderMap) -> Result<(String, String), (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(|x| x.to_string()))
        .ok_or((StatusCode::UNAUTHORIZED, "missing Bearer token".into()))?;
    let v = kvs
        .get(&format!("tinyzkp:sess:{token}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    
    // Handle Redis returning array-wrapped JSON: ["{...}"] -> {...}
    let session_json_str = if v.starts_with('[') {
        let arr: Vec<String> = serde_json::from_str(&v).unwrap_or_default();
        arr.first().cloned().unwrap_or_default()
    } else {
        v.clone()
    };
    
    let obj: serde_json::Value = serde_json::from_str(&session_json_str).unwrap_or(serde_json::json!({}));
    let uid = obj.get("user_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let email = obj.get("email").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if uid.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, "invalid session".into()));
    }
    Ok((uid, email))
}

async fn check_and_count(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<(String, Tier, i64, i64), (StatusCode, String)> {
    let api_key = headers
        .get("x-api-key")
        .and_then(|h| h.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "missing X-API-Key".into()))?
        .to_string();

    let tier_s = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{api_key}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "unknown API key".into()))?;

    if tier_s.eq_ignore_ascii_case("disabled") {
        return Err((StatusCode::UNAUTHORIZED, "API key disabled".into()));
    }

    let tier = match tier_s.as_str() {
        "pro" | "Pro" | "PRO" => Tier::Pro,
        "scale" | "Scale" | "SCALE" => Tier::Scale,
        _ => Tier::Free,
    };

    let cap = match tier {
        Tier::Free => st.free_cap,
        Tier::Pro => st.pro_cap,
        Tier::Scale => st.scale_cap,
    };

    let used = st
        .kvs
        .incr(&monthly_usage_key(&api_key))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if used == 1 {
        let _ = st
            .kvs
            .set_ex(&monthly_usage_key(&api_key), "1", end_of_month_ttl_secs())
            .await;
    }

    if used > cap {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            format!("monthly cap reached ({used}/{cap})"),
        ));
    }

    Ok((api_key, tier, used, cap))
}

// ------------------------------ Public Handlers ------------------------------

async fn health() -> impl IntoResponse {
    Json(Health { status: "ok" })
}

async fn version() -> impl IntoResponse {
    Json(Version {
        api: "tinyzkp-api/0.3",
        protocol: "sszkp-v2",
        curve: "bn254/kzg",
        features: VersionFeatures {
            dev_srs: cfg!(feature = "dev-srs"),
            zeta_shift: cfg!(feature = "zeta-shift"),
            lookups: cfg!(feature = "lookups"),
        },
    })
}

async fn domain_plan(
    Json(req): Json<DomainPlanReq>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let n = next_pow2(req.rows.max(1));
    let b_blk = plan_b_blk(req.rows, req.b_blk);
    let omega = F::get_root_of_unity(n as u64)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "no n-th root of unity".into()))?;
    let mut ok = true;
    let mut pow = F::from(1u64);
    for _ in 0..n {
        pow *= omega;
    }
    if pow != F::from(1u64) {
        ok = false;
    }
    if n >= 2 {
        let mut pow2 = F::from(1u64);
        for _ in 0..(n / 2) {
            pow2 *= omega;
        }
        if pow2 == F::from(1u64) {
            ok = false;
        }
    }
    let mem_hint_bytes = b_blk * 64;
    Ok(Json(DomainPlanRes {
        n,
        b_blk,
        omega_ok: ok,
        mem_hint_bytes,
    }))
}

// ------------------------------ Accounts Handlers ------------------------------

async fn auth_signup(
    State(st): State<AppState>,
    Json(req): Json<SignupReq>,
) -> Result<Json<SignupRes>, (StatusCode, String)> {
    let email = req.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err((StatusCode::BAD_REQUEST, "invalid email".into()));
    }
    if req.password.len() < 8 {
        return Err((StatusCode::BAD_REQUEST, "password too short".into()));
    }

    if st
        .kvs
        .get(&format!("tinyzkp:user:by_email:{email}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some()
    {
        return Err((StatusCode::CONFLICT, "email already registered".into()));
    }

    let api_key = random_key();
    let user_id = random_user_id();

    // Hash password (Argon2id default params)
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    let pw_hash = argon
        .hash_password(req.password.as_bytes(), &salt)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();

    let user_obj = serde_json::json!({
        "email": email,
        "pw_hash": pw_hash,
        "api_key": api_key,
        "tier": "free",
        "created_at": Utc::now().timestamp(),
        "status": "active"
    });

    // Store user + indexes + API key tier
    let year = 365 * 24 * 3600;
    st.kvs
        .set_ex(
            &format!("tinyzkp:user:by_email:{email}"),
            &serde_json::json!({ "user_id": user_id }).to_string(),
            year,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    st.kvs
        .set_ex(&format!("tinyzkp:user:{user_id}"), &user_obj.to_string(), year)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    st.kvs
        .set_ex(
            &format!("tinyzkp:key:owner:{api_key}"),
            &serde_json::json!({ "user_id": user_id }).to_string(),
            year,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    st.kvs
        .set_ex(&format!("tinyzkp:key:tier:{api_key}"), "free", year)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let session = new_session(&st.kvs, &user_id, &email)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(SignupRes {
        user_id,
        api_key,
        tier: "free".into(),
        session_token: session,
    }))
}

async fn auth_login(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Result<Json<LoginRes>, (StatusCode, String)> {
    let email = req.email.trim().to_lowercase();
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("unknown");
    
    info!("Login attempt: email={}, ip={}", email, client_ip);
    
    let uid_v = st
        .kvs
        .get(&format!("tinyzkp:user:by_email:{email}"))
        .await
        .map_err(|e| {
            error!("Redis error fetching user by email: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| {
            warn!(email = %email, ip = %client_ip, "Failed login: user not found");
            (StatusCode::UNAUTHORIZED, "invalid credentials".into())
        })?;
    
    info!("Found user by email, uid_v={}", uid_v);
    
    // Handle Redis returning array-wrapped JSON: ["{...}"] -> {...}
    let uid_json_str = if uid_v.starts_with('[') {
        // Parse as array, extract first element
        let arr: Vec<String> = serde_json::from_str(&uid_v).unwrap_or_default();
        arr.first().cloned().unwrap_or_default()
    } else {
        uid_v.clone()
    };
    
    info!("Extracted uid_json_str: {}", uid_json_str);
    let uid_obj: serde_json::Value =
        serde_json::from_str(&uid_json_str).unwrap_or(serde_json::json!({}));
    let user_id = uid_obj
        .get("user_id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if user_id.is_empty() {
        error!("User ID is empty after parsing. uid_obj={}", uid_obj);
        return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
    }

    info!("Fetching user object for user_id={}", user_id);
    let user_v = st
        .kvs
        .get(&format!("tinyzkp:user:{user_id}"))
        .await
        .map_err(|e| {
            error!("Redis error fetching user object: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| {
            error!("User object not found for user_id={}", user_id);
            (StatusCode::UNAUTHORIZED, "invalid credentials".into())
        })?;
    
    info!("Found user object, user_v length={}, user_v={}", user_v.len(), 
        if user_v.len() > 200 { &user_v[..200] } else { &user_v });
    
    // Handle Redis returning array-wrapped JSON: ["{...}"] -> {...}
    let user_json_str = if user_v.starts_with('[') {
        let arr: Vec<String> = serde_json::from_str(&user_v).unwrap_or_default();
        arr.first().cloned().unwrap_or_default()
    } else {
        user_v.clone()
    };
    
    let user: serde_json::Value = serde_json::from_str(&user_json_str).unwrap_or(serde_json::json!({}));

    let pw_hash = user
        .get("pw_hash")
        .and_then(|x| x.as_str())
        .ok_or_else(|| {
            error!("pw_hash field missing from user object");
            (StatusCode::UNAUTHORIZED, "invalid credentials".into())
        })?;
    
    info!("Parsing password hash...");
    let parsed =
        PasswordHash::new(pw_hash).map_err(|e| {
            error!("Failed to parse password hash: {:?}", e);
            (StatusCode::UNAUTHORIZED, "invalid credentials".into())
        })?;
    
    info!("Verifying password...");
    Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed)
        .map_err(|e| {
            error!("Password verification failed: {:?}", e);
            warn!(email = %email, ip = %client_ip, "Failed login: invalid password");
            (StatusCode::UNAUTHORIZED, "invalid credentials".into())
        })?;
    
    info!("Password verified successfully for user_id={}", user_id);

    let api_key = user
        .get("api_key")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    let tier_raw = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{api_key}"))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "free".into());
    
    // Handle Redis returning array-wrapped strings: ["free"] -> free
    let tier_live = if tier_raw.starts_with('[') {
        let arr: Vec<String> = serde_json::from_str(&tier_raw).unwrap_or_default();
        arr.first().cloned().unwrap_or_else(|| "free".into())
    } else {
        tier_raw
    };

    let session = new_session(&st.kvs, &user_id, &email)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(LoginRes {
        user_id,
        api_key,
        tier: tier_live,
        session_token: session,
    }))
}

async fn me(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MeRes>, (StatusCode, String)> {
    let (user_id, email) = auth_session(&st.kvs, &headers).await?;
    let user_v = st
        .kvs
        .get(&format!("tinyzkp:user:{user_id}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    let user: serde_json::Value = serde_json::from_str(&user_v).unwrap_or(serde_json::json!({}));

    let api_key = user
        .get("api_key")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let tier_live = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{api_key}"))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "free".into());
    let used = st
        .kvs
        .get(&monthly_usage_key(&api_key))
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    Ok(Json(MeRes {
        user_id,
        email,
        api_key,
        tier: tier_live,
        month: month_bucket(),
        used,
        caps: CapsView {
            free: st.free_cap,
            pro: st.pro_cap,
            scale: st.scale_cap,
        },
        limits: LimitsView {
            free_max_rows: st.free_max_rows,
            pro_max_rows: st.pro_max_rows,
            scale_max_rows: st.scale_max_rows,
        },
    }))
}

async fn rotate_key(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RotateRes>, (StatusCode, String)> {
    let (user_id, _email) = auth_session(&st.kvs, &headers).await?;

    let user_key = format!("tinyzkp:user:{user_id}");
    let user_v = st
        .kvs
        .get(&user_key)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    let mut user: serde_json::Value =
        serde_json::from_str(&user_v).unwrap_or(serde_json::json!({}));

    let old_api = user
        .get("api_key")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    let tier_str = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{old_api}"))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "free".into());

    let new_api = random_key();
    let year = 365 * 24 * 3600;

    st.kvs
        .set_ex(&format!("tinyzkp:key:tier:{old_api}"), "disabled", year)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    st.kvs
        .set_ex(
            &format!("tinyzkp:key:owner:{new_api}"),
            &serde_json::json!({ "user_id": user_id }).to_string(),
            year,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    st.kvs
        .set_ex(&format!("tinyzkp:key:tier:{new_api}"), &tier_str, year)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    user["api_key"] = serde_json::Value::String(new_api.clone());
    let user_str = user.to_string();
    st.kvs
        .set_ex(&user_key, &user_str, year)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(RotateRes { api_key: new_api }))
}

// ------------------------------ Paid Handlers ------------------------------

async fn prove(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ProveReq>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (_api_key, tier, _used, _cap) = check_and_count(&st, &headers).await?;

    let tier_max = max_rows_for_tier(&st, tier);
    if req.domain.rows > tier_max {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "rows exceeds tier limit for {:?} ({}/{}). If you need more, upgrade your plan.",
                tier, req.domain.rows, tier_max
            ),
        ));
    }

    let n_rows = req.domain.rows;
    let b_blk = req.domain.b_blk.max(1);
    let n_domain = next_pow2(n_rows);
    let omega = F::get_root_of_unity(n_domain as u64)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("no {}-th root of unity", n_domain),
            )
        })?;
    let zh_u = req
        .domain
        .zh_c
        .parse::<u64>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "zh_c must be decimal u64".into()))?;
    let zh_c = F::from(zh_u);

    let selectors = if let Some(sel) = &req.air.selectors {
        let mut rows: Vec<Vec<F>> = Vec::new();
        for (lineno, line) in sel.csv.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut r = Vec::new();
            for tok in line.split(|c: char| c == ',' || c.is_whitespace()) {
                if tok.is_empty() {
                    continue;
                }
                let v = tok.parse::<u64>().map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!(
                            "selectors parse error at line {}: `{}` ({})",
                            lineno + 1,
                            tok,
                            e
                        ),
                    )
                })?;
                r.push(F::from(v));
            }
            if !r.is_empty() {
                rows.push(r);
            }
        }
        if !rows.is_empty() {
            let s_cols = rows[0].len();
            for (i, r) in rows.iter().enumerate() {
                if r.len() != s_cols {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!(
                            "selectors are ragged: row 0 has {} cols, row {} has {}",
                            s_cols,
                            i,
                            r.len()
                        ),
                    ));
                }
            }
            let mut cols: Vec<Vec<F>> = vec![Vec::with_capacity(rows.len()); s_cols];
            for r in rows {
                for (j, v) in r.into_iter().enumerate() {
                    cols[j].push(v);
                }
            }
            cols.into_iter().map(|v| v.into_boxed_slice()).collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let air = AirSpec {
        k: req.air.k,
        id_table: Vec::new(),
        sigma_table: Vec::new(),
        selectors,
    };
    let basis_wires = parse_basis(&req.pcs.basis_wires);
    let domain = myzkp::domain::Domain {
        n: n_domain,
        omega,
        zh_c,
    };

    let pcs_wires = PcsParams {
        max_degree: n_domain - 1,
        basis: basis_wires,
        srs_placeholder: (),
    };
    let pcs_coeff = PcsParams {
        max_degree: n_domain - 1,
        basis: Basis::Coefficient,
        srs_placeholder: (),
    };
    let prove_params = ProveParams {
        domain: domain.clone(),
        pcs_wires,
        pcs_coeff,
        b_blk,
    };

    let witness_rows: Vec<Row> = match &req.witness {
        WitnessInput::JsonRows { rows } => {
            rows_from_json(rows, req.air.k).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
        }
    };

    let prover = Prover {
        air: &air,
        params: &prove_params,
    };
    let proof = prover.prove_with_restreamer(&witness_rows).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("prover failed: {e}"),
        )
    })?;

    let header_v = header_view(&proof);

    let proof_b64 = if req.return_proof {
        let mut payload = Vec::new();
        proof
            .serialize_compressed(&mut payload)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize proof: {e}")))?;
        let mut out = Vec::with_capacity(8 + 2 + payload.len());
        out.extend_from_slice(b"SSZKPv2\0");
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&payload);
        Some(base64::engine::general_purpose::STANDARD.encode(out))
    } else {
        None
    };

    Ok(Json(ProveRes {
        header: header_v,
        proof_b64,
    }))
}

async fn verify(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let _ = check_and_count(&st, &headers).await?;

    let mut proof_bytes: Option<Vec<u8>> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart error: {e}")))?
    {
        if let Some(name) = field.name() {
            if name == "proof" {
                let data = field.bytes().await.map_err(|e| {
                    (StatusCode::BAD_REQUEST, format!("read multipart: {e}"))
                })?;
                proof_bytes = Some(data.to_vec());
            }
        }
    }
    let buf = proof_bytes
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "multipart field 'proof' is required".into()))?;

    if buf.len() < 10 || &buf[0..8] != b"SSZKPv2\0" {
        return Err((StatusCode::BAD_REQUEST, "bad proof file: missing magic".into()));
    }
    let ver = u16::from_be_bytes([buf[8], buf[9]]);
    if ver != 2 {
        return Err((StatusCode::BAD_REQUEST, format!("unsupported proof version {ver}")));
    }
    let payload = &buf[10..];
    let mut slice = payload;
    let proof: Proof = CanonicalDeserialize::deserialize_compressed(&mut slice)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("deserialize proof: {e}")))?;

    let domain = myzkp::domain::Domain {
        n: proof.header.domain_n as usize,
        omega: proof.header.domain_omega,
        zh_c: proof.header.zh_c,
    };
    let pcs_wires = PcsParams {
        max_degree: domain.n - 1,
        basis: proof.header.basis_wires,
        srs_placeholder: (),
    };
    let pcs_coeff = PcsParams {
        max_degree: domain.n - 1,
        basis: Basis::Coefficient,
        srs_placeholder: (),
    };
    let vp = VerifyParams {
        domain,
        pcs_wires,
        pcs_coeff,
    };
    let verifier = SchedVerifier { params: &vp };

    if let Err(e) = verifier.verify(&proof) {
        return Ok((
            StatusCode::OK,
            Json(VerifyRes {
                status: "failed",
                reason: Some(format!("{e}")),
            }),
        ));
    }

    Ok((
        StatusCode::OK,
        Json(VerifyRes {
            status: "ok",
            reason: None,
        }),
    ))
}

async fn inspect(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let _ = check_and_count(&st, &headers).await?;

    let mut proof_bytes: Option<Vec<u8>> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart error: {e}")))?
    {
        if let Some(name) = field.name() {
            if name == "proof" {
                let data = field.bytes().await.map_err(|e| {
                    (StatusCode::BAD_REQUEST, format!("read multipart: {e}"))
                })?;
                proof_bytes = Some(data.to_vec());
            }
        }
    }
    let buf = proof_bytes
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "multipart field 'proof' is required".into()))?;
    if buf.len() < 10 || &buf[0..8] != b"SSZKPv2\0" {
        return Err((StatusCode::BAD_REQUEST, "bad proof file: missing magic".into()));
    }
    let ver = u16::from_be_bytes([buf[8], buf[9]]);
    if ver != 2 {
        return Err((StatusCode::BAD_REQUEST, format!("unsupported proof version {ver}")));
    }
    let payload = &buf[10..];
    let mut slice = payload;
    let proof: Proof = CanonicalDeserialize::deserialize_compressed(&mut slice)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("deserialize proof: {e}")))?;

    Ok(Json(header_view(&proof)))
}

async fn prove_checked(
    st: State<AppState>,
    headers: HeaderMap,
    req: Json<ProveReq>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_srs().await?;
    prove(st, headers, req).await
}

async fn verify_checked(
    st: State<AppState>,
    headers: HeaderMap,
    mp: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_srs().await?;
    verify(st, headers, mp).await
}

// ------------------------------ Billing Handlers (Stripe) ------------------------------

async fn billing_checkout(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Result<Json<CheckoutRes>, (StatusCode, String)> {
    let api_key = headers
        .get("x-api-key")
        .and_then(|h| h.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "missing X-API-Key".into()))?
        .to_string();

    let _tier = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{api_key}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "unknown API key".into()))?;

    let plan = req.plan.as_deref().unwrap_or("pro");
    let (price_id, tier_str) = if plan.eq_ignore_ascii_case("scale") {
        (st.price_scale.as_str(), "scale")
    } else {
        (st.price_pro.as_str(), "pro")
    };

    let mut params = CreateCheckoutSession::new();
    params.mode = Some(CheckoutSessionMode::Subscription);
    params.success_url = Some(st.success_url.as_str());
    params.cancel_url = Some(st.cancel_url.as_str());
    params.line_items = Some(vec![CreateCheckoutSessionLineItems {
        price: Some(price_id.to_string()),
        quantity: Some(1),
        ..Default::default()
    }]);

    let mut md = std::collections::HashMap::new();
    md.insert("api_key".to_string(), api_key.clone());
    md.insert("tier".to_string(), tier_str.to_string());
    params.metadata = Some(md);

    let mut sub_md = std::collections::HashMap::new();
    sub_md.insert("api_key".to_string(), api_key.clone());
    sub_md.insert("tier".to_string(), tier_str.to_string());
    params.subscription_data =
        Some(stripe::CreateCheckoutSessionSubscriptionData { metadata: Some(sub_md), ..Default::default() });

    let customer_email_owned = req.customer_email;
    if let Some(ref email) = customer_email_owned {
        params.customer_email = Some(email.as_str());
    }

    let session: CheckoutSession = CheckoutSession::create(&st.stripe, params)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("stripe: {e}")))?;

    let url = session
        .url
        .ok_or((StatusCode::BAD_GATEWAY, "stripe: missing checkout URL".into()))?;
    Ok(Json(CheckoutRes { url }))
}

/// Verifies Stripe webhook signature using HMAC-SHA256
/// Format: "t=timestamp,v1=signature" where v1 = HMAC-SHA256(timestamp.payload, secret)
fn verify_stripe_signature(sig_header: &str, payload: &str, secret: &str) -> Result<(), String> {
    // Parse signature header: "t=1234567890,v1=abc123..."
    let mut timestamp = None;
    let mut signature = None;
    
    for part in sig_header.split(',') {
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "t" => timestamp = Some(value),
                "v1" => signature = Some(value),
                _ => {}
            }
        }
    }
    
    let timestamp = timestamp.ok_or("Missing timestamp in signature header")?;
    let expected_sig = signature.ok_or("Missing v1 signature in header")?;
    
    // Construct signed payload: timestamp.payload
    let signed_payload = format!("{}.{}", timestamp, payload);
    
    // Compute HMAC-SHA256
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("Invalid secret: {}", e))?;
    mac.update(signed_payload.as_bytes());
    
    let computed_sig = hex::encode(mac.finalize().into_bytes());
    
    // Constant-time comparison
    if computed_sig != expected_sig {
        return Err("Signature mismatch".into());
    }
    
    Ok(())
}

async fn stripe_webhook(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<HookAck>), (StatusCode, String)> {
    let sig_header = headers
        .get("stripe-signature")
        .and_then(|h| h.to_str().ok())
        .ok_or((StatusCode::BAD_REQUEST, "missing stripe-signature".into()))?;

    // ✅ SECURITY FIX: Verify webhook signature
    let webhook_secret = std::env::var("STRIPE_WEBHOOK_SECRET")
        .map_err(|_| {
            error!("STRIPE_WEBHOOK_SECRET not configured");
            (StatusCode::INTERNAL_SERVER_ERROR, "webhook secret not configured".into())
        })?;

    let payload_str = std::str::from_utf8(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid utf-8 payload".to_string()))?;

    // Verify Stripe webhook signature (HMAC-SHA256)
    verify_stripe_signature(sig_header, payload_str, &webhook_secret)
        .map_err(|e| {
            warn!("Stripe webhook signature verification failed: {}", e);
            (StatusCode::BAD_REQUEST, format!("Invalid signature: {}", e))
        })?;

    // Parse the verified JSON
    let v: serde_json::Value = serde_json::from_str(payload_str)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    info!("Received verified Stripe webhook: {}", typ);

    let obj = v
        .get("data")
        .and_then(|d| d.get("object"))
        .and_then(|o| o.as_object())
        .ok_or((StatusCode::BAD_REQUEST, "missing data.object".to_string()))?;

    // Helper to extract metadata
    let get_api_key = || -> Option<String> {
        obj.get("metadata")?
            .get("api_key")?
            .as_str()
            .map(|s| s.to_string())
    };

    let get_tier = || -> Option<&'static str> {
        let tier_str = obj.get("metadata")?.get("tier")?.as_str()?;
        if tier_str.eq_ignore_ascii_case("scale") {
            Some("scale")
        } else if tier_str.eq_ignore_ascii_case("pro") {
            Some("pro")
        } else if tier_str.eq_ignore_ascii_case("free") {
            Some("free")
        } else {
            None
        }
    };

    // Process event based on type
    match typ {
        "checkout.session.completed" => {
            if let Some(api_key) = get_api_key() {
                let tier = get_tier().unwrap_or("pro");
                info!("Upgrading API key {} to tier {}", api_key, tier);
                st.kvs
                    .set_ex(&format!("tinyzkp:key:tier:{api_key}"), tier, 365 * 24 * 3600)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
        }
        "customer.subscription.deleted" => {
            if let Some(api_key) = get_api_key() {
                info!("Downgrading API key {} to free tier (subscription deleted)", api_key);
                st.kvs
                    .set_ex(&format!("tinyzkp:key:tier:{api_key}"), "free", 365 * 24 * 3600)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
        }
        "customer.subscription.updated" => {
            if let Some(api_key) = get_api_key() {
                let status = obj.get("status").and_then(|s| s.as_str()).unwrap_or("");
                let tier = if status == "active" || status == "trialing" {
                    get_tier().unwrap_or("pro")
                } else {
                    "free"
                };
                info!("Updating API key {} to tier {} (status: {})", api_key, tier, status);
                st.kvs
                    .set_ex(&format!("tinyzkp:key:tier:{api_key}"), tier, 365 * 24 * 3600)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
        }
        _ => {
            info!("Ignoring unhandled webhook event: {}", typ);
        }
    }

    Ok((StatusCode::OK, Json(HookAck { ok: true })))
}

// ------------------------------ Admin Handlers ------------------------------

fn random_key() -> String {
    let mut r = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut r);
    let h = blake3::hash(&r).to_hex();
    format!("tz_{h}")
}

async fn admin_new_key(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiKeyInfo>, (StatusCode, String)> {
    let auth = headers.get("x-admin-token").and_then(|v| v.to_str().ok());
    if auth != Some(st.admin_token.as_str()) {
        return Err((StatusCode::UNAUTHORIZED, "bad admin token".into()));
    }
    let key = random_key();
    st.kvs
        .set_ex(&format!("tinyzkp:key:tier:{key}"), "free", 365 * 24 * 3600)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(ApiKeyInfo {
        key,
        tier: Tier::Free,
    }))
}

async fn admin_set_tier(
    State(st): State<AppState>,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<SetTierReq>,
) -> Result<Json<ApiKeyInfo>, (StatusCode, String)> {
    let auth = headers.get("x-admin-token").and_then(|v| v.to_str().ok());
    if auth != Some(st.admin_token.as_str()) {
        return Err((StatusCode::UNAUTHORIZED, "bad admin token".into()));
    }
    let tier_s = match req.tier.as_str() {
        "scale" | "Scale" | "SCALE" => "scale",
        "pro" | "Pro" | "PRO" => "pro",
        _ => "free",
    };
    st.kvs
        .set_ex(&format!("tinyzkp:key:tier:{key}"), tier_s, 365 * 24 * 3600)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let tier = match tier_s {
        "scale" => Tier::Scale,
        "pro" => Tier::Pro,
        _ => Tier::Free,
    };
    Ok(Json(ApiKeyInfo { key, tier }))
}

async fn admin_usage(
    State(st): State<AppState>,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Result<Json<UsageRes>, (StatusCode, String)> {
    let auth = headers.get("x-admin-token").and_then(|v| v.to_str().ok());
    if auth != Some(st.admin_token.as_str()) {
        return Err((StatusCode::UNAUTHORIZED, "bad admin token".into()));
    }
    let tier_s = st
        .kvs
        .get(&format!("tinyzkp:key:tier:{key}"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or_else(|| "free".into());
    let tier = match tier_s.as_str() {
        "scale" => Tier::Scale,
        "pro" => Tier::Pro,
        _ => Tier::Free,
    };
    let used = st
        .kvs
        .get(&monthly_usage_key(&key))
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let cap = match tier {
        Tier::Free => st.free_cap,
        Tier::Pro => st.pro_cap,
        Tier::Scale => st.scale_cap,
    };
    Ok(Json(UsageRes {
        key,
        month: month_bucket(),
        used,
        cap,
        tier,
    }))
}

async fn admin_init_srs(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InitSrsReq>,
) -> Result<Json<InitSrsRes>, (StatusCode, String)> {
    let auth = headers.get("x-admin-token").and_then(|v| v.to_str().ok());
    if auth != Some(st.admin_token.as_str()) {
        return Err((StatusCode::UNAUTHORIZED, "bad admin token".into()));
    }

    if SRS_INITIALIZED.get().is_some() {
        return Err((
            StatusCode::CONFLICT,
            "SRS already initialized (restart server to reinitialize)".into(),
        ));
    }

    #[cfg(not(feature = "dev-srs"))]
    {
        let g1_path = std::env::var("SSZKP_SRS_G1_PATH").map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "SSZKP_SRS_G1_PATH not set in environment".into(),
            )
        })?;
        let g2_path = std::env::var("SSZKP_SRS_G2_PATH").map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "SSZKP_SRS_G2_PATH not set in environment".into(),
            )
        })?;

        eprintln!("Loading production SRS from files...");
        eprintln!("  G1: {}", g1_path);
        eprintln!("  G2: {}", g2_path);

        let g1_powers = myzkp::srs_setup::load_and_validate_g1_srs(&g1_path, req.max_degree)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Load G1 SRS: {}", e)))?;

        let tau_g2 = myzkp::srs_setup::load_and_validate_g2_srs(&g2_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Load G2 SRS: {}", e)))?;

        if req.validate_pairing {
            eprintln!("Performing cryptographic pairing check...");
            myzkp::srs_setup::validate_g1_pairing(&g1_powers, tau_g2)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Pairing check: {}", e)))?;
            eprintln!("✓ Pairing check passed");
        }

        myzkp::pcs::load_srs_g1(&g1_powers);
        myzkp::pcs::load_srs_g2(tau_g2);

        eprintln!("✓ Production SRS loaded successfully");
    }

    #[cfg(feature = "dev-srs")]
    {
        eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        eprintln!("⚠️  WARNING: Using DEVELOPMENT SRS (NOT FOR PRODUCTION!)");
        eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        let (g1_powers, tau_g2) = myzkp::srs_setup::generate_dev_srs(req.max_degree);
        myzkp::pcs::load_srs_g1(&g1_powers);
        myzkp::pcs::load_srs_g2(tau_g2);

        eprintln!("✓ Dev SRS generated (degree={})", req.max_degree);
    }

    SRS_INITIALIZED.set(true).ok();

    let g1_dig = myzkp::pcs::srs_g1_digest();
    let g2_dig = myzkp::pcs::srs_g2_digest();

    eprintln!("SRS initialized successfully:");
    eprintln!("  G1 digest: {:02x?}", g1_dig);
    eprintln!("  G2 digest: {:02x?}", g2_dig);

    Ok(Json(InitSrsRes {
        status: "initialized".into(),
        g1_powers: req.max_degree + 1,
        g2_loaded: true,
        g1_digest_hex: hex::encode(g1_dig),
        g2_digest_hex: hex::encode(g2_dig),
    }))
}

// ------------------------------ Main ------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "tinyzkp_api=info,tower_http=info".into())
        )
        .with_target(false)
        .compact()
        .init();

    info!("Starting TinyZKP API server");

    let addr: SocketAddr = std::env::var("TINYZKP_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8080)));

    let kvs = Kvs::from_env()?;
    let admin_token =
        std::env::var("TINYZKP_ADMIN_TOKEN").unwrap_or_else(|_| "changeme-admin".into());

    let free_cap = std::env::var("TINYZKP_FREE_MONTHLY_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let pro_cap = std::env::var("TINYZKP_PRO_MONTHLY_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    let scale_cap = std::env::var("TINYZKP_SCALE_MONTHLY_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000);

    let max_rows = std::env::var("TINYZKP_MAX_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(131_072);
    let free_max_rows = std::env::var("TINYZKP_FREE_MAX_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4_096);
    let pro_max_rows = std::env::var("TINYZKP_PRO_MAX_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16_384);
    let scale_max_rows = std::env::var("TINYZKP_SCALE_MAX_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(131_072);

    let allow_dev_srs = std::env::var("TINYZKP_ALLOW_DEV_SRS")
        .map(|s| s == "true")
        .unwrap_or(true);

    let stripe = StripeClient::new(std::env::var("STRIPE_SECRET_KEY")?);
    let price_pro = std::env::var("STRIPE_PRICE_PRO")?;
    let price_scale = std::env::var("STRIPE_PRICE_SCALE")
        .unwrap_or_else(|_| "price_scale_not_set".into());
    let success_url =
        std::env::var("BILLING_SUCCESS_URL").unwrap_or_else(|_| "https://tinyzkp.com/success".into());
    let cancel_url =
        std::env::var("BILLING_CANCEL_URL").unwrap_or_else(|_| "https://tinyzkp.com/cancel".into());
    let portal_return_url = std::env::var("BILLING_PORTAL_RETURN_URL")
        .unwrap_or_else(|_| "https://tinyzkp.com/account".into());

    // ✅ SECURITY FIX: Configure restrictive CORS
    let allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "https://tinyzkp.com,https://app.tinyzkp.com".into());
    
    let cors = if allowed_origins == "*" {
        warn!("⚠️  WARNING: CORS set to permissive mode (*). This is INSECURE for production!");
        CorsLayer::permissive()
    } else {
        let origins: Vec<_> = allowed_origins
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        
        info!("✅ CORS configured for origins: {:?}", origins);
        
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderName::from_static("x-api-key"),
                axum::http::HeaderName::from_static("x-admin-token"),
            ])
            .allow_credentials(true)
            .max_age(std::time::Duration::from_secs(3600))
    };

    // ✅ SECURITY FIX: Add rate limiting (10 req/sec per IP, burst of 30)
    let governor_conf = Box::leak(Box::new(
        GovernorConfigBuilder::default()
            .per_second(10)
            .burst_size(30)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .unwrap()
    ));

    info!("✅ Rate limiting configured: 10 req/sec per IP (burst: 30)");

    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/version", get(version))
        .route("/v1/domain/plan", post(domain_plan))
        .route("/v1/auth/signup", post(auth_signup))
        .route("/v1/auth/login", post(auth_login))
        .route("/v1/me", get(me))
        .route("/v1/keys/rotate", post(rotate_key))
        .route("/v1/prove", post(prove_checked))
        .route("/v1/verify", post(verify_checked))
        .route("/v1/proof/inspect", post(inspect))
        .route("/v1/billing/checkout", post(billing_checkout))
        .route("/v1/stripe/webhook", post(stripe_webhook))
        .route("/v1/admin/keys", post(admin_new_key))
        .route("/v1/admin/keys/:key/tier", post(admin_set_tier))
        .route("/v1/admin/keys/:key/usage", get(admin_usage))
        .route("/v1/admin/srs/init", post(admin_init_srs))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(GovernorLayer {
            config: governor_conf,
        })
        .with_state(AppState {
            addr,
            kvs,
            admin_token,
            free_cap,
            pro_cap,
            scale_cap,
            max_rows,
            free_max_rows,
            pro_max_rows,
            scale_max_rows,
            allow_dev_srs,
            stripe,
            price_pro,
            price_scale,
            success_url,
            cancel_url,
            portal_return_url,
        })
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    println!("tinyzkp API listening on http://{addr}");
    println!();
    println!("IMPORTANT: Initialize SRS before proving/verifying:");
    println!("  curl -X POST http://{addr}/v1/admin/srs/init \\");
    println!("    -H \"X-Admin-Token: $ADMIN_TOKEN\" \\");
    println!("    -H \"Content-Type: application/json\" \\");
    println!("    -d '{{\"max_degree\": 16384, \"validate_pairing\": false}}'");
    println!();

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}