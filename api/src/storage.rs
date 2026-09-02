// Object storage for upload blobs — the verbs behind `uploads.path`. A
// transcript re-reads a message's textual attachments (getUpload →
// readBlob), so a read has to find the bytes wherever they live — local
// disk, the configured bucket, or the replica behind it. The write side
// (s3_put/s3_delete, ensure_bucket, the admin probe, the config writes)
// serves the uploads family and the admin storage surface.
//
// No SDK, by house rule: any S3-compatible endpoint (AWS, B2, R2, MinIO, …)
// behind hand-rolled SigV4 verbs, the same no-SDK fetch pattern as the
// gateway's providers. Config lives in app_settings (`storage_config`) with
// the secret sealed by secretbox; uploads.path records where each blob
// actually lives (`s3://bucket/key` vs a filesystem path), so a read by
// recorded path never strands a blob written under a different mode.

use regex::Regex;
use sqlx::PgPool;
use std::sync::LazyLock;

use crate::gateway::provider::http;
use crate::gateway::settings::get_setting;
use crate::secretbox::SecretBox;

const KEY: &str = "storage_config";

/// The published dev secret — the fallback the bundled MinIO container runs
/// on. Read paths stay unguarded on purpose (refusing them would brick blobs
/// already stored); the refusal lives at the write-time doors
/// (`refuse_dev_secret`).
pub const DEV_S3_SECRET: &str = "talaria-dev-secret";

/// One bucket to talk to. `secret_access_key` is UNSEALED in memory here;
/// it is never logged and never serialized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketTarget {
    pub endpoint: String,
    /// Blank = derived from the endpoint host (region_for), else us-east-1.
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// true works everywhere (B2, R2, MinIO); false = virtual-host style.
    pub path_style: bool,
    /// Key prefix inside the bucket, e.g. "talaria/".
    pub prefix: String,
}

impl BucketTarget {
    /// Fully configured: the four fields a signed request cannot live
    /// without.
    fn ready(&self) -> bool {
        !self.endpoint.is_empty()
            && !self.bucket.is_empty()
            && !self.access_key_id.is_empty()
            && !self.secret_access_key.is_empty()
    }
}

/// Ready to sign — the four fields a signed request cannot live without.
pub fn target_ready(t: &BucketTarget) -> bool {
    t.ready()
}

/// The published dev password may never gate production bytes —
/// NODE_ENV=production is the production posture.
pub fn refuse_dev_secret(t: &BucketTarget) -> Result<(), String> {
    if std::env::var("NODE_ENV").as_deref() == Ok("production")
        && t.secret_access_key == DEV_S3_SECRET
    {
        return Err(
            "internal storage refused: TALARIA_S3_SECRET_KEY is unset in production, and the fallback is the published \
             dev password. Set a real secret (openssl rand -hex 24) and update the minio container to match."
                .into(),
        );
    }
    Ok(())
}

/// Where writes go: the internal container, the configured bucket, or None
/// for local disk. Internal ensures its bucket exists (idempotent,
/// process-cached).
pub async fn active_target(cfg: &StorageConfig) -> Result<Option<(BucketTarget, bool)>, String> {
    if cfg.mode == "internal" {
        let target = internal_target();
        refuse_dev_secret(&target)?;
        ensure_bucket(&target).await?;
        return Ok(Some((target, true)));
    }
    if cfg.mode == "s3" && target_ready(&cfg.target) {
        return Ok(Some((cfg.target.clone(), false)));
    }
    Ok(None)
}

/// The enabled + fully-configured replica, if any.
pub fn replica_target(cfg: &StorageConfig) -> Option<BucketTarget> {
    if cfg.replica_enabled && target_ready(&cfg.replica) {
        Some(cfg.replica.clone())
    } else {
        None
    }
}

/// The whole config: the primary target's fields flat, the mode, and the
/// optional replica — the exact shape `storage_config` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// 'local' | 'internal' | 's3'.
    pub mode: String,
    pub target: BucketTarget,
    pub replica: BucketTarget,
    pub replica_enabled: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            mode: "local".into(),
            target: BucketTarget {
                path_style: true,
                ..BucketTarget::default()
            },
            replica: BucketTarget {
                path_style: true,
                ..BucketTarget::default()
            },
            replica_enabled: false,
        }
    }
}

