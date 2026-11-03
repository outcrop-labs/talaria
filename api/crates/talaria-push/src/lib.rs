// Web Push, closed-tab: the notification that reaches a browser whose tab
// is gone. RFC 8292 (VAPID) proves to the push service who is knocking and
// lets it reach this instance's subscribers; RFC 8291 (aes128gcm) puts the
// payload beyond the service's eyes — the push service carries ciphertext
// only, and the service worker holds the only other key half.
//
// HAND-ROLLED ON p256, NOT the `web-push` crate: that crate links OpenSSL,
// and the no-OpenSSL invariant (bare-alpine runtime, rustls everywhere)
// outranks convenience. Every primitive below is assembled from the
// RustCrypto family already in the tree — HKDF over hmac+sha2, records over
// aes-gcm, ECDSA/ECDH over p256 — and pinned to RFC test vectors so the
// hand-assembly is the TESTED part, not the trusted one.
//
// The plane: `push_subscriptions` rows (one per device browser, unique on
// the push service's endpoint), a VAPID keypair born once and SEALED in
// app_settings (the private half under the instance secretbox — never
// plaintext, never logged), and `deliver`, fanned to every subscription of
// a user from the single notification writer.
//
// DELIVERY DEGRADES PER DEVICE, NEVER ACROSS THEM. A person's devices are
// posted in parallel (four at a time), every POST is bounded (5 s to
// connect, 15 s in all) and the whole process holds at most 32 in flight —
// one hung push service used to hold a person's other devices for as long
// as it liked (75 s measured), and every notification spawned its own
// unbounded fan. The payload is clamped under the 4,096-byte record every
// push service guarantees; a 413 still gets one minimal resend. 429, 5xx and
// connect failures are retried twice at most, a TIMEOUT once (an endpoint
// that accepts and never answers holds a process-wide permit 15 s a try).
// The row records every attempt (`last_status`, `last_error`,
// `last_attempt_at`): 2xx resets `failures`, 404/410 prune at once (the
// service already forgot it), and 401/403 prune at three in a row — except a
// refusal of the INSTANCE's own token (Apple's `BadJwtToken` family), which
// is this server's fault and no device's. A row that timed out, or failed
// three times running, is SKIPPED for 2^failures minutes (an hour at most)
// after its last attempt, and ten failures in a row with `last_seen_at` a
// week old prune it (`last_seen_at` moves on a 2xx AND on every re-file of
// the subscription, so a browser that keeps opening Talaria keeps its row,
// tried at most hourly).
//
// AN ENDPOINT IS A URL THE PERSON CHOSE. Whatever a session posts to
// /api/push/subscribe is where this server POSTs, so the subscribe door
// and the edge both walk the talaria-safe-fetch policy like every other
// user-supplied URL: a public address (loopback, RFC1918, link-local — the
// 169.254.169.254 metadata service — CGNAT, multicast and reserved space are
// refused by name and by address before the POST, and again at connect time
// by the resolver the socket dials through), and no redirect followed: a
// push service answers 201, so a 3xx is an answer, not a hop. (The door
// also wants https, the push services' own contract.) A refusal is
// `PushFailure::Refused` and its text says only that — never the address a
// name resolved to — because the test push and the devices list read the
// error back to the person, and a refusal that named the inside address
// would be the very oracle the rule closes. The operator's
// TALARIA_FETCH_ALLOW_HOSTS (safe-fetch's escape hatch, and how a live suite
// reaches a capture server on loopback) admits a push service on their own
// network. A person keeps at most `DEVICES_PER_PERSON` subscriptions: filing
// one more retires the longest-unseen, and a delivery reads at most that
// many rows.
//
// The VAPID keypair is unsealed once and cached, keyed by the sealed string
// the row holds: the cheap SELECT stays on every delivery, so a re-mint, a
// restore or a key rotation's re-seal is seen at once (SE-15).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use futures_util::future::BoxFuture;
use hmac::{Hmac, Mac};
use p256::ecdh::EphemeralSecret;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::pkcs8::DecodePrivateKey;
use serde_json::Value;
use sha2::Sha256;
use sqlx::PgPool;

use talaria_secretbox::SecretBox;

type HmacSha256 = Hmac<Sha256>;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// app_settings key for the VAPID keypair row. Pub because the live suite
/// cleans the row by name — the key IS the plane's storage contract.
pub const VAPID_KEY: &str = "push_vapid";
/// The JWT's `sub` when nothing better is known — RFC 8292 wants an operator
/// contact the push service could reach. The instance's own public https
/// URL is preferred (`vapid_subject`): `.local` routes nowhere, and Apple's
/// service is reported to refuse placeholder subjects with a 403 that would
/// otherwise read as every Safari subscription's fault.
const VAPID_SUB_MAILTO: &str = "mailto:notifications@talaria.local";
/// JWT lifetime: long enough for slow deliveries, short enough that a leaked
/// token dies on its own (RFC 8292 suggests ≥ 12h tolerance, services
/// commonly cap at 24h).
const VAPID_TTL_SECS: i64 = 12 * 60 * 60;
/// aes128gcm's record size — above any payload this plane sends
/// (`push_payload` holds the plaintext under `PAYLOAD_MAX_BYTES`, whatever a
/// writer handed over), so the single record the scheme allows stays single.
const RECORD_SIZE: u32 = 4096;

// ── HKDF (RFC 5869) ───────────────────────────────────────────────────────────

pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(salt).expect("hmac accepts any key length");
    mac.update(ikm);
    mac.finalize()
        .into_bytes()
        .to_vec()
        .try_into()
        .expect("sha256 is 32 bytes")
}

pub fn hkdf_expand(prk: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut prev: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < len {
        let mut mac = HmacSha256::new_from_slice(prk).expect("hmac accepts any key length");
        mac.update(&prev);
        mac.update(info);
        mac.update(&[counter]);
        prev = mac.finalize().into_bytes().to_vec();
        out.extend_from_slice(&prev);
        counter = counter.checked_add(1).expect("len is bounded by callers");
    }
    out.truncate(len);
    out
}

/// The aes128gcm info lines (RFC 8291 §3.2): the literal, a NULL, and
/// NOTHING else. The length-suffixed shape ("…" || 0x00 || 0x01 || len
/// little-endian) belongs to the retired aesgcm scheme of RFC 8188 —
/// appending it here derives a CEK the browser will never hold. The HKDF
/// counter byte my expand appends (0x01 on the single block these need) IS
/// the RFC's "|| 0x01" step; the pinned RFC 8291 vector proves the whole
/// chain byte for byte in the test at the bottom of this file.
fn content_key_info() -> Vec<u8> {
    let mut info = b"Content-Encoding: aes128gcm".to_vec();
    info.push(0x00);
    info
}

fn nonce_info() -> Vec<u8> {
    let mut info = b"Content-Encoding: nonce".to_vec();
    info.push(0x00);
    info
}

// ── Payload encryption (RFC 8291 aes128gcm) ──────────────────────────────────

use aes_gcm::Aes128Gcm;
use aes_gcm::Nonce;
use aes_gcm::aead::{Aead, KeyInit};
use p256::PublicKey;

/// Encrypt one payload against one subscription. Returns the wire body:
/// salt(16) ‖ rs(4, big-endian) ‖ id-len(1) ‖ key-id(65, the ephemeral
/// public key uncompressed) ‖ ciphertext. The plaintext gains the 0x02
/// record delimiter — the scheme's padding byte, terminating one record.
pub fn aes128gcm_encrypt(
    plaintext: &[u8],
    subscriber_key: &PublicKey,
    auth_secret: &[u8; 16],
    salt: &[u8; 16],
    ephemeral: &EphemeralSecret,
) -> Vec<u8> {
    let shared = ephemeral.diffie_hellman(subscriber_key);
    let subscriber_pub: [u8; 65] = subscriber_key
        .to_sec1_point(false)
        .as_bytes()
        .try_into()
        .expect("uncompressed p256 is 65 bytes");
    let ephemeral_pub: [u8; 65] = ephemeral
        .public_key()
        .to_sec1_point(false)
        .as_bytes()
        .try_into()
        .expect("uncompressed p256 is 65 bytes");
    encrypt_with_shared(
        plaintext,
        shared.raw_secret_bytes(),
        &subscriber_pub,
        &ephemeral_pub,
        auth_secret,
        salt,
    )
}

/// The derivation and the single record, with the ECDH leg's two artifacts
/// handed in rather than computed — the split the RFC 8291 vector test
/// needs. An EphemeralSecret cannot be built from pinned bytes (rightly:
/// its whole job is never existing before the moment), so the test pins
/// what the ephemeral PRODUCES — the shared secret and the public point —
/// and this function proves the rest.
///
/// RFC 8291 §3.2 is TWO HKDF stages, not one:
///
///   PRK_key = HMAC-SHA256(auth_secret, ecdh_secret)      // stage 1
///   IKM     = HKDF-Expand(PRK_key, key_info, 32)         // key_info binds
///              both public keys ("WebPush: info" || 0x00 || ua || as)
///   PRK     = HMAC-SHA256(salt, IKM)                     // stage 2
///   CEK     = HKDF-Expand(PRK, "…aes128gcm" || 0x00, 16)
///   NONCE   = HKDF-Expand(PRK, "…nonce"   || 0x00, 12)
///
/// The single `ecdh || auth` concatenation is the retired aesgcm scheme's
/// IKM — feeding it here would encrypt against a key no browser holds.
fn encrypt_with_shared(
    plaintext: &[u8],
    shared: &[u8],
    subscriber_pub: &[u8; 65],
    ephemeral_pub: &[u8; 65],
    auth_secret: &[u8; 16],
    salt: &[u8; 16],
) -> Vec<u8> {
    let prk_key = hkdf_extract(auth_secret, shared);
    let mut key_info = b"WebPush: info".to_vec();
    key_info.push(0x00);
    key_info.extend_from_slice(subscriber_pub);
    key_info.extend_from_slice(ephemeral_pub);
    let ikm = hkdf_expand(&prk_key, &key_info, 32);

    let prk = hkdf_extract(salt, &ikm);
    let cek = hkdf_expand(&prk, &content_key_info(), 16);
    let nonce = hkdf_expand(&prk, &nonce_info(), 12);

    // The tree's secretbox idiom: aead 0.6 has no GenericArray re-export, and
    // `encrypt` takes the payload as `&[u8]` (its `Payload` From-impls stop at
    // slices — a `&Vec<u8>` does not convert).
    let cipher = Aes128Gcm::new_from_slice(&cek).expect("cek is 16 bytes");
    let iv: [u8; 12] = nonce.as_slice().try_into().expect("nonce is 12 bytes");
    let mut padded = plaintext.to_vec();
    padded.push(0x02);
    let ct = cipher
        .encrypt(&Nonce::from(iv), padded.as_slice())
        .expect("aes-gcm with a fresh key cannot fail");

    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + ct.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(65);
    body.extend_from_slice(ephemeral_pub);
    body.extend_from_slice(&ct);
    body
}

