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
// a user from the single notification writer. 2xx touches last_seen_at,
// 404/410 deletes the row (the service already forgot it), anything else
// logs and keeps.

use std::sync::{Arc, OnceLock};

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

use crate::secretbox::SecretBox;

type HmacSha256 = Hmac<Sha256>;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// app_settings key for the VAPID keypair row. Pub because the live suite
/// cleans the row by name — the key IS the plane's storage contract.
pub const VAPID_KEY: &str = "push_vapid";
/// The JWT's `sub` — RFC 8292 wants an operator contact the push service
/// could reach; it is informational, never routed to.
const VAPID_SUB_MAILTO: &str = "mailto:notifications@talaria.local";
/// JWT lifetime: long enough for slow deliveries, short enough that a leaked
/// token dies on its own (RFC 8292 suggests ≥ 12h tolerance, services
/// commonly cap at 24h).
const VAPID_TTL_SECS: i64 = 12 * 60 * 60;
/// aes128gcm's record size — comfortably above any payload this plane sends
/// (the notification body is truncated to 200 UTF-16 units before it ever
/// gets here), so the single record the scheme allows stays single.
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
pub fn vapid_jwt(signing: &SigningKey, audience_origin: &str, now_unix: i64) -> String {
    let header = "{\"alg\":\"ES256\",\"typ\":\"JWT\"}";
    let claims = format!(
        "{{\"aud\":\"{audience_origin}\",\"exp\":{exp},\"sub\":\"{VAPID_SUB_MAILTO}\"}}",
        exp = now_unix + VAPID_TTL_SECS,
    );
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
pub fn vapid_authorization(signing: &SigningKey, audience_origin: &str, now_unix: i64) -> String {
    let jwt = vapid_jwt(signing, audience_origin, now_unix);
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
    Ok(Some(VapidKeys { signing, public }))
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

// ── The send edge (injectable, the DrainDeps pattern) ────────────────────────

/// One delivery as the plane sees it: where, with what headers, carrying
/// what ciphertext. The fake edge in tests answers by endpoint.
pub struct PushPost {
    pub endpoint: String,
    pub authorization: String,
    pub body: Vec<u8>,
}

pub type PostPushFn =
    Arc<dyn Fn(PushPost) -> BoxFuture<'static, Result<u16, String>> + Send + Sync>;

/// The production edge over reqwest (rustls — the no-OpenSSL invariant is
/// the whole reason this module is hand-rolled). One client, built once:
/// pushes are rare, but a client per delivery would pay the pool tax every
/// time.
pub fn real_post_push() -> PostPushFn {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new).clone();
    Arc::new(move |post: PushPost| {
        let client = client.clone();
        Box::pin(async move {
            let resp = client
                .post(&post.endpoint)
                .header("Authorization", &post.authorization)
                .header("TTL", "3600")
                .header("Urgency", "normal")
                .header("Content-Encoding", "aes128gcm")
                .header("Content-Type", "application/octet-stream")
                .body(post.body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            Ok(resp.status().as_u16())
        })
    })
}

// ── Delivery ─────────────────────────────────────────────────────────────────

/// The row-shaped notification the plane sends — everything the service
/// worker needs to show AND to navigate on click.
pub struct PushNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub href: String,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Send one note to every device browser of one person. Per subscription:
/// encrypt against ITS keys, prove with the instance VAPID identity, POST,
/// then tend the row — 2xx touches liveness, 404/410 prunes (the service
/// has already forgotten the subscription), anything else logs and keeps
/// for the next notification to retry. A subscription whose row cannot be
/// encrypted (bad key material) is pruned too: it can never succeed.
pub async fn deliver_push(
    pg: &PgPool,
    sb: &SecretBox,
    post: &PostPushFn,
    user_id: &str,
    note: &PushNote,
) {
    let rows: Vec<(String, String, String)> = match sqlx::query_as(
        "select endpoint, p256dh, auth from push_subscriptions where user_id = $1::uuid",
    )
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
    let payload = serde_json::to_vec(&serde_json::json!({
        "id": note.id,
        "title": note.title,
        "body": note.body,
        "href": note.href,
    }))
    .expect("plain strings serialize");
    for (endpoint, p256dh_b64, auth_b64) in rows {
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
                let _ = sqlx::query("delete from push_subscriptions where endpoint = $1")
                    .bind(&endpoint)
                    .execute(pg)
                    .await;
                continue;
            }
        };
        let Ok(subscriber_key) = PublicKey::from_sec1_bytes(&key_bytes) else {
            tracing::warn!("[push] a subscription's p256dh is not a p256 point; pruning");
            let _ = sqlx::query("delete from push_subscriptions where endpoint = $1")
                .bind(&endpoint)
                .execute(pg)
                .await;
            continue;
        };
        let auth: [u8; 16] = auth_bytes.try_into().expect("len checked above");
        let Ok(url) = url::Url::parse(&endpoint) else {
            tracing::warn!("[push] a subscription endpoint does not parse; pruning");
            let _ = sqlx::query("delete from push_subscriptions where endpoint = $1")
                .bind(&endpoint)
                .execute(pg)
                .await;
            continue;
        };
        let origin = url.origin().ascii_serialization();
        let salt: [u8; 16] = {
            let mut s = [0u8; 16];
            _ = getrandom::fill(&mut s);
            s
        };
        let ephemeral = EphemeralSecret::generate();
        let body = aes128gcm_encrypt(&payload, &subscriber_key, &auth, &salt, &ephemeral);
        let authorization = vapid_authorization(&keys.signing, &origin, now_unix());
        let outcome = post(PushPost {
            endpoint: endpoint.clone(),
            authorization,
            body,
        })
        .await;
        match outcome {
            Ok(code) if (200..300).contains(&code) => {
                let _ = sqlx::query(
                    "update push_subscriptions set last_seen_at = now() where endpoint = $1",
                )
                .bind(&endpoint)
                .execute(pg)
                .await;
            }
            Ok(404) | Ok(410) => {
                // The push service has forgotten this subscription — the
                // browser unregistered, was reset, or expired it. The row's
                // only remaining job is making every future delivery fail.
                let _ = sqlx::query("delete from push_subscriptions where endpoint = $1")
                    .bind(&endpoint)
                    .execute(pg)
                    .await;
            }
            Ok(code) => {
                tracing::warn!("[push] delivery to an endpoint answered {code}; keeping the row");
            }
            Err(e) => {
                tracing::warn!("[push] delivery to an endpoint failed: {e}");
            }
        }
    }
}