/// The bundled MinIO container. Defaults match docker/dev-compose.yml; the
/// TALARIA_S3_* and TALARIA_MINIO_PORT vars override each leg.
pub fn internal_target() -> BucketTarget {
    fn env_or(key: &str, fallback: &str) -> String {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| fallback.into())
    }
    let port = env_or("TALARIA_MINIO_PORT", "9010");
    BucketTarget {
        endpoint: std::env::var("TALARIA_S3_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
        region: "us-east-1".into(),
        bucket: env_or("TALARIA_S3_BUCKET", "talaria"),
        access_key_id: env_or("TALARIA_S3_ACCESS_KEY", "talaria"),
        secret_access_key: env_or("TALARIA_S3_SECRET_KEY", DEV_S3_SECRET),
        path_style: true,
        prefix: String::new(),
    }
}

/// The raw jsonb as stored, before defaults are applied — a map so the
/// merge is field-wise (present keys override, absent keys keep the
/// default; the `null`-override corner is noted at the merge).
fn str_field(raw: &serde_json::Map<String, serde_json::Value>, key: &str, default: &str) -> String {
    raw.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

/// The merge: defaults filled out with whatever the row says, nested
/// replica merged the same way. A present `null` reads the default here
/// rather than overriding to null — the difference is observable only
/// through the emptiness `ready()` asks for, and a null and a default-empty
/// string answer every question the same way (except pathStyle, which the
/// admin panel writes as a real boolean).
fn merged(raw: &serde_json::Value) -> StorageConfig {
    let d = StorageConfig::default();
    let Some(obj) = raw.as_object() else {
        return d;
    };
    let replica_raw = obj.get("replica").and_then(|v| v.as_object());
    let replica_of = |k: &str| match replica_raw {
        Some(r) => str_field(r, k, ""),
        None => String::new(),
    };
    StorageConfig {
        mode: str_field(obj, "mode", "local"),
        target: BucketTarget {
            endpoint: str_field(obj, "endpoint", &d.target.endpoint),
            region: str_field(obj, "region", &d.target.region),
            bucket: str_field(obj, "bucket", &d.target.bucket),
            access_key_id: str_field(obj, "accessKeyId", &d.target.access_key_id),
            secret_access_key: str_field(obj, "secretAccessKey", &d.target.secret_access_key),
            path_style: obj
                .get("pathStyle")
                .and_then(|v| v.as_bool())
                .unwrap_or(d.target.path_style),
            prefix: str_field(obj, "prefix", &d.target.prefix),
        },
        replica: BucketTarget {
            endpoint: replica_of("endpoint"),
            region: replica_of("region"),
            bucket: replica_of("bucket"),
            access_key_id: replica_of("accessKeyId"),
            secret_access_key: replica_of("secretAccessKey"),
            path_style: replica_raw
                .and_then(|r| r.get("pathStyle"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            prefix: replica_of("prefix"),
        },
        replica_enabled: replica_raw
            .and_then(|r| r.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// The unseal step a non-empty secret goes through. A cipher that does not
/// open is EMPTY, which makes the target not-ready — the same fail-closed
/// posture as a never-configured bucket.
fn unseal(sb: &SecretBox, token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    sb.open(token).unwrap_or_default()
}

/// The row (or {}), merged over defaults, secrets unsealed.
pub async fn get_storage_config(pg: &PgPool, sb: &SecretBox) -> StorageConfig {
    let raw = get_setting(pg, KEY, serde_json::json!({})).await;
    let mut cfg = merged(&raw);
    cfg.target.secret_access_key = unseal(sb, &cfg.target.secret_access_key);
    cfg.replica.secret_access_key = unseal(sb, &cfg.replica.secret_access_key);
    cfg
}

/// The whole config written, secrets SEALED at rest. An empty secret stays
/// empty (local mode has none to keep).
pub async fn set_storage_config(pg: &PgPool, sb: &SecretBox, cfg: &StorageConfig) {
    let seal = |plain: &str| -> String {
        if plain.is_empty() {
            String::new()
        } else {
            sb.seal(plain).unwrap_or_default()
        }
    };
    let t = &cfg.target;
    let r = &cfg.replica;
    let value = serde_json::json!({
        "mode": cfg.mode,
        "endpoint": t.endpoint,
        "region": t.region,
        "bucket": t.bucket,
        "accessKeyId": t.access_key_id,
        "secretAccessKey": seal(&t.secret_access_key),
        "pathStyle": t.path_style,
        "prefix": t.prefix,
        "replica": {
            "enabled": cfg.replica_enabled,
            "endpoint": r.endpoint,
            "region": r.region,
            "bucket": r.bucket,
            "accessKeyId": r.access_key_id,
            "secretAccessKey": seal(&r.secret_access_key),
            "pathStyle": r.path_style,
            "prefix": r.prefix,
        },
    });
    let _ = crate::gateway::settings::set_setting(pg, KEY, &value).await;
}

/// The admin GET's masked view: never the secrets, only whether each is set.
/// `hasSecret` rides last in each object; the replica object follows the
/// primary's shape.
pub async fn public_storage_config(pg: &PgPool, sb: &SecretBox) -> serde_json::Value {
    let cfg = get_storage_config(pg, sb).await;
    let t = &cfg.target;
    let r = &cfg.replica;
    serde_json::json!({
        "mode": cfg.mode,
        "endpoint": t.endpoint,
        "region": t.region,
        "bucket": t.bucket,
        "accessKeyId": t.access_key_id,
        "pathStyle": t.path_style,
        "prefix": t.prefix,
        "hasSecret": !t.secret_access_key.is_empty(),
        "replica": {
            "enabled": cfg.replica_enabled,
            "endpoint": r.endpoint,
            "region": r.region,
            "bucket": r.bucket,
            "accessKeyId": r.access_key_id,
            "pathStyle": r.path_style,
            "prefix": r.prefix,
            "hasSecret": !r.secret_access_key.is_empty(),
        },
    })
}

// ── URL + region ─────────────────────────────────────────────────────────────

/// Derive the region when the field is blank: B2 and AWS embed it in the
/// host (s3.us-west-004.backblazeb2.com, s3.eu-central-1.amazonaws.com);
/// R2 wants the literal "auto"; everything else falls back to us-east-1, which
/// MinIO also accepts.
pub fn region_for(t: &BucketTarget) -> String {
    let region = t.region.trim();
    if !region.is_empty() {
        return region.to_string();
    }
    let host = host_of(&t.endpoint);
    static HOST_REGION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^s3\.([a-z0-9-]+)\.(backblazeb2\.com|amazonaws\.com)$").unwrap()
    });
    if let Some(m) = HOST_REGION.captures(&host)
        && let Some(r) = m.get(1)
    {
        return r.as_str().to_string();
    }
    if host.ends_with(".r2.cloudflarestorage.com") {
        return "auto".into();
    }
    "us-east-1".into()
}

/// host[:port] of the endpoint, or '' when there is nothing parseable.
fn host_of(endpoint: &str) -> String {
    let rest = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint);
    rest.split(['/', '?', '#']).next().unwrap_or("").to_string()
}

/// RFC 3986-encode a key, keeping `/` as the segment separator. Everything
/// but the unreserved set — including !'()* — percent-encodes uppercase,
/// the S3 canonical form.
fn encode_key(key: &str) -> String {
    fn enc_seg(seg: &str, out: &mut String) {
        for b in seg.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, seg) in key.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        enc_seg(seg, &mut out);
    }
    out
}

/// The request URL and the EXACT path string the canonical request signs.
/// The path is built, not read back off a parsed URL: the canonical request
/// must carry the encoded form exactly as constructed, and re-reading it off
/// a url crate parse invites normalization differences to drift in.
fn object_url(t: &BucketTarget, key: &str) -> (String, String, String) {
    let endpoint = &t.endpoint;
    let scheme = if endpoint.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let after_scheme = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let base_path = after_scheme[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let base_path = base_path.strip_suffix('/').unwrap_or(base_path);
    let encoded = encode_key(key);
    if t.path_style {
        let path = if key.is_empty() {
            format!("{base_path}/{}", t.bucket)
        } else {
            format!("{base_path}/{}/{}", t.bucket, encoded)
        };
        (
            format!("{scheme}://{authority}{path}"),
            path,
            authority.to_string(),
        )
    } else {
        let path = if key.is_empty() {
            "/".to_string()
        } else {
            format!("/{encoded}")
        };
        let host = format!("{}.{}", t.bucket, authority);
        (
            format!("{scheme}://{host}{path}"),
            path,
            host, // the signed host: bucket-prefixed, port riding along inside
        )
    }
}

// ── SigV4 ────────────────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::Mac as _;
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    // hmac 0.13: the constructor lives on KeyInit (crypto-common 0.2), while
    // Mac keeps only update/finalize.
    let mut mac =
        <HmacSha256 as hmac::KeyInit>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// The signed GET, assembled for a fixed `amz_date` (YYYYMMDDTHHMMSSZ) so the
/// signature is testable against pinned vectors — production passes the
/// current time. Returns (url, authorization_header).
fn signed_get(t: &BucketTarget, key: &str, amz_date: &str) -> (String, String) {
    let (url, path, host) = object_url(t, key);
    let date_stamp = &amz_date[..8];
    let region = region_for(t);
    let payload_hash = sha256_hex(b"");
    // Sorted: exactly the three headers a GET signs.
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("GET\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(
        format!("AWS4{}", t.secret_access_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        t.access_key_id
    );
    (url, authorization)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The signed PUT. The canonical request carries the payload's own hash and,
/// when a content-type rides the request, that header too — headers sign
/// sorted, which puts content-type FIRST, and the signature is over that
/// ordering.
fn signed_put(
    t: &BucketTarget,
    key: &str,
    payload_hash: &str,
    content_type: &str,
    amz_date: &str,
) -> (String, String) {
    let (url, path, host) = object_url(t, key);
    let date_stamp = &amz_date[..8];
    let region = region_for(t);
    let canonical_headers = format!(
        "content-type:{content_type}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("PUT\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(
        format!("AWS4{}", t.secret_access_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        t.access_key_id
    );
    (url, authorization)
}

/// The signed PUT. Failure is the sentence `storage PUT {status}:
/// {body ≤300}`.
pub async fn s3_put(t: &BucketTarget, key: &str, bytes: &[u8], mime: &str) -> Result<(), String> {
    let content_type = if mime.is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
    let amz_date = amz_now();
    let payload_hash = sha256_hex(bytes);
    let (url, authorization) = signed_put(t, key, &payload_hash, content_type, &amz_date);
    let res = http()
        .put(&url)
        .header("content-type", content_type)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .header("authorization", authorization)
        .body(bytes.to_vec())
        .send()
        .await
        .map_err(|e| format!("storage PUT: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&body);
        return Err(format!(
            "storage PUT {}: {}",
            status.as_u16(),
            crate::body::truncate_utf16(text.trim(), 300)
        ));
    }
    Ok(())
}

/// CreateBucket, tolerant of "already exists". Cached per process per target
/// so internal mode doesn't re-check on every upload — the api never
/// reloads, so a static set is enough.
static ENSURED_BUCKETS: LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

pub async fn ensure_bucket(t: &BucketTarget) -> Result<(), String> {
    let id = format!("{}|{}", t.endpoint, t.bucket);
    if ENSURED_BUCKETS.lock().expect("bucket set").contains(&id) {
        return Ok(());
    }
    // The bucket PUT signs an empty payload and no content-type — the plain
    // GET's three headers with the PUT verb.
    let amz_date = amz_now();
    let (url, path, host) = object_url(t, "");
    let date_stamp = &amz_date[..8];
    let region = region_for(t);
    let payload_hash = sha256_hex(b"");
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("PUT\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(
        format!("AWS4{}", t.secret_access_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        t.access_key_id
    );
    let res = http()
        .put(&url)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .header("authorization", authorization)
        .send()
        .await
        .map_err(|e| format!("storage create-bucket: {e}"))?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&body);
        // The two "fine, it exists" spellings S3 answers with.
        if !(text.contains("BucketAlreadyOwnedByYou") || text.contains("BucketAlreadyExists")) {
            return Err(format!(
                "storage create-bucket {}: {}",
                status.as_u16(),
                crate::body::truncate_utf16(text.trim(), 300)
            ));
        }
    }
    ENSURED_BUCKETS.lock().expect("bucket set").insert(id);
    Ok(())
}

/// The signed GET: 404 is Ok(None); anything else not-ok is `storage GET
/// {status}: {body ≤300}`; ok is the bytes.
pub async fn s3_get(t: &BucketTarget, key: &str) -> Result<Option<Vec<u8>>, String> {
    let amz_date = amz_now();
    let (url, authorization) = signed_get(t, key, &amz_date);
    let res = http()
        .get(&url)
        .header("x-amz-content-sha256", sha256_hex(b""))
        .header("x-amz-date", amz_date)
        .header("authorization", authorization)
        .send()
        .await
        .map_err(|e| format!("storage GET: {e}"))?;
    let status = res.status();
    if status.as_u16() == 404 {
        return Ok(None);
    }
    let body = res.bytes().await.map_err(|e| format!("storage GET: {e}"))?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body);
        return Err(format!(
            "storage GET {}: {}",
            status.as_u16(),
            crate::body::truncate_utf16(text.trim(), 300)
        ));
    }
    Ok(Some(body.to_vec()))
}

/// The signed DELETE. The probe's cleanup step; a failure there is swallowed
/// by the caller, so this surfaces the error for anyone who does care.
pub async fn s3_delete(t: &BucketTarget, key: &str) -> Result<(), String> {
    let amz_date = amz_now();
    // DELETE signs exactly like the GET — same three headers, empty payload.
    let (url, authorization) = signed_get(t, key, &amz_date);
    let res = http()
        .delete(&url)
        .header("x-amz-content-sha256", sha256_hex(b""))
        .header("x-amz-date", amz_date)
        .header("authorization", authorization)
        .send()
        .await
        .map_err(|e| format!("storage DELETE: {e}"))?;
    let status = res.status();
    if !status.is_success() && status.as_u16() != 404 {
        let body = res.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&body);
        return Err(format!(
            "storage DELETE {}: {}",
            status.as_u16(),
            crate::body::truncate_utf16(text.trim(), 300)
        ));
    }
    Ok(())
}

/// The round-trip probe: PUT a tiny object, GET it back, DELETE it. Returns
/// a human-readable failure reason rather than throwing, for the admin
/// panel.
pub async fn test_storage(t: &BucketTarget) -> serde_json::Value {
    if !target_ready(t) {
        return serde_json::json!({
            "ok": false,
            "detail": "endpoint, bucket, access key, and secret are all required",
        });
    }
    let key = format!("{}talaria-storage-probe", t.prefix);
    let payload = b"talaria storage probe";
    let probe = async {
        s3_put(t, &key, payload, "text/plain").await?;
        let back = s3_get(t, &key).await?;
        if back.as_deref() != Some(payload.as_slice()) {
            return Err("wrote the probe object but read back different bytes".to_string());
        }
        Ok(())
    }
    .await;
    match probe {
        Ok(()) => {
            // cleanup failure isn't a config failure
            let _ = s3_delete(t, &key).await;
            serde_json::json!({
                "ok": true,
                "detail": format!(
                    "bucket \"{}\" is reachable and writable (region {})",
                    t.bucket,
                    region_for(t)
                ),
            })
        }
        Err(e) => serde_json::json!({ "ok": false, "detail": e }),
    }
}

/// YYYYMMDDTHHMMSS.mmmZ — ISO-8601 UTC with the punctuation removed. Takes
/// the epoch ms so the civil-from-days math is pinnable.
fn amz_of(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    // Civil-from-days (Howard Hinnant's algorithm) — no chrono dependency for
    // one timestamp format.
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}{month:02}{d:02}T{h:02}{m:02}{s:02}.{millis:03}Z")
}

/// The current time in the signing format.
fn amz_now() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    amz_of(ms)
}

// ── Reads by recorded path ───────────────────────────────────────────────────

static S3_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(s3\+internal|s3)://([^/]+)/(.+)$").unwrap());

/// One blob, wherever the row says it lives:
///   1. `s3+internal://bucket/key` → the bundled MinIO (env-configured);
///      `s3://bucket/key` → the CONFIGURED target with that bucket;
///      anything else → a filesystem path.
///   2. Whatever the primary answered (including nothing), the enabled,
///      fully-configured replica gets one try at `uploads/<basename>`.
///
/// A blob neither place has is None — a transcript block that quietly omits
/// one file, never a failed draft.
pub async fn read_blob(pg: &PgPool, sb: &SecretBox, path: &str) -> Option<Vec<u8>> {
    let cfg = get_storage_config(pg, sb).await;
    let primary = if let Some(m) = S3_PATH.captures(path) {
        let key = m.get(3)?.as_str();
        let target = if m.get(1)?.as_str() == "s3+internal" {
            internal_target()
        } else {
            BucketTarget {
                bucket: m.get(2)?.as_str().to_string(),
                ..cfg.target.clone()
            }
        };
        if target.ready() {
            s3_get(&target, key).await.ok().flatten()
        } else {
            None
        }
    } else {
        tokio::fs::read(path).await.ok()
    };
    if primary.is_some() {
        return primary;
    }
    let replica = if cfg.replica_enabled && cfg.replica.ready() {
        &cfg.replica
    } else {
        return None;
    };
    let key = format!("{}uploads/{}", replica.prefix, blob_basename(path));
    s3_get(replica, &key).await.ok().flatten()
}

/// Canonical `<id><ext>` tail — the same for a disk path and a bucket key, so
/// replica keys are derivable from any row.
fn blob_basename(path: &str) -> &str {
    &path[path.rfind('/').map_or(0, |i| i + 1)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(
        endpoint: &str,
        bucket: &str,
        key: &str,
        secret: &str,
        path_style: bool,
        region: &str,
    ) -> BucketTarget {
        BucketTarget {
            endpoint: endpoint.into(),
            region: region.into(),
            bucket: bucket.into(),
            access_key_id: key.into(),
            secret_access_key: secret.into(),
            path_style,
            prefix: String::new(),
        }
    }

    #[test]
    fn regions_derive_from_the_host() {
        let blank = target(
            "https://s3.us-west-004.backblazeb2.com",
            "b",
            "k",
            "s",
            true,
            "",
        );
        assert_eq!(region_for(&blank), "us-west-004");
        let aws = target(
            "https://s3.eu-central-1.amazonaws.com",
            "b",
            "k",
            "s",
            false,
            "",
        );
        assert_eq!(region_for(&aws), "eu-central-1");
        let r2 = target(
            "https://acct.r2.cloudflarestorage.com",
            "b",
            "k",
            "s",
            true,
            "",
        );
        assert_eq!(region_for(&r2), "auto");
        let ip = target("http://127.0.0.1:9010", "talaria", "k", "s", true, "");
        assert_eq!(region_for(&ip), "us-east-1");
        let said = target("https://example.com", "b", "k", "s", true, "eu-north-1 ");
        assert_eq!(region_for(&said), "eu-north-1");
    }

    #[test]
    fn keys_encode_rfc3986_uppercase_with_slashes_kept() {
        assert_eq!(encode_key("uploads/a b+c!.txt"), "uploads/a%20b%2Bc%21.txt");
        assert_eq!(encode_key("a~b-c_d.e"), "a~b-c_d.e");
        // The sub-delims a lax encoder spares — !'()* — percent-encode
        // uppercase here.
        assert_eq!(encode_key("'*()"), "%27%2A%28%29");
    }

    #[test]
    fn object_urls_carry_the_endpoint_path_and_port() {
        // pathStyle with an endpoint that itself lives under a path.
        let t = target(
            "http://127.0.0.1:9010/minio/",
            "talaria",
            "x y.txt",
            "s",
            true,
            "",
        );
        let (url, path, host) = object_url(&t, "x y.txt");
        assert_eq!(path, "/minio/talaria/x%20y.txt");
        assert_eq!(url, "http://127.0.0.1:9010/minio/talaria/x%20y.txt");
        assert_eq!(host, "127.0.0.1:9010");
        // virtual-host keeps the port riding along inside the signed host.
        let v = target(
            "https://s3.example.com:8443",
            "data",
            "k.txt",
            "s",
            false,
            "",
        );
        let (url, path, host) = object_url(&v, "k.txt");
        assert_eq!(path, "/k.txt");
        assert_eq!(url, "https://data.s3.example.com:8443/k.txt");
        assert_eq!(host, "data.s3.example.com:8443");
        // Empty key: the bucket root (the shape ensureBucket PUTs — kept so
        // the builder is the whole objectUrl, not just the keyed case).
        let (url, path, _) = object_url(&v, "");
        assert_eq!(path, "/");
        assert_eq!(url, "https://data.s3.example.com:8443/");
        assert!(url.ends_with('/'));
    }

    /// Pinned vectors. The first is AWS's own documented SigV4 example
    /// keypair; the second is the house MinIO shape, independently verified
    /// against a from-spec signer that agreed byte-for-byte.
    #[test]
    fn sigv4_signatures_match_the_cross_language_vectors() {
        let aws = target(
            "https://s3.amazonaws.com",
            "examplebucket",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            false,
            "us-east-1",
        );
        let (url, auth) = signed_get(&aws, "test.txt", "20130524T000000Z");
        assert_eq!(url, "https://examplebucket.s3.amazonaws.com/test.txt");
        assert!(
            auth.ends_with(
                "Signature=14f6a0997b2b70a86f4726658a6575b5109092ccb5fd328f51b369c44b4ac958"
            ),
            "aws vector, got: {auth}"
        );
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="
        ));

        // The talaria shape: internal minio, path-style, blank region (derived
        // us-east-1), a key that exercises the encoder.
        let tal = target(
            "http://127.0.0.1:9010",
            "talaria",
            "talaria",
            DEV_S3_SECRET,
            true,
            "",
        );
        let (url, auth) = signed_get(&tal, "uploads/a b+c!.txt", "20260829T000000Z");
        assert_eq!(
            url,
            "http://127.0.0.1:9010/talaria/uploads/a%20b%2Bc%21.txt"
        );
        assert!(
            auth.ends_with(
                "Signature=761e44f3497ba453aa8de575fa310398a41edafb297a3cd63efb78724e8b2d09"
            ),
            "talaria vector, got: {auth}"
        );
    }

    #[test]
    fn amz_dates_match_node_to_isostring_vectors() {
        // ISO-8601 UTC with the punctuation removed — pinned including the
        // epoch and a leap day, which is where a hand-rolled civil calendar
        // goes quietly wrong.
        assert_eq!(amz_of(0), "19700101T000000.000Z");
        assert_eq!(amz_of(1_709_164_800_000), "20240229T000000.000Z");
        assert_eq!(amz_of(1_769_664_000_000), "20260129T052000.000Z");
        assert_eq!(amz_of(1_809_504_000_000), "20270505T080000.000Z");
        let s = amz_now();
        // 19700101T000000.000Z — 20 chars: 8 date, T, 6 time, '.', 3 millis, Z.
        assert_eq!(s.len(), 20);
        assert_eq!(&s[8..9], "T");
        assert_eq!(&s[19..], "Z");
        assert!(
            s[..19]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == 'T')
        );
    }

    #[test]
    fn the_config_merge_fills_defaults_and_keeps_overrides() {
        let empty = merged(&serde_json::json!({}));
        assert_eq!(empty.mode, "local");
        assert!(empty.target.path_style);
        assert!(!empty.replica_enabled);
        assert!(empty.target.endpoint.is_empty());

        let row = serde_json::json!({
            "mode": "s3",
            "endpoint": "https://s3.us-west-004.backblazeb2.com",
            "bucket": "boxie",
            "accessKeyId": "key",
            "secretAccessKey": "cipher",
            "pathStyle": true,
            "prefix": "talaria/",
            "replica": { "enabled": true, "endpoint": "https://acct.r2.cloudflarestorage.com", "bucket": "backup", "accessKeyId": "rk" }
        });
        let cfg = merged(&row);
        assert_eq!(cfg.mode, "s3");
        assert_eq!(cfg.target.bucket, "boxie");
        assert_eq!(cfg.target.prefix, "talaria/");
        assert!(cfg.replica_enabled);
        assert_eq!(cfg.replica.bucket, "backup");
        assert_eq!(cfg.replica.region, ""); // replica fields default empty, not inherited
        assert!(cfg.replica.path_style); // the replica default is path-style too
        // A non-object row is the defaults (get_setting's fallback shape).
        assert_eq!(merged(&serde_json::json!(null)).mode, "local");
    }

    #[test]
    fn unseal_never_panics_and_fails_closed() {
        let sb = crate::secretbox::SecretBox::from_parts(
            crate::secretbox::derive_kek("test-root"),
            std::collections::HashMap::from([(1u32, [7u8; 32])]),
            Some(1),
        );
        assert_eq!(unseal(&sb, ""), "");
        let sealed = sb.seal("wJalrXUtnFEMI/K7MDENG").unwrap();
        assert_eq!(unseal(&sb, &sealed), "wJalrXUtnFEMI/K7MDENG");
        assert_eq!(unseal(&sb, "not-a-token"), "");
    }

    #[test]
    fn replica_keys_derive_from_any_recorded_path() {
        assert_eq!(blob_basename("/var/uploads/abc.txt"), "abc.txt");
        assert_eq!(blob_basename("s3://bucket/prefix/abc.txt"), "abc.txt");
        assert_eq!(blob_basename("no-slash"), "no-slash");
    }

    #[test]
    fn readiness_is_four_truthy_fields() {
        let mut t = target("http://e", "b", "k", "s", true, "");
        assert!(t.ready());
        t.secret_access_key = String::new();
        assert!(!t.ready());
        let mut r = target("http://e", "b", "k", "s", true, "");
        r.endpoint = String::new();
        assert!(!r.ready());
    }
}