// ── VAPID JWT (RFC 8292, ES256) ──────────────────────────────────────────────

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};

/// The ES256 JWT a push service checks before it will talk to a subscriber's
/// server: aud = the ENDPOINT's origin, exp = now + 12h, sub = a contact.
/// The signature is the raw 64-byte r‖s, base64url — RFC 8292's shape, not
/// the DER one JWT tooling usually emits.
pub fn vapid_jwt(
    signing: &SigningKey,
    audience_origin: &str,
    now_unix: i64,
    subject: &str,
) -> String {
    let header = "{\"alg\":\"ES256\",\"typ\":\"JWT\"}";
    // serde_json, not format!: the subject is configuration (a URL), and a
    // quote in it must not break the claims object. The three claims RFC
    // 8292 names, nothing else.
    let claims = serde_json::json!({
        "aud": audience_origin,
        "exp": now_unix + VAPID_TTL_SECS,
        "sub": subject,
    })
    .to_string();
    let signing_input = format!(
        "{}.{}",
        B64U.encode(header.as_bytes()),
        B64U.encode(claims.as_bytes())
    );
    // SigningKey's Signer hashes with SHA-256 internally (P-256's default
    // prehash), which is exactly ES256's contract.
    let sig: Signature = signing.sign(signing_input.as_bytes());
    let raw = sig.to_bytes(); // r‖s, 64 bytes
    format!("{signing_input}.{}", B64U.encode(raw))
}

/// The Authorization header value for a delivery: `vapid t=<jwt>, k=<public>`.
/// k is the UNCOMPRESSED public key (65 bytes, 0x04 prefix included).
pub fn vapid_authorization(
    signing: &SigningKey,
    audience_origin: &str,
    now_unix: i64,
    subject: &str,
) -> String {
    let jwt = vapid_jwt(signing, audience_origin, now_unix, subject);
    // to_sec1_point hands back an owned point — bind it, then borrow: the
    // bytes must outlive the encode below.
    let point = signing.verifying_key().to_sec1_point(false);
    let public = point.as_bytes();
    format!("vapid t={jwt}, k={}", B64U.encode(public))
}

// ── The keypair: born once, sealed, then read forever ────────────────────────

/// The instance's VAPID identity for this run.
pub struct VapidKeys {
    pub signing: SigningKey,
    /// Uncompressed public key, 65 bytes — the exact bytes /api/push/key
    /// hands the browser.
    pub public: [u8; 65],
}

impl Clone for VapidKeys {
    fn clone(&self) -> Self {
        Self {
            signing: self.signing.clone(),
            public: self.public,
        }
    }
}

/// The unsealed keypair, keyed by the sealed `private` string it came from.
/// A different string (re-minted, restored, re-sealed by a rotation) misses
/// and unseals again.
static VAPID_CACHE: Mutex<Option<(String, VapidKeys)>> = Mutex::new(None);
/// How many times this process has unsealed the keypair — the number the
/// cache keeps at one per key.
static VAPID_UNSEALS: AtomicU64 = AtomicU64::new(0);

/// How many VAPID unseals this process has done since boot.
pub fn vapid_unseals() -> u64 {
    VAPID_UNSEALS.load(Ordering::Relaxed)
}

/// Drop the cached keypair, so the next read unseals again. For a test that
/// opens the row with a second box, and for anything that restores keys in
/// place.
pub fn forget_vapid_cache() {
    *VAPID_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Read-or-create the instance VAPID keypair. The private half lives in
/// app_settings SEALED by the instance secretbox; the INSERT is
/// on-conflict-do-nothing so two concurrent first calls (two processes, two
/// tabs, a deploy overlap) converge on the WINNER's row by re-reading.
pub async fn vapid_keys(pg: &PgPool, sb: &SecretBox) -> Result<VapidKeys, String> {
    if let Some(keys) = read_vapid_keys(pg, sb).await? {
        return Ok(keys);
    }
    let secret = fresh_secret_key();
    let public = public_65(&secret);
    let sealed = sb
        .seal(&B64.encode(pkcs8_der(&secret)))
        .map_err(|e| format!("sealing the vapid private key failed: {e}"))?;
    let row = serde_json::json!({
        "public": B64U.encode(public),
        "private": sealed,
    });
    // on-conflict-do-nothing, NOT the settings upsert: a racing writer must
    // not overwrite the winner — both then read back the same keypair.
    if let Err(e) = sqlx::query(
        "insert into app_settings (key, value) values ($1, $2) on conflict (key) do nothing",
    )
    .bind(VAPID_KEY)
    .bind(&row)
    .execute(pg)
    .await
    {
        tracing::error!("[push] could not store the vapid keypair: {e}");
    }
    read_vapid_keys(pg, sb)
        .await?
        .ok_or_else(|| "the vapid keypair row vanished between write and read".into())
}

async fn read_vapid_keys(pg: &PgPool, sb: &SecretBox) -> Result<Option<VapidKeys>, String> {
    let row: Option<(Value,)> = sqlx::query_as("select value from app_settings where key = $1")
        .bind(VAPID_KEY)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("reading the vapid keypair failed: {e}"))?;
    let Some((value,)) = row else { return Ok(None) };
    let sealed = value
        .get("private")
        .and_then(Value::as_str)
        .ok_or("the vapid row has no private key")?;
    if let Some((cached_for, keys)) = &*VAPID_CACHE.lock().unwrap_or_else(|p| p.into_inner())
        && cached_for == sealed
    {
        return Ok(Some(keys.clone()));
    }
    let der_b64 = sb
        .open(sealed)
        .map_err(|e| format!("opening the vapid private key failed: {e}"))?;
    let der = B64
        .decode(der_b64.as_bytes())
        .map_err(|_| "the vapid private key is not base64".to_string())?;
    let secret = p256::SecretKey::from_pkcs8_der(&der)
        .map_err(|e| format!("the vapid private key does not parse: {e}"))?;
    let signing = SigningKey::from(&secret);
    let public = signing
        .verifying_key()
        .to_sec1_point(false)
        .as_bytes()
        .to_vec()
        .try_into()
        .expect("uncompressed p256 is 65 bytes");
    let keys = VapidKeys { signing, public };
    let n = VAPID_UNSEALS.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::debug!("[push] vapid keypair unsealed (unseal #{n} since boot)");
    *VAPID_CACHE.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((sealed.to_string(), keys.clone()));
    Ok(Some(keys))
}

// ── Fresh key material ───────────────────────────────────────────────────────

/// A fresh P-256 secret key from the OS CSPRNG. `Generate::generate` (the
/// crypto-common trait elliptic-curve re-exports) draws from the system RNG
/// directly — `SecretKey::random(&mut OsRng)` is the deprecated spelling and
/// would drag rand_core's os_rng feature in for one call.
fn fresh_secret_key() -> p256::SecretKey {
    p256::SecretKey::generate()
}

/// The key as PKCS#8 DER — the sealed-at-rest form.
fn pkcs8_der(secret: &p256::SecretKey) -> Vec<u8> {
    use p256::pkcs8::EncodePrivateKey;
    secret
        .to_pkcs8_der()
        .expect("a p256 key always encodes")
        .as_bytes()
        .to_vec()
}

/// The public half, uncompressed (65 bytes, 0x04 ‖ X ‖ Y) — the byte shape
/// both the browser and the VAPID header consume.
fn public_65(secret: &p256::SecretKey) -> [u8; 65] {
    secret
        .public_key()
        .to_sec1_point(false)
        .as_bytes()
        .to_vec()
        .try_into()
        .expect("uncompressed p256 is 65 bytes")
}

// ── The VAPID subject ────────────────────────────────────────────────────────

/// The `sub` claim: the instance's public https URL when one is configured
/// (`AUTH_PUBLIC_URL`), else its verified hosting domain, else the mailto
/// placeholder. Pure, so the precedence is pinned by a test.
pub fn pick_subject(public_url: Option<&str>, verified_base: Option<&str>) -> String {
    if let Some(u) = public_url
        .map(str::trim)
        .filter(|u| u.starts_with("https://"))
    {
        return u.trim_end_matches('/').to_string();
    }
    if let Some(u) = verified_base
        .map(str::trim)
        .filter(|u| u.starts_with("https://"))
    {
        return u.trim_end_matches('/').to_string();
    }
    VAPID_SUB_MAILTO.to_string()
}

/// This instance's subject, read fresh (one env read and one settings read
/// per delivery — a domain verified a minute ago is the subject now).
pub async fn vapid_subject(pg: &PgPool) -> String {
    let public = talaria_auth_config::get_auth_config().public_url;
    let verified = talaria_instance::instance_base_url(pg).await;
    pick_subject(public.as_deref(), verified.as_deref())
}

// ── The send edge (injectable, the DrainDeps pattern) ────────────────────────

/// One delivery as the plane sees it: where, with what headers, carrying
/// what ciphertext. The fake edge in tests answers by endpoint.
pub struct PushPost {
    pub endpoint: String,
    pub authorization: String,
    pub body: Vec<u8>,
    /// `TTL` in seconds; `None` sends 3600 (wave 1's value).
    pub ttl_secs: Option<u32>,
    /// `Urgency`; `None` sends `normal`.
    pub urgency: Option<Urgency>,
    /// `Topic` (RFC 8030 §5.4: a newer push under the same topic replaces an
    /// undelivered one); `None`, or one that is not ≤ 32 base64url
    /// characters, sends no header.
    pub topic: Option<String>,
}

/// RFC 8030 §5.3 urgency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    VeryLow,
    Low,
    Normal,
    High,
}

impl Urgency {
    /// The header value.
    pub fn as_header(self) -> &'static str {
        match self {
            Urgency::VeryLow => "very-low",
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "high",
        }
    }
}

/// The TTL a post carries when its note names none.
pub const DEFAULT_TTL_SECS: u32 = 3600;