// ── The plane: installed once at boot, consulted by the single writer ────────

struct Plane {
    pg: PgPool,
    sb: SecretBox,
    post: PostPushFn,
}

static PLANE: OnceLock<Plane> = OnceLock::new();

/// Install the push plane at boot (jobs.rs, beside real_drain_deps — the
/// secretbox loads through AppState there). Not installed = push off: the
/// hook below is a no-op, which is exactly what tests and any
/// push-less deployment want.
pub fn install_push_plane(pg: PgPool, sb: SecretBox) {
    let _ = PLANE.set(Plane {
        pg,
        sb,
        post: real_post_push(),
    });
}

/// The single writer's hook: every notification row that lands with an
/// in-app or both route also reaches closed tabs. Detached — a push service
///'s latency must never ride a request — and quiet: the row in the database
/// is the record, the delivery is best-effort on top of it.
pub fn push_notification(user_id: &str, note: PushNote) {
    let Some(plane) = PLANE.get() else { return };
    let pg = plane.pg.clone();
    let sb = plane.sb.clone();
    let post = plane.post.clone();
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        deliver_push(&pg, &sb, &post, &user_id, &note).await;
    });
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

    // The VAPID half: a JWT a push service can verify, carrying exactly the
    // three claims RFC 8292 names, signed over the exact input string, with
    // the raw r‖s (not DER) signature shape.
    #[test]
    fn vapid_jwt_verifies_and_carries_the_right_claims() {
        let signing = SigningKey::generate();
        let now = 1_700_000_000i64;
        let jwt = vapid_jwt(&signing, "https://push.example.example", now);
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
        let authz = vapid_authorization(&signing, "https://push.example.example", now);
        let point = signing.verifying_key().to_sec1_point(false);
        let want_k = B64U.encode(point.as_bytes());
        assert!(authz.starts_with("vapid t="));
        assert!(authz.ends_with(&format!(", k={want_k}")));
        assert_eq!(point.as_bytes().len(), 65);
    }
}