/// A `Topic` the push services accept: 1–32 characters of the URL-safe
/// base64 alphabet. Anything else is dropped rather than sent.
pub fn valid_topic(t: &str) -> bool {
    (1..=32).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The three delivery headers a post sends, from what it carries: wave 1's
/// `TTL: 3600` / `Urgency: normal` / no `Topic` when it carries nothing.
pub fn post_headers(post: &PushPost) -> Vec<(&'static str, String)> {
    let mut h = vec![
        ("TTL", post.ttl_secs.unwrap_or(DEFAULT_TTL_SECS).to_string()),
        (
            "Urgency",
            post.urgency
                .unwrap_or(Urgency::Normal)
                .as_header()
                .to_string(),
        ),
    ];
    if let Some(t) = post.topic.as_deref().filter(|t| valid_topic(t)) {
        h.push(("Topic", t.to_string()));
    }
    h
}

/// What a push service answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushAnswer {
    pub status: u16,
    /// `Retry-After` in whole seconds, when the service sent the
    /// delta-seconds form (the date form is read as absent).
    pub retry_after: Option<u64>,
    /// The start of the response body — where a refusal names its reason
    /// (`{"reason":"BadJwtToken"}`).
    pub body: String,
}

impl PushAnswer {
    /// An answer with no Retry-After and no body — what a fake edge mostly
    /// needs.
    pub fn status(status: u16) -> Self {
        Self {
            status,
            retry_after: None,
            body: String::new(),
        }
    }
}

/// Why a POST got no HTTP answer at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushFailure {
    /// The connect or the whole exchange ran past its bound.
    Timeout(String),
    /// The connection could not be made.
    Connect(String),
    /// Anything else the transport refused (a request that never left).
    Other(String),
    /// The endpoint is not one this server may reach — not a public address
    /// (see the header). Never retried, and its text never names what the
    /// endpoint resolved to.
    Refused(String),
}

impl std::fmt::Display for PushFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushFailure::Timeout(e) => write!(f, "timeout: {e}"),
            PushFailure::Connect(e) => write!(f, "connect: {e}"),
            PushFailure::Other(e) => write!(f, "{e}"),
            PushFailure::Refused(e) => write!(f, "refused: {e}"),
        }
    }
}

pub type PushResult = Result<PushAnswer, PushFailure>;

pub type PostPushFn = Arc<dyn Fn(PushPost) -> BoxFuture<'static, PushResult> + Send + Sync>;

/// Connect bound for one POST — and for the pre-flight resolution of its
/// endpoint, which runs before the client's own clock starts.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole-exchange bound for one POST (connect, send, answer, body).
const POST_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of a refusal's body is kept — enough for a reason code.
const ANSWER_BODY_CHARS: usize = 512;
/// The most bytes of an answer's body read off the wire: 512 characters at
/// four bytes apiece. A body past it is dropped unread, not buffered.
const ANSWER_BODY_BYTES: usize = ANSWER_BODY_CHARS * 4;

/// What a refused endpoint says to the person — the test push's `error` and
/// the devices list's `lastError`. One sentence for every refusal: which
/// rule fired, and what the name resolved to, stay in the server log.
pub const ENDPOINT_REFUSED: &str = "not a public push service address";

/// What the endpoint policy says about one push endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointVerdict {
    /// Every address it resolves to is public (or the operator allowlisted
    /// it in TALARIA_FETCH_ALLOW_HOSTS).
    Public,
    /// Not a public address, an internal-only name, or not an http(s) URL.
    /// The reason names the address: it is for the server log, never for
    /// the person.
    Refused(String),
    /// Its name did not resolve, or not within `CONNECT_TIMEOUT` — a
    /// transport failure, not a verdict on the endpoint.
    Unresolved(String),
}

/// The push endpoint policy — talaria-safe-fetch's: literal addresses and
/// internal-only names refused outright, a name resolved and EVERY address
/// it answers checked against the blocked table. The subscribe door asks it
/// before a row files; the edge asks it before every POST (a row filed
/// before this rule, or a name that has moved since). Resolution is bounded
/// by `CONNECT_TIMEOUT`.
pub async fn check_endpoint(endpoint: &str) -> EndpointVerdict {
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        talaria_safe_fetch::assert_fetchable_url(endpoint),
    )
    .await
    {
        Ok(Ok(_)) => EndpointVerdict::Public,
        // safe-fetch's own sentence for a name with no answer; every other
        // refusal is a verdict on the address.
        Ok(Err(e)) if e.0.starts_with("could not resolve") => EndpointVerdict::Unresolved(e.0),
        Ok(Err(e)) => EndpointVerdict::Refused(e.0),
        Err(_) => EndpointVerdict::Unresolved(format!(
            "resolving it took over {} s",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// The client every push POST rides. BOUNDED (a push service that accepts
/// the connection and never answers used to hold the delivery for as long
/// as it liked), NEVER follows a redirect (a 3xx comes back as the answer),
/// and dials through safe-fetch's validating resolver, so the address the
/// socket actually uses is checked again at connect time — the pre-flight
/// check alone leaves a name free to answer differently a moment later.
fn push_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(Arc::new(talaria_safe_fetch::ValidatingResolver))
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(POST_TIMEOUT)
                .build()
                .expect("the push client builds")
        })
        .clone()
}

/// Whether a transport error is the validating resolver refusing the
/// address at connect time — its sentence ("refusing <host> — …") rides
/// somewhere down reqwest's source chain.
fn refused_at_connect(e: &reqwest::Error) -> bool {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(c) = cause {
        if c.to_string().starts_with("refusing ") {
            return true;
        }
        cause = c.source();
    }
    false
}

/// The POST itself, over `client`, with no pre-flight: the edge's second
/// half, apart so a test can drive the client against a loopback listener.
async fn send_push(client: &reqwest::Client, post: PushPost) -> PushResult {
    let mut req = client
        .post(&post.endpoint)
        .header("Authorization", &post.authorization);
    for (name, value) in post_headers(&post) {
        req = req.header(name, value);
    }
    let mut resp = req
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .body(post.body)
        .send()
        .await
        .map_err(|e| {
            if refused_at_connect(&e) {
                tracing::warn!(
                    "[push] the resolver refused a push endpoint's address at connect time"
                );
                PushFailure::Refused(ENDPOINT_REFUSED.into())
            } else if e.is_timeout() {
                PushFailure::Timeout(e.to_string())
            } else if e.is_connect() {
                PushFailure::Connect(e.to_string())
            } else {
                PushFailure::Other(e.to_string())
            }
        })?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    // The body is read under the same 15 s bound and only as far as the
    // reason code needs; a service that answers the status and stalls (or
    // floods) the body still settles.
    let mut raw: Vec<u8> = Vec::new();
    while raw.len() < ANSWER_BODY_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => raw.extend_from_slice(&chunk),
            _ => break,
        }
    }
    raw.truncate(ANSWER_BODY_BYTES);
    let body: String = String::from_utf8_lossy(&raw)
        .chars()
        .take(ANSWER_BODY_CHARS)
        .collect();
    Ok(PushAnswer {
        status,
        retry_after,
        body,
    })
}

/// The production edge over reqwest (rustls — the no-OpenSSL invariant is
/// the whole reason this module is hand-rolled): the endpoint policy first,
/// then one POST over the guarded client. A refused endpoint is never
/// dialled.
pub fn real_post_push() -> PostPushFn {
    let client = push_client();
    Arc::new(move |post: PushPost| {
        let client = client.clone();
        Box::pin(async move {
            match check_endpoint(&post.endpoint).await {
                EndpointVerdict::Public => {}
                EndpointVerdict::Refused(why) => {
                    tracing::warn!("[push] not posting to a subscription endpoint: {why}");
                    return Err(PushFailure::Refused(ENDPOINT_REFUSED.into()));
                }
                EndpointVerdict::Unresolved(why) => {
                    return Err(PushFailure::Connect(format!(
                        "the push service's name did not resolve ({why})"
                    )));
                }
            }
            send_push(&client, post).await
        })
    })
}

// ── Delivery ─────────────────────────────────────────────────────────────────

/// The row-shaped notification the plane sends — everything the service
/// worker needs to show AND to navigate on click. `Default` so a call site
/// names only what it sets (C15): `PushNote { id, title, ..Default::default() }`.
#[derive(Clone, Debug, Default)]
pub struct PushNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub href: String,
    /// The writer's kind (`mention`, `dm`, …) — a hint the worker may read.
    pub kind: String,
    /// A row brought up to date in place (a newer DM in the same
    /// conversation): the OS should re-show it under the same tag rather
    /// than replace it silently.
    pub renotify: bool,
    /// The row's createdAt as the list serves it (ISO, ms precision) — the
    /// VERSION the worker and the pages record as bannered, so one version
    /// of a row is shown once per browser and a refreshed row again.
    pub created_at: String,
    /// Delivery headers (W2-E sets them per category); `None` sends wave 1's.
    pub ttl_secs: Option<u32>,
    pub urgency: Option<Urgency>,
    pub topic: Option<String>,
}

/// Title and body bounds before the byte budget applies.
const TITLE_MAX_CHARS: usize = 200;
const BODY_MAX_CHARS: usize = 600;
/// The plaintext budget. RFC 8030 push services only guarantee a 4,096-byte
/// body; aes128gcm adds 86 bytes of header, 16 of tag and 1 delimiter, so
/// 3,000 leaves room for every engine's own framing.
pub const PAYLOAD_MAX_BYTES: usize = 3_000;
/// The 413 resend's title bound.
const MINIMAL_TITLE_CHARS: usize = 80;

/// The first `max` chars, with '…' only when something was cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn payload_json(note: &PushNote, title: &str, body: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "id": note.id,
        "title": title,
        "body": body,
        "href": note.href,
        "kind": note.kind,
        "renotify": note.renotify,
        "createdAt": note.created_at,
    }))
    .expect("plain strings serialize")
}

/// The JSON the worker decrypts, held under `PAYLOAD_MAX_BYTES`: the title
/// and body are clamped, then the body shrinks, then the title, until it
/// fits. A writer may hand over anything (a 2,500-character plan title, an
/// unbounded approval detail); the push service never sees more than its
/// guarantee.
pub fn push_payload(note: &PushNote) -> Vec<u8> {
    let mut title_chars = note.title.chars().count().min(TITLE_MAX_CHARS);
    let mut body_chars = note.body.chars().count().min(BODY_MAX_CHARS);
    loop {
        let out = payload_json(
            note,
            &clip(&note.title, title_chars),
            &clip(&note.body, body_chars),
        );
        if out.len() <= PAYLOAD_MAX_BYTES || (title_chars == 0 && body_chars == 0) {
            if out.len() <= PAYLOAD_MAX_BYTES {
                return out;
            }
            return minimal_payload(note);
        }
        if body_chars > 0 {
            body_chars = body_chars * 3 / 4;
        } else {
            title_chars = title_chars * 3 / 4;
        }
    }
}

/// The 413 resend: `{id, title, href}` with the title at 80 characters —
/// enough for the worker to show something and open the right page.
pub fn minimal_payload(note: &PushNote) -> Vec<u8> {
    let href = if note.href.len() + MINIMAL_TITLE_CHARS * 4 + 128 <= PAYLOAD_MAX_BYTES {
        note.href.as_str()
    } else {
        ""
    };
    serde_json::to_vec(&serde_json::json!({
        "id": note.id,
        "title": clip(&note.title, MINIMAL_TITLE_CHARS),
        "href": href,
    }))
    .expect("plain strings serialize")
}

/// Retry spacing, injectable so the live suite need not sleep ten seconds.
#[derive(Debug, Clone)]
pub struct PushPolicy {
    /// The waits before the first and second retry when the service named
    /// none.
    pub backoff: [Duration; 2],
    /// The longest `Retry-After` honored; a service asking for longer is
    /// taken at its word and not retried this time.
    pub retry_after_cap: Duration,
}

impl Default for PushPolicy {
    fn default() -> Self {
        Self {
            backoff: [Duration::from_secs(2), Duration::from_secs(8)],
            retry_after_cap: Duration::from_secs(60),
        }
    }
}

/// The wait before retry number `retries_done + 1`, or `None` when this
/// answer is final. Retried: 429, 5xx and a connect failure at most twice; a
/// TIMEOUT once — an endpoint that accepts and never answers holds one of the
/// process's 32 permits for the whole 15 s bound on every try. `Retry-After`
/// wins when it is within the cap; past it the service asked for more
/// patience than a notification is worth.
pub fn retry_delay(
    retries_done: usize,
    result: &PushResult,
    policy: &PushPolicy,
) -> Option<Duration> {
    if retries_done >= policy.backoff.len() {
        return None;
    }
    match result {
        Ok(a) if a.status == 429 || (500..600).contains(&a.status) => match a.retry_after {
            Some(secs) => {
                let wait = Duration::from_secs(secs);
                (wait <= policy.retry_after_cap).then_some(wait)
            }
            None => Some(policy.backoff[retries_done]),
        },
        Err(PushFailure::Timeout(_)) if retries_done < TIMEOUT_RETRIES => {
            Some(policy.backoff[retries_done])
        }
        Err(PushFailure::Connect(_)) => Some(policy.backoff[retries_done]),
        _ => None,
    }
}

/// Retries a timeout gets.
pub const TIMEOUT_RETRIES: usize = 1;

/// Apple's refusals of the INSTANCE's token rather than the subscription:
/// the JWT itself is bad, expired, missing or unverifiable. Counting these
/// against devices would prune every Safari subscription for a fault that
/// lives in this server's configuration.
const INSTANCE_JWT_FAULTS: &[&str] = &[
    "BadJwtToken",
    "ExpiredProviderToken",
    "InvalidProviderToken",
    "MissingProviderToken",
];

/// What a final answer does to the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tend {
    /// 2xx: touch liveness, reset the failure count.
    Delivered,
    /// 404/410: the service has forgotten it — prune now.
    Gone,
    /// 401/403 naming the subscription: count it; prune at three in a row.
    Refused,
    /// 401/403 naming the instance's own token: log loudly, count nothing.
    InstanceFault,
    /// Anything else (5xx or 429 past retries, 413 past its resend, a
    /// transport failure): count it, keep the row.
    Failed,
}

/// Consecutive counted failures that prune a row refused by its service.
pub const REFUSALS_TO_PRUNE: i32 = 3;
/// Consecutive failures of any kind that prune a row once its `last_seen_at`
/// (a 2xx, or the browser filing it again) is also a week old: an endpoint
/// that has not worked, nor been opened, for days.
pub const FAILURES_TO_PRUNE: i32 = 10;
/// Failures after which a row sits out its backoff window.
pub const FAILURES_TO_SKIP: i32 = 3;
/// The longest backoff window, in minutes.
pub const SKIP_CAP_MIN: i32 = 60;

/// A delivery's rows: the person's subscriptions, less any sitting out its
/// backoff — it timed out last time (no status, a `timeout:` error) or has
/// failed `FAILURES_TO_SKIP` times running, and its last attempt is within
/// 2^failures minutes (`SKIP_CAP_MIN` at most). A 2xx resets `failures`, so
/// the window closes itself; the per-device test ignores it. At most
/// `DEVICES_PER_PERSON` of them, the most recently seen: the filing cap keeps
/// a person under it, and this keeps a person filed past it before the cap
/// existed from fanning one notification out to every row.
const DELIVERY_ROWS: &str = "select endpoint, p256dh, auth from push_subscriptions \
     where user_id = $1::uuid \
       and not (last_attempt_at is not null \
                and (failures >= 3 or (last_status is null and last_error like 'timeout:%')) \
                and last_attempt_at > now() \
                    - least(power(2, greatest(failures, 1)), 60) * interval '1 minute') \
     order by last_seen_at desc, created_at desc, id \
     limit 20";

pub fn classify(result: &PushResult) -> Tend {
    match result {
        Ok(a) if (200..300).contains(&a.status) => Tend::Delivered,
        Ok(a) if a.status == 404 || a.status == 410 => Tend::Gone,
        Ok(a) if a.status == 401 || a.status == 403 => {
            if INSTANCE_JWT_FAULTS.iter().any(|r| a.body.contains(r)) {
                Tend::InstanceFault
            } else {
                Tend::Refused
            }
        }
        _ => Tend::Failed,
    }
}

/// `last_error` for a final answer: the status and the start of the body,
/// or the transport's own sentence.
fn error_text(result: &PushResult) -> Option<String> {
    match result {
        Ok(a) if (200..300).contains(&a.status) => None,
        Ok(a) => Some(
            format!("{} {}", a.status, a.body.trim())
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(300)
                .collect(),
        ),
        Err(e) => Some(
            e.to_string()
                .chars()
                .filter(|c| !c.is_control())
                .take(300)
                .collect(),
        ),
    }
}

/// Every POST in the process waits for one of these. A burst (a mention to a
/// whole board, an admin fan-out) queues here instead of opening a socket
/// per device at once.
static IN_FLIGHT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);
/// A person's devices posted at once.
const PER_USER_PARALLEL: usize = 4;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn prune(pg: &PgPool, endpoint: &str) {
    let _ = sqlx::query("delete from push_subscriptions where endpoint = $1")
        .bind(endpoint)
        .execute(pg)
        .await;
}

/// Write the final answer onto the row (see `Tend`).
async fn tend(pg: &PgPool, endpoint: &str, result: &PushResult) {
    let status: Option<i32> = result.as_ref().ok().map(|a| i32::from(a.status));
    let error = error_text(result);
    match classify(result) {
        Tend::Delivered => {
            let _ = sqlx::query(
                "update push_subscriptions set last_seen_at = now(), failures = 0, \
                 last_status = $2, last_error = null, last_attempt_at = now() \
                 where endpoint = $1",
            )
            .bind(endpoint)
            .bind(status)
            .execute(pg)
            .await;
        }
        Tend::Gone => {
            // The push service has forgotten this subscription — the
            // browser unregistered, was reset, or expired it. The row's
            // only remaining job is making every future delivery fail.
            prune(pg, endpoint).await;
        }
        Tend::InstanceFault => {
            tracing::error!(
                "[push] a push service refused THIS INSTANCE's VAPID token ({}) — an instance configuration fault, not the device's; the row is kept and not counted",
                error.as_deref().unwrap_or("")
            );
            let _ = sqlx::query(
                "update push_subscriptions set last_status = $2, last_error = $3, \
                 last_attempt_at = now() where endpoint = $1",
            )
            .bind(endpoint)
            .bind(status)
            .bind(&error)
            .execute(pg)
            .await;
        }
        tended @ (Tend::Refused | Tend::Failed) => {
            let failures: Option<i32> = sqlx::query_scalar(
                "update push_subscriptions set failures = failures + 1, last_status = $2, \
                 last_error = $3, last_attempt_at = now() where endpoint = $1 \
                 returning failures",
            )
            .bind(endpoint)
            .bind(status)
            .bind(&error)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
            if tended == Tend::Refused && failures.is_some_and(|n| n >= REFUSALS_TO_PRUNE) {
                tracing::warn!(
                    "[push] a subscription was refused {REFUSALS_TO_PRUNE} times in a row; pruning"
                );
                prune(pg, endpoint).await;
            } else if failures.is_some_and(|n| n >= FAILURES_TO_PRUNE)
                && sqlx::query(
                    "delete from push_subscriptions where endpoint = $1 \
                     and last_seen_at < now() - interval '7 days'",
                )
                .bind(endpoint)
                .execute(pg)
                .await
                .is_ok_and(|r| r.rows_affected() > 0)
            {
                tracing::warn!(
                    "[push] a subscription failed {FAILURES_TO_PRUNE} times running and was neither delivered to nor re-filed for a week; pruned"
                );
            } else {
                tracing::warn!(
                    "[push] delivery to an endpoint failed ({}); keeping the row",
                    error.as_deref().unwrap_or("")
                );
            }
        }
    }
}

/// One device: encrypt against ITS keys, prove with the instance VAPID
/// identity, POST under the process-wide bound, retry what is worth it,
/// resend minimal once on a 413, then tend the row with the final answer.
#[allow(clippy::too_many_arguments)]
async fn deliver_one(
    pg: &PgPool,
    keys: &VapidKeys,
    post: &PostPushFn,
    policy: &PushPolicy,
    subject: &str,
    payload: &[u8],
    minimal: &[u8],
    note: &PushNote,
    (endpoint, p256dh_b64, auth_b64): (String, String, String),
) {
    // Decode the subscription's key material; unreadable rows are dead
    // weight that can never deliver — prune rather than retry forever.
    let key_bytes = B64U
        .decode(p256dh_b64.as_bytes())
        .or_else(|_| B64.decode(p256dh_b64.as_bytes()));
    let auth_bytes = B64U
        .decode(auth_b64.as_bytes())
        .or_else(|_| B64.decode(auth_b64.as_bytes()));
    let (key_bytes, auth_bytes): (Vec<u8>, Vec<u8>) = match (key_bytes, auth_bytes) {
        (Ok(k), Ok(a)) if k.len() == 65 && a.len() == 16 => (k, a),
        _ => {
            tracing::warn!("[push] a subscription has unreadable keys; pruning");
            prune(pg, &endpoint).await;
            return;
        }
    };
    let Ok(subscriber_key) = PublicKey::from_sec1_bytes(&key_bytes) else {
        tracing::warn!("[push] a subscription's p256dh is not a p256 point; pruning");
        prune(pg, &endpoint).await;
        return;
    };
    let auth: [u8; 16] = auth_bytes.try_into().expect("len checked above");
    let Ok(url) = url::Url::parse(&endpoint) else {
        tracing::warn!("[push] a subscription endpoint does not parse; pruning");
        prune(pg, &endpoint).await;
        return;
    };
    let origin = url.origin().ascii_serialization();
    let mut plaintext = payload;
    let mut resent_minimal = false;
    let mut retries = 0usize;
    loop {
        let salt: [u8; 16] = {
            let mut s = [0u8; 16];
            _ = getrandom::fill(&mut s);
            s
        };
        let ephemeral = EphemeralSecret::generate();
        let body = aes128gcm_encrypt(plaintext, &subscriber_key, &auth, &salt, &ephemeral);
        let authorization = vapid_authorization(&keys.signing, &origin, now_unix(), subject);
        let result = {
            let _permit = IN_FLIGHT.acquire().await;
            post(PushPost {
                endpoint: endpoint.clone(),
                authorization,
                body,
                ttl_secs: note.ttl_secs,
                urgency: note.urgency,
                topic: note.topic.clone(),
            })
            .await
        };
        if matches!(&result, Ok(a) if a.status == 413) && !resent_minimal {
            // The service refused the size. Once, the smallest thing that
            // still shows and still opens the right page.
            tracing::warn!(
                "[push] a push service refused the payload size (413); resending minimal"
            );
            resent_minimal = true;
            plaintext = minimal;
            continue;
        }
        if let Some(wait) = retry_delay(retries, &result, policy) {
            retries += 1;
            tokio::time::sleep(wait).await;
            continue;
        }
        tend(pg, &endpoint, &result).await;
        return;
    }
}

/// Send one note to every device browser of one person, the default retry
/// spacing and this instance's VAPID subject.
pub async fn deliver_push(
    pg: &PgPool,
    sb: &SecretBox,
    post: &PostPushFn,
    user_id: &str,
    note: &PushNote,
) {
    let subject = vapid_subject(pg).await;
    deliver_push_with(
        pg,
        sb,
        post,
        user_id,
        note,
        &PushPolicy::default(),
        &subject,
    )
    .await;
}

/// `deliver_push` with the spacing and subject handed in — the live suite's
/// door. The person's devices go four at a time; each settles on its own.
pub async fn deliver_push_with(
    pg: &PgPool,
    sb: &SecretBox,
    post: &PostPushFn,
    user_id: &str,
    note: &PushNote,
    policy: &PushPolicy,
    subject: &str,
) {
    let rows: Vec<(String, String, String)> = match sqlx::query_as(DELIVERY_ROWS)
        .bind(user_id)
        .fetch_all(pg)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[push] could not read subscriptions for delivery: {e}");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let Ok(keys) = vapid_keys(pg, sb).await else {
        // The reason is already logged inside; a failed keypair is a
        // configuration state, not a per-delivery one.
        return;
    };
    let payload = push_payload(note);
    let minimal = minimal_payload(note);
    use futures_util::StreamExt as _;
    futures_util::stream::iter(rows)
        .for_each_concurrent(PER_USER_PARALLEL, |row| {
            deliver_one(
                pg, &keys, post, policy, subject, &payload, &minimal, note, row,
            )
        })
        .await;
}

// ── The plane: installed once at boot, consulted by the single writer ────────

/// Where the plane gets the instance secretbox, per delivery. Production
/// reads the process's CURRENT box (AppState's), never a clone taken at boot:
/// a key rotation installs a new box and re-seals `push_vapid` under its new
/// version, and a stale clone could not open the re-sealed key.
pub type SecretBoxSource =
    Arc<dyn Fn() -> BoxFuture<'static, Result<SecretBox, String>> + Send + Sync>;

/// A source that always answers the same box (tests).
pub fn fixed_secretbox(sb: SecretBox) -> SecretBoxSource {
    Arc::new(move || {
        let sb = sb.clone();
        Box::pin(async move { Ok(sb) })
    })
}

struct Plane {
    pg: PgPool,
    sb: SecretBoxSource,
    post: PostPushFn,
}

static PLANE: OnceLock<Plane> = OnceLock::new();

/// Install the push plane at boot (talaria-jobs, beside real_drain_deps).
/// Not installed = push off: the hook below is a no-op, which is exactly
/// what tests and any push-less deployment want. A fixed box; production
/// installs with `install_push_plane_with` and AppState's live box.
pub fn install_push_plane(pg: PgPool, sb: SecretBox) {
    install_push_plane_with(pg, fixed_secretbox(sb));
}

/// Install the push plane with a secretbox SOURCE, read per delivery.
pub fn install_push_plane_with(pg: PgPool, sb: SecretBoxSource) {
    let _ = PLANE.set(Plane {
        pg,
        sb,
        post: real_post_push(),
    });
}

/// Whether this process can push at all (the plane is installed at boot).
/// A row filed while it is not records no `pushed_at`.
pub fn push_plane_installed() -> bool {
    PLANE.get().is_some()
}

/// The single writer's hook: every notification row that lands with an
/// in-app or both route also reaches closed tabs. Detached — a push service
///'s latency must never ride a request — and quiet: the row in the database
/// is the record, the delivery is best-effort on top of it. The network work
/// itself waits on the process-wide bound inside.
pub fn push_notification(user_id: &str, note: PushNote) {
    let Some(plane) = PLANE.get() else { return };
    let pg = plane.pg.clone();
    let source = plane.sb.clone();
    let post = plane.post.clone();
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        let sb = match source().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::error!("[push] the secretbox did not load; not delivering: {e}");
                return;
            }
        };
        deliver_push(&pg, &sb, &post, &user_id, &note).await;
    });
}

// ── Devices: the person's own subscriptions, as a status panel reads them ────

/// One device browser, as `GET /api/push/devices` serves it (`to_wire`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    /// "Chrome on Linux", from the User-Agent the subscription was filed with.
    pub label: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub last_attempt_at: Option<String>,
    pub last_status: Option<i32>,
    /// What the push service last said, as plain text of at most 120
    /// characters: an endpoint's body is whatever its operator (or an
    /// attacker holding the endpoint) chose to answer.
    pub last_error: Option<String>,
    pub failures: i32,
    /// Filed by the session asking.
    pub this_session: bool,
}

impl Device {
    /// The wire shape, camelCase, in field order.
    pub fn to_wire(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "createdAt": self.created_at,
            "lastSeenAt": self.last_seen_at,
            "lastAttemptAt": self.last_attempt_at,
            "lastStatus": self.last_status,
            "lastError": self.last_error,
            "failures": self.failures,
            "thisSession": self.this_session,
        })
    }
}

/// The longest `last_error` a client is shown.
pub const DEVICE_ERROR_CHARS: usize = 120;
/// The longest User-Agent a row keeps.
pub const USER_AGENT_CHARS: usize = 300;

/// A User-Agent as stored: trimmed, control characters dropped, at most 300
/// characters; None when nothing is left.
pub fn stored_user_agent(raw: Option<&str>) -> Option<String> {
    let ua: String = raw?
        .chars()
        .filter(|c| !c.is_control())
        .take(USER_AGENT_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    (!ua.is_empty()).then_some(ua)
}

/// "<browser> on <OS>" from a User-Agent, or "Browser" when it says nothing
/// recognisable. Order matters: Edge and Opera carry "Chrome", Chrome carries
/// "Safari", and an iPad says "Mac OS X".
pub fn device_label(user_agent: Option<&str>) -> String {
    let ua = user_agent.unwrap_or("");
    let browser = if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        "Edge"
    } else if ua.contains("OPR/") || ua.contains("Opera") {
        "Opera"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chromium/") {
        "Chromium"
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        "Chrome"
    } else if ua.contains("Safari/") || ua.contains("AppleWebKit/") {
        "Safari"
    } else {
        ""
    };
    let os = if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") {
        "iOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") || ua.contains("X11") {
        "Linux"
    } else {
        ""
    };
    match (browser, os) {
        ("", "") => "Browser".into(),
        (b, "") => b.into(),
        ("", o) => format!("Browser on {o}"),
        (b, o) => format!("{b} on {o}"),
    }
}

/// `last_error` as a client may see it: markup removed, control characters
/// and runs of whitespace collapsed to one space, at most 120 characters.
pub fn plain_error(raw: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut space = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                space = true;
            }
            _ if in_tag => {}
            c if c.is_control() || c.is_whitespace() => space = true,
            c => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
    }
    if out.chars().count() > DEVICE_ERROR_CHARS {
        let mut cut: String = out.chars().take(DEVICE_ERROR_CHARS - 1).collect();
        cut.push('…');
        return cut;
    }
    out
}

type DeviceRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<i32>,
    Option<String>,
    i32,
    Option<String>,
);

/// A timestamptz column as the wire's ISO shape (ms precision, UTC, `Z`).
macro_rules! iso_col {
    ($c:literal) => {
        concat!(
            "to_char(",
            $c,
            " at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')"
        )
    };
}

/// The person's device browsers, newest first, with which one the asking
/// session filed (`session_hash`, the hex SHA-256 of its id).
pub async fn list_devices(
    pg: &PgPool,
    user_id: &str,
    session_hash: Option<&str>,
) -> Result<Vec<Device>, sqlx::Error> {
    let rows: Vec<DeviceRow> = sqlx::query_as(concat!(
        "select id::text, user_agent, ",
        iso_col!("created_at"),
        ", ",
        iso_col!("last_seen_at"),
        ", ",
        iso_col!("last_attempt_at"),
        ", last_status, last_error, failures, session_hash \
         from push_subscriptions where user_id = $1::uuid \
         order by created_at desc, id"
    ))
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, ua, created, seen, attempt, status, error, failures, hash)| Device {
                id,
                label: device_label(ua.as_deref()),
                created_at: created,
                last_seen_at: seen,
                last_attempt_at: attempt,
                last_status: status,
                last_error: error.as_deref().map(plain_error).filter(|e| !e.is_empty()),
                failures,
                this_session: session_hash.is_some() && hash.as_deref() == session_hash,
            },
        )
        .collect())
}

/// The most device browsers one person keeps. A real person has a handful
/// (a laptop, a phone, a second browser); without a ceiling one session
/// could file hundreds and turn every notification into as many POSTs.
pub const DEVICES_PER_PERSON: i64 = 20;

/// One device browser's subscription as the subscribe door files it.
pub struct FiledSubscription<'a> {
    pub user_id: &'a str,
    pub endpoint: &'a str,
    pub p256dh: &'a str,
    pub auth: &'a str,
    /// The filing session's hash, so signing out retires this row and no other.
    pub session_hash: Option<&'a str>,
    /// As `stored_user_agent` cleaned it.
    pub user_agent: Option<&'a str>,
}

/// File (or re-file) one device browser, then hold its person to
/// `DEVICES_PER_PERSON`: the row just filed always stays, and of the rest the
/// most recently seen are kept — a browser that keeps opening Talaria
/// re-files on every boot, so what goes is the device nobody has opened in
/// longest. Upsert on the endpoint: a browser that re-subscribes overwrites
/// its own row, and a row whose endpoint moved to another account is CLAIMED
/// (one endpoint is one browser, whoever is signed into it now). A re-file
/// of the same keys keeps its failure count (every boot re-files, and a row
/// the service keeps refusing must still reach its prune); new keys are a
/// new subscription and start clean; a re-file without a User-Agent keeps
/// the name it had. Answers how many rows the cap retired. The retiring
/// runs after the filing and on its own: a failed sweep is logged and the
/// subscription still stands.
pub async fn file_subscription(
    pg: &PgPool,
    sub: &FiledSubscription<'_>,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth, session_hash, user_agent) \
         values ($1::uuid, $2, $3, $4, $5, $6) \
         on conflict (endpoint) do update \
         set user_id = excluded.user_id, p256dh = excluded.p256dh, \
             auth = excluded.auth, last_seen_at = now(), \
             session_hash = excluded.session_hash, \
             user_agent = coalesce(excluded.user_agent, push_subscriptions.user_agent), \
             failures = case when push_subscriptions.p256dh = excluded.p256dh \
                              and push_subscriptions.auth = excluded.auth \
                             then push_subscriptions.failures else 0 end",
    )
    .bind(sub.user_id)
    .bind(sub.endpoint)
    .bind(sub.p256dh)
    .bind(sub.auth)
    .bind(sub.session_hash)
    .bind(sub.user_agent)
    .execute(pg)
    .await?;
    match sqlx::query(
        "delete from push_subscriptions where id in ( \
           select id from push_subscriptions \
           where user_id = $1::uuid and endpoint <> $2 \
           order by last_seen_at desc, created_at desc, id \
           offset $3)",
    )
    .bind(sub.user_id)
    .bind(sub.endpoint)
    .bind(DEVICES_PER_PERSON - 1)
    .execute(pg)
    .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "[push] a person filed past {DEVICES_PER_PERSON} devices; retired the {} seen longest ago",
                    r.rows_affected()
                );
            }
            Ok(r.rows_affected())
        }
        Err(e) => {
            tracing::warn!("[push] the per-person device cap did not run: {e}");
            Ok(0)
        }
    }
}

/// Retire one of the caller's own device browsers; false when no row of
/// theirs has that id (another person's id answers the same false).
pub async fn delete_device(pg: &PgPool, user_id: &str, id: &str) -> Result<bool, sqlx::Error> {
    let res =
        sqlx::query("delete from push_subscriptions where id = $1::uuid and user_id = $2::uuid")
            .bind(id)
            .bind(user_id)
            .execute(pg)
            .await?;
    Ok(res.rows_affected() > 0)
}

/// Whether `id` is one of the caller's own device browsers.
pub async fn owns_device(pg: &PgPool, user_id: &str, id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from push_subscriptions where id = $1::uuid and user_id = $2::uuid",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// What a per-device test push did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTest {
    pub id: String,
    /// The push service's status, or None when nothing answered.
    pub status: Option<u16>,
    pub ms: u64,
    pub error: Option<String>,
}

impl DeviceTest {
    /// The wire shape (`device` in the test route's answer).
    pub fn to_wire(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "status": self.status,
            "ms": self.ms,
            "error": self.error,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestError {
    /// The test send is not built yet (phase 0) or the plane is absent.
    NotReady,
    /// Not one of the caller's devices.
    NotFound,
}

/// The test push's payload — the shape the service worker shows whether or
/// not a window is focused (`kind: "test"`, ui/src/sw.ts).
pub fn test_note(instance: &str, now_ms: i64) -> PushNote {
    PushNote {
        id: format!("talaria-test-{now_ms}"),
        title: "Talaria test notification".into(),
        body: format!("{instance}: this device gets alerts"),
        href: "/settings/notifications".into(),
        kind: "test".into(),
        renotify: false,
        created_at: iso_ms(now_ms),
        // A test is worth showing now or never: high urgency, five minutes.
        ttl_secs: Some(300),
        urgency: Some(Urgency::High),
        topic: None,
    }
}

fn iso_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// What the test push names the instance by: the company name an admin set,
/// else the verified domain, else "Talaria".
async fn instance_label(pg: &PgPool) -> String {
    if let Some(name) = talaria_instance::get_company_name(pg)
        .await
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    {
        return name;
    }
    talaria_instance::instance_base_url(pg)
        .await
        .map(|u| u.trim_start_matches("https://").to_string())
        .unwrap_or_else(|| "Talaria".into())
}

/// Send one test push to one of the person's devices (the status panel's
/// per-device test, O-20 rate-limits it at the route): ONE direct delivery to
/// that subscription — no presence, no DND, no retry, no backoff window, so
/// the answer is this device's right now — tended like any delivery, so the
/// panel's last status moves with it. The instance's push kill switch still
/// holds: a test that pushes when the operator said no push would lie about
/// what a notification would do.
pub async fn send_test(user_id: &str, device_id: &str) -> Result<DeviceTest, TestError> {
    let Some(plane) = PLANE.get() else {
        return Err(TestError::NotReady);
    };
    let pg = &plane.pg;
    let row: Option<(String, String, String)> = sqlx::query_as(
        "select endpoint, p256dh, auth from push_subscriptions \
         where id = $1::uuid and user_id = $2::uuid",
    )
    .bind(device_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| {
        tracing::error!("[push] the test's device read failed: {e}");
        TestError::NotReady
    })?;
    let Some((endpoint, p256dh, auth)) = row else {
        return Err(TestError::NotFound);
    };
    let fail = |error: String| DeviceTest {
        id: device_id.to_string(),
        status: None,
        ms: 0,
        error: Some(plain_error(&error)),
    };
    let switch: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = 'notify_delivery'")
            .fetch_optional(pg)
            .await
            .unwrap_or(None);
    if switch.as_ref().and_then(|v| v.get("pushEnabled")) == Some(&Value::Bool(false)) {
        return Ok(fail("push is switched off for this instance".into()));
    }
    let sb = match (plane.sb)().await {
        Ok(sb) => sb,
        Err(e) => return Ok(fail(format!("the secretbox did not load: {e}"))),
    };
    let keys = match vapid_keys(pg, &sb).await {
        Ok(k) => k,
        Err(e) => return Ok(fail(e)),
    };
    let subject = vapid_subject(pg).await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let note = test_note(&instance_label(pg).await, now_ms);
    let started = std::time::Instant::now();
    let result = post_once(
        &plane.post,
        &keys,
        &subject,
        &push_payload(&note),
        &note,
        (endpoint.clone(), p256dh, auth),
    )
    .await;
    let ms = started.elapsed().as_millis() as u64;
    let Some(result) = result else {
        // Unreadable key material: the row is dead weight, pruned like any
        // delivery would.
        prune(pg, &endpoint).await;
        return Ok(fail(
            "this device's subscription is unreadable and was removed".into(),
        ));
    };
    tend(pg, &endpoint, &result).await;
    Ok(DeviceTest {
        id: device_id.to_string(),
        status: result.as_ref().ok().map(|a| a.status),
        ms,
        error: error_text(&result)
            .map(|e| plain_error(&e))
            .filter(|e| !e.is_empty()),
    })
}

/// One POST of one payload to one subscription: encrypt against its keys,
/// sign with the instance identity, send under the process-wide bound. None
/// when the subscription's key material cannot be used.
async fn post_once(
    post: &PostPushFn,
    keys: &VapidKeys,
    subject: &str,
    payload: &[u8],
    note: &PushNote,
    (endpoint, p256dh_b64, auth_b64): (String, String, String),
) -> Option<PushResult> {
    let key_bytes = B64U
        .decode(p256dh_b64.as_bytes())
        .or_else(|_| B64.decode(p256dh_b64.as_bytes()))
        .ok()?;
    let auth_bytes = B64U
        .decode(auth_b64.as_bytes())
        .or_else(|_| B64.decode(auth_b64.as_bytes()))
        .ok()?;
    if key_bytes.len() != 65 || auth_bytes.len() != 16 {
        return None;
    }
    let subscriber_key = PublicKey::from_sec1_bytes(&key_bytes).ok()?;
    let auth: [u8; 16] = auth_bytes.try_into().ok()?;
    let origin = url::Url::parse(&endpoint)
        .ok()?
        .origin()
        .ascii_serialization();
    let mut salt = [0u8; 16];
    _ = getrandom::fill(&mut salt);
    let ephemeral = EphemeralSecret::generate();
    let body = aes128gcm_encrypt(payload, &subscriber_key, &auth, &salt, &ephemeral);
    let authorization = vapid_authorization(&keys.signing, &origin, now_unix(), subject);
    let _permit = IN_FLIGHT.acquire().await;
    Some(
        post(PushPost {
            endpoint,
            authorization,
            body,
            ttl_secs: note.ttl_secs,
            urgency: note.urgency,
            topic: note.topic.clone(),
        })
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::SecretKey;
    use p256::ecdh::diffie_hellman;
    use p256::ecdsa::Signature;
    use p256::ecdsa::signature::Verifier;

    fn hex(s: &str) -> Vec<u8> {
        s.bytes()
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }

    fn b64u(s: &str) -> Vec<u8> {
        B64U.decode(s.as_bytes()).unwrap()
    }

    // RFC 5869 A.1 (the basic case) and A.3 (zero-length salt and info) —
    // the two ends of the input space this hand-rolled HKDF serves. The
    // vectors are the point of hand-rolling: the RFC's own answers are the
    // only witness that the assembly is the algorithm.
    #[test]
    fn hkdf_reproduces_rfc5869() {
        let ikm = vec![0x0b; 22];

        // A.1: salt and info both present.
        let salt = hex("000102030405060708090a0b0c");
        let info = hex("f0f1f2f3f4f5f6f7f8f9");
        let prk = hkdf_extract(&salt, &ikm);
        assert_eq!(
            prk.to_vec(),
            hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
        );
        let okm = hkdf_expand(&prk, &info, 42);
        assert_eq!(
            okm,
            hex(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
            )
        );

        // A.3: both empty — extract falls back to the zero salt of the
        // hash's own length, exactly as the RFC spells it.
        let prk = hkdf_extract(&[], &ikm);
        assert_eq!(
            prk.to_vec(),
            hex("19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04")
        );
        let okm = hkdf_expand(&prk, &[], 42);
        assert_eq!(
            okm,
            hex(
                "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
            )
        );
    }

    // RFC 8291 §5 — the whole aes128gcm chain in one assertion set. The
    // ECDH leg is pinned first (the application server's private scalar
    // against the user agent's public point must give the RFC's shared
    // secret), then the shared secret drives the derivation-and-record
    // core, which must produce the RFC's posted body byte for byte: the
    // 86-octet header (salt, rs, the ephemeral point as key id) followed by
    // the one ciphertext of "When I grow up, I want to be a watermelon".
    #[test]
    fn rfc8291_vector_is_reproduced_byte_for_byte() {
        let as_private = b64u("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw");
        let ua_public = b64u(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let as_public = b64u(
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
        );
        let auth: [u8; 16] = b64u("BTBZMqHH6r4Tts7J_aSIgg").try_into().unwrap();
        let salt: [u8; 16] = b64u("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        assert_eq!(as_private.len(), 32);
        assert_eq!(ua_public.len(), 65);
        assert_eq!(as_public.len(), 65);

        // The ECDH leg: sender private × receiver public = the RFC's
        // shared secret ("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs").
        let secret = SecretKey::from_slice(&as_private).unwrap();
        let receiver = PublicKey::from_sec1_bytes(&ua_public).unwrap();
        let shared = diffie_hellman(secret.to_nonzero_scalar(), receiver.as_affine());
        assert_eq!(
            shared.raw_secret_bytes().to_vec(),
            b64u("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs")
        );

        // The whole rest: derivation + record, byte for byte.
        let subscriber: &[u8; 65] = ua_public.as_slice().try_into().unwrap();
        let ephemeral: &[u8; 65] = as_public.as_slice().try_into().unwrap();
        // The RFC spells the plaintext in unpadded standard alphabet; the
        // string is alphabet-agnostic, so the url-safe engine reads it too.
        let plaintext = b64u("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        let body = encrypt_with_shared(
            &plaintext,
            shared.raw_secret_bytes(),
            subscriber,
            ephemeral,
            &auth,
            &salt,
        );
        let want_header = b64u(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
        );
        let want_ct =
            b64u("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ");
        let mut want = want_header.clone();
        want.extend_from_slice(&want_ct);
        assert_eq!(body, want);
        // And the header's own anatomy, spelled out: the pinned salt, the
        // record size, and a 65-byte key id that IS the ephemeral point.
        assert_eq!(&body[..16], &salt);
        assert_eq!(&body[16..20], &4096u32.to_be_bytes());
        assert_eq!(body[20], 65);
        assert_eq!(&body[21..86], ephemeral.as_slice());
    }

    fn note(title: &str, body: &str, href: &str) -> PushNote {
        PushNote {
            id: "6f1c1c1e-0000-4000-8000-000000000001".into(),
            title: title.into(),
            body: body.into(),
            href: href.into(),
            kind: "plan-share".into(),
            renotify: false,
            created_at: "2026-10-31T12:00:00.000Z".into(),
            ..Default::default()
        }
    }

    // The ceiling every push service guarantees, whatever a writer hands
    // over: a 2,500-character plan title (the reproduced 413), a body of
    // escaped quotes, a maximal href. The plaintext stays valid JSON under
    // 3,000 bytes, so the encrypted record stays under 4,096.
    #[test]
    fn the_payload_fits_the_push_ceiling_for_any_writer() {
        let cases = [
            note(&"Ü".repeat(2500), "", "/plan/x"),
            note("t", &"\"".repeat(5000), "/plan/x"),
            note(
                &"😀".repeat(900),
                &"😀".repeat(900),
                &format!("/{}", "a".repeat(2047)),
            ),
            note("", "", ""),
        ];
        for n in &cases {
            let out = push_payload(n);
            assert!(out.len() <= PAYLOAD_MAX_BYTES, "{} bytes", out.len());
            let v: Value = serde_json::from_slice(&out).expect("valid JSON");
            assert_eq!(v["id"], n.id.as_str());
            // The encrypted body: header 86 + plaintext + delimiter + tag.
            assert!(86 + out.len() + 1 + 16 <= 4096);
        }
        // An ordinary note goes out whole, with the new hints.
        let v: Value = serde_json::from_slice(&push_payload(&note("Hi", "there", "/a"))).unwrap();
        assert_eq!(v["title"], "Hi");
        assert_eq!(v["body"], "there");
        assert_eq!(v["kind"], "plan-share");
        assert_eq!(v["renotify"], false);
        assert_eq!(v["createdAt"], "2026-10-31T12:00:00.000Z");
        // The clamp is 200 title chars, with an ellipsis only when cut.
        let v: Value =
            serde_json::from_slice(&push_payload(&note(&"a".repeat(300), "", "/"))).unwrap();
        assert_eq!(v["title"].as_str().unwrap().chars().count(), 200);
        assert!(v["title"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn the_413_resend_is_id_title_and_href_only() {
        let v: Value =
            serde_json::from_slice(&minimal_payload(&note(&"b".repeat(500), "body", "/x")))
                .unwrap();
        let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["href", "id", "title"]);
        assert_eq!(v["title"].as_str().unwrap().chars().count(), 80);
        assert_eq!(v["href"], "/x");
    }

    fn answer(status: u16, retry_after: Option<u64>, body: &str) -> PushResult {
        Ok(PushAnswer {
            status,
            retry_after,
            body: body.into(),
        })
    }

    #[test]
    fn retries_are_for_throttles_outages_and_transport_only() {
        let p = PushPolicy::default();
        let s2 = Duration::from_secs(2);
        let s8 = Duration::from_secs(8);
        // 5xx with no Retry-After: 2 s then 8 s, then final (3 attempts).
        assert_eq!(retry_delay(0, &answer(500, None, ""), &p), Some(s2));
        assert_eq!(retry_delay(1, &answer(503, None, ""), &p), Some(s8));
        assert_eq!(retry_delay(2, &answer(500, None, ""), &p), None);
        // 429 names its wait; it is honored within the cap.
        assert_eq!(retry_delay(0, &answer(429, Some(2), ""), &p), Some(s2));
        assert_eq!(
            retry_delay(0, &answer(429, Some(60), ""), &p),
            Some(Duration::from_secs(60))
        );
        assert_eq!(retry_delay(0, &answer(429, Some(61), ""), &p), None);
        // Transport: a timeout or a refused connect retries; anything else
        // the transport refused does not.
        assert_eq!(
            retry_delay(0, &Err(PushFailure::Timeout("t".into())), &p),
            Some(s2)
        );
        assert_eq!(
            retry_delay(1, &Err(PushFailure::Connect("c".into())), &p),
            Some(s8)
        );
        // A timeout is retried ONCE: a hung endpoint holds a permit for the
        // whole 15 s bound every try (two tries at most, ≤ 2 × 15 s).
        assert_eq!(
            retry_delay(1, &Err(PushFailure::Timeout("t".into())), &p),
            None
        );
        assert_eq!(
            retry_delay(0, &Err(PushFailure::Other("o".into())), &p),
            None
        );
        // A refused endpoint is a verdict, not a hiccup: never retried.
        assert_eq!(
            retry_delay(0, &Err(PushFailure::Refused(ENDPOINT_REFUSED.into())), &p),
            None
        );
        // Final answers are final.
        for code in [201, 400, 403, 404, 410, 413] {
            assert_eq!(retry_delay(0, &answer(code, None, ""), &p), None, "{code}");
        }
    }

    // The backoff window's numbers live in SQL; the consts name them.
    #[test]
    fn the_backoff_window_is_the_named_rule() {
        // A delivery reads at most the per-person cap, newest-seen first.
        assert!(DELIVERY_ROWS.ends_with(&format!(
            "order by last_seen_at desc, created_at desc, id limit {DEVICES_PER_PERSON}"
        )));
        assert!(DELIVERY_ROWS.contains(&format!("failures >= {FAILURES_TO_SKIP}")));
        assert!(DELIVERY_ROWS.contains(&format!(", {SKIP_CAP_MIN}) * interval '1 minute'")));
        assert!(DELIVERY_ROWS.contains("last_error like 'timeout:%'"));
        // …and the timeout prefix it matches is the one error_text writes.
        let e = error_text(&Err(PushFailure::Timeout("operation timed out".into()))).unwrap();
        assert!(e.starts_with("timeout:"), "{e}");
        // Control characters never reach the row.
        let e = error_text(&Ok(PushAnswer {
            status: 500,
            retry_after: None,
            body: "bad\u{7}\r\nthing".into(),
        }))
        .unwrap();
        assert_eq!(e, "500 badthing");
    }

    #[test]
    fn the_test_push_is_a_test_kind_with_a_short_life() {
        let n = test_note("Acme", 1_767_268_800_123);
        assert_eq!(n.id, "talaria-test-1767268800123");
        assert_eq!(n.kind, "test");
        assert_eq!(n.title, "Talaria test notification");
        assert_eq!(n.body, "Acme: this device gets alerts");
        assert_eq!(n.href, "/settings/notifications");
        assert_eq!(n.created_at, "2026-01-01T12:00:00.123Z");
        assert_eq!((n.ttl_secs, n.urgency), (Some(300), Some(Urgency::High)));
        let p: serde_json::Value = serde_json::from_slice(&push_payload(&n)).unwrap();
        assert_eq!(p["kind"], "test");
        assert_eq!(iso_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_ms(1_798_761_599_999), "2026-12-31T23:59:59.999Z");
    }

    #[test]
    fn row_tending_by_answer() {
        assert_eq!(classify(&answer(201, None, "")), Tend::Delivered);
        assert_eq!(classify(&answer(404, None, "")), Tend::Gone);
        assert_eq!(classify(&answer(410, None, "")), Tend::Gone);
        assert_eq!(classify(&answer(403, None, "")), Tend::Refused);
        assert_eq!(classify(&answer(401, None, "unauthorized")), Tend::Refused);
        // Apple's refusal of the instance's own token is not the device's.
        assert_eq!(
            classify(&answer(403, None, "{\"reason\":\"BadJwtToken\"}")),
            Tend::InstanceFault
        );
        assert_eq!(
            classify(&answer(403, None, "{\"reason\":\"ExpiredProviderToken\"}")),
            Tend::InstanceFault
        );
        assert_eq!(classify(&answer(500, None, "")), Tend::Failed);
        assert_eq!(classify(&answer(413, None, "")), Tend::Failed);
        assert_eq!(classify(&answer(429, None, "")), Tend::Failed);
        // A refusal is counted and kept like any failure; the error it
        // stores says what happened and nothing about where.
        let refused: PushResult = Err(PushFailure::Refused(ENDPOINT_REFUSED.into()));
        assert_eq!(classify(&refused), Tend::Failed);
        assert_eq!(
            error_text(&refused).as_deref(),
            Some("refused: not a public push service address")
        );
        assert_eq!(
            classify(&Err(PushFailure::Timeout("t".into()))),
            Tend::Failed
        );
    }

    #[test]
    fn the_subject_prefers_the_public_https_url() {
        assert_eq!(
            pick_subject(
                Some("https://talaria.example.com/"),
                Some("https://other.example")
            ),
            "https://talaria.example.com"
        );
        // A plain-http public URL is not a contact a push service accepts.
        assert_eq!(
            pick_subject(
                Some("http://192.168.0.9:5273"),
                Some("https://talaria.example.com")
            ),
            "https://talaria.example.com"
        );
        assert_eq!(pick_subject(None, None), VAPID_SUB_MAILTO);
        assert_eq!(pick_subject(Some(""), None), VAPID_SUB_MAILTO);
    }

    // The VAPID half: a JWT a push service can verify, carrying exactly the
    // three claims RFC 8292 names, signed over the exact input string, with
    // the raw r‖s (not DER) signature shape.
    #[test]
    fn vapid_jwt_verifies_and_carries_the_right_claims() {
        let signing = SigningKey::generate();
        let now = 1_700_000_000i64;
        let jwt = vapid_jwt(
            &signing,
            "https://push.example.example",
            now,
            VAPID_SUB_MAILTO,
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            String::from_utf8(b64u(parts[0])).unwrap(),
            "{\"alg\":\"ES256\",\"typ\":\"JWT\"}"
        );
        let claims = String::from_utf8(b64u(parts[1])).unwrap();
        assert!(claims.contains("\"aud\":\"https://push.example.example\""));
        assert!(claims.contains(&format!("\"exp\":{}", now + VAPID_TTL_SECS)));
        assert!(claims.contains(&format!("\"sub\":\"{VAPID_SUB_MAILTO}\"")));

        // The signature verifies against the signing input, in the raw
        // 64-byte shape.
        let raw = b64u(parts[2]);
        assert_eq!(raw.len(), 64);
        let sig = Signature::from_slice(&raw).unwrap();
        let input = format!("{}.{}", parts[0], parts[1]);
        signing
            .verifying_key()
            .verify(input.as_bytes(), &sig)
            .expect("the jwt verifies");

        // The Authorization header pairs it with the UNCOMPRESSED public
        // key — the same 65 bytes a service decodes to check it.
        let authz = vapid_authorization(
            &signing,
            "https://push.example.example",
            now,
            VAPID_SUB_MAILTO,
        );
        let point = signing.verifying_key().to_sec1_point(false);
        let want_k = B64U.encode(point.as_bytes());
        assert!(authz.starts_with("vapid t="));
        assert!(authz.ends_with(&format!(", k={want_k}")));
        assert_eq!(point.as_bytes().len(), 65);
    }

    // Wave 1's headers when a note carries none; a carried value wins; a
    // topic the services would refuse is dropped, not sent.
    #[test]
    fn delivery_headers_default_to_wave_ones() {
        let post = |ttl, urgency, topic: Option<&str>| PushPost {
            endpoint: "https://push.example/x".into(),
            authorization: "vapid t=x, k=y".into(),
            body: vec![],
            ttl_secs: ttl,
            urgency,
            topic: topic.map(str::to_string),
        };
        assert_eq!(
            post_headers(&post(None, None, None)),
            vec![
                ("TTL", "3600".to_string()),
                ("Urgency", "normal".to_string())
            ]
        );
        assert_eq!(
            post_headers(&post(Some(60), Some(Urgency::High), Some("dm-c1"))),
            vec![
                ("TTL", "60".to_string()),
                ("Urgency", "high".to_string()),
                ("Topic", "dm-c1".to_string())
            ]
        );
        assert_eq!(post_headers(&post(None, None, Some("has space"))).len(), 2);
        assert_eq!(
            post_headers(&post(None, None, Some(&"a".repeat(33)))).len(),
            2
        );
        assert_eq!(Urgency::VeryLow.as_header(), "very-low");
    }

    #[test]
    fn devices_are_named_from_their_user_agent() {
        for (ua, label) in [
            (
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
                "Chrome on Linux",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:144.0) Gecko/20100101 Firefox/144.0",
                "Firefox on Linux",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15",
                "Safari on macOS",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
                "Edge on Windows",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1",
                "Safari on iOS",
            ),
            (
                "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            ("curl/8.9", "Browser"),
        ] {
            assert_eq!(device_label(Some(ua)), label, "{ua}");
        }
        assert_eq!(device_label(None), "Browser");
        assert_eq!(stored_user_agent(Some("  \u{7}  ")), None);
        assert_eq!(
            stored_user_agent(Some(&"x".repeat(400))).unwrap().len(),
            300
        );
    }

    // An endpoint's answer is attacker-shaped: no markup, no control
    // characters, at most 120 characters reach a client.
    #[test]
    fn a_device_error_is_plain_and_short() {
        let planted = format!(
            "500 <html><head><script>alert(1)</script></head><body>\u{1b}[31m{}\r\nbad\u{0}</body></html>",
            "x".repeat(480)
        );
        let e = plain_error(&planted);
        assert!(e.chars().count() <= 120, "{}", e.chars().count());
        assert!(!e.contains('<') && !e.contains('>'));
        assert!(!e.chars().any(char::is_control));
        assert!(e.starts_with("500 alert(1) [31mxxx"), "{e}");
        assert_eq!(plain_error("410  Gone\n"), "410 Gone");
    }

    // ── The edge's endpoint policy (security-1) ─────────────────────────────

    /// A loopback listener that answers every connection with `reply` and
    /// counts them. Returns its port and the count.
    async fn listener(
        reply: impl Fn(u16) -> Vec<u8> + Send + Sync + 'static,
    ) -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener binds");
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(&reply(port)).await;
                let _ = sock.shutdown().await;
            }
        });
        (port, hits)
    }

    fn post_to(endpoint: String) -> PushPost {
        PushPost {
            endpoint,
            authorization: "vapid t=x, k=y".into(),
            body: vec![0u8; 16],
            ttl_secs: None,
            urgency: None,
            topic: None,
        }
    }

    /// The SSRF the release sweep reproduced: a member files an endpoint
    /// inside the server's network, fires the test push, and reads back what
    /// answered. The production edge must refuse every such endpoint WITHOUT
    /// dialling it — the loopback listener is never reached — and say the
    /// same refusal for each, naming no address.
    #[tokio::test]
    async fn the_edge_never_dials_an_endpoint_inside_the_network() {
        let (port, hits) =
            listener(|_| b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let edge = real_post_push();
        for endpoint in [
            format!("https://127.0.0.1:{port}/push"),
            format!("https://localhost:{port}/push"),
            format!("https://[::1]:{port}/push"),
            format!("https://[::ffff:127.0.0.1]:{port}/push"),
            format!("http://127.0.0.1:{port}/push"),
            "https://169.254.169.254/latest/meta-data/".to_string(),
            "https://10.255.255.1/x".to_string(),
            "https://192.168.254.254/x".to_string(),
            "https://box.internal/x".to_string(),
        ] {
            let got = edge(post_to(endpoint.clone())).await;
            assert_eq!(
                got.as_ref().err().map(ToString::to_string).as_deref(),
                Some("refused: not a public push service address"),
                "{endpoint} -> {got:?}"
            );
        }
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no inside endpoint was ever dialled"
        );
    }

    /// No redirect is followed: a push service answers 201, so a 3xx comes
    /// back as the answer and its Location is never dialled. (The client is
    /// driven directly against a loopback LITERAL — dialled without the
    /// resolver — because the policy in front of it refuses loopback.)
    #[tokio::test]
    async fn the_push_client_hands_a_redirect_back_instead_of_following_it() {
        let (port, hits) = listener(|port| {
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes()
        })
        .await;
        let got = send_push(
            &push_client(),
            post_to(format!("http://127.0.0.1:{port}/push")),
        )
        .await;
        assert_eq!(got.map(|a| a.status), Ok(302), "the 302 is the answer");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the Location was never dialled"
        );
    }

    /// The connect-time half: a NAME the pre-flight passed but that resolves
    /// inside when the socket dials is refused by the validating resolver,
    /// and reads as the same refusal. (`localhost` resolves to loopback on
    /// every host; the client alone has no name rule, only the resolver.)
    #[tokio::test]
    async fn the_push_client_refuses_an_inside_address_at_connect_time() {
        let (port, hits) =
            listener(|_| b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let got = send_push(
            &push_client(),
            post_to(format!("http://localhost:{port}/push")),
        )
        .await;
        assert_eq!(got, Err(PushFailure::Refused(ENDPOINT_REFUSED.into())));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// An answer's body is read only as far as the reason code needs: a
    /// service that answers a refusal with a megabyte is cut at 512
    /// characters, not buffered whole.
    #[tokio::test]
    async fn an_answer_body_is_read_only_as_far_as_its_reason() {
        let (port, _) = listener(|_| {
            let mut r = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 1048576\r\n\r\n".to_vec();
            r.extend(std::iter::repeat_n(b'x', 1 << 20));
            r
        })
        .await;
        let got = send_push(
            &push_client(),
            post_to(format!("http://127.0.0.1:{port}/push")),
        )
        .await
        .expect("an answer");
        assert_eq!(got.status, 400);
        assert_eq!(got.body.chars().count(), ANSWER_BODY_CHARS);
    }

    /// The policy the subscribe door and the edge share: a public address.
    /// Literals and internal names never touch DNS; a name nothing answers
    /// for is a transport failure, not a verdict. (https is the door's own
    /// rule, pinned in its validation test.)
    #[tokio::test]
    async fn the_endpoint_policy_wants_a_public_address() {
        assert_eq!(
            check_endpoint("https://8.8.8.8/fcm/send/x").await,
            EndpointVerdict::Public
        );
        for refused in [
            "ftp://8.8.8.8/x",
            "not a url",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://10.0.0.5/x",
            "https://172.16.0.1/x",
            "https://192.168.1.1/x",
            "https://100.64.0.1/x",
            "https://[fd00::1]/x",
            "https://localhost/x",
            "https://push.local/x",
            "https://box.internal/x",
        ] {
            assert!(
                matches!(check_endpoint(refused).await, EndpointVerdict::Refused(_)),
                "{refused}"
            );
        }
        // RFC 6761: .invalid never resolves.
        assert!(matches!(
            check_endpoint("https://push.invalid/x").await,
            EndpointVerdict::Unresolved(_)
        ));
    }
}
