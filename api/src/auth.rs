// Bearer-key authentication for the LLM relay. The secret is `tlk_<hex>`,
// shown once at mint; only sha256-hex(lowercase) is stored in
// llm_api_keys.key_hash.

use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// The per-key ceilings (#265), read on the same hot-path query — the chat
/// relay enforces them. None = unlimited.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyCaps {
    pub tokens: Option<f64>,
    pub usd: Option<f64>,
    pub rpm: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct KeyIdentity {
    pub key_id: String,
    pub key_name: String,
    pub user_id: String,
    pub email: Option<String>,
    pub caps: KeyCaps,
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Strip one leading `Bearer` (case-insensitive) plus at least one ASCII
/// whitespace char — exotic unicode spaces don't count, and none can legally
/// appear in an Authorization header value anyway.
/// Returns the secret candidate, or None when the header isn't Bearer-shaped
/// (`Basic …`, missing, or `Bearer` with no whitespace — at least one space
/// is required, and a header like `Bearertlk_…` must NOT authenticate).
pub fn bearer_secret(header: Option<&str>) -> Option<&str> {
    let h = header?;
    // Byte-wise prefix check — `h[..6]` on a non-ASCII header could split a
    // multibyte char and panic.
    if !h
        .as_bytes()
        .get(0..6)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"bearer"))
    {
        return None;
    }
    let rest = &h[6..]; // first 6 bytes are ASCII, this cannot split a char
    let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r', '\u{0b}', '\u{0c}']);
    (trimmed.len() != rest.len()).then_some(trimmed)
}

// ── The identity cache ───────────────────────────────────────────────────────
//
// Every authenticated relay call — every agent tool turn — resolved its key
// with a pool checkout. The identity changes only by mint/revoke/caps-edit,
// all admin-rare, so a 15s serve window keyed by the key hash takes the
// checkout off the hot path. LAW: every in-process writer of the key rows
// (revoke, policy, fleet rotation) resets the cache, so those land now; the
// TTL is the backstop for anything else. Ok(None) is cached too (unknown
// keys stop costing a lookup); Err NEVER is — a database outage must not be
// remembered as "unknown key".

const IDENTITY_TTL: std::time::Duration = std::time::Duration::from_secs(15);
/// Past this many distinct keys in the window, the map clears rather than
/// grows — random-key hammering must not buy unbounded memory.
const IDENTITY_CAP: usize = 4096;

fn identity_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Option<KeyIdentity>)>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Option<KeyIdentity>)>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Drop every cached identity — revocation and caps edits take effect on the
/// next call instead of the next TTL expiry. Tests call it to pin the
/// tradeoff from both sides.
pub fn reset_identity_cache() {
    identity_cache().lock().expect("identity cache").clear();
}

/// Resolve a `tlk_` secret to its owner. Ok(None) = unknown/revoked/malformed.
/// The caller owns the detached last_used_at update (a fire-and-forget write,
/// never worth failing a request over).
pub async fn authenticate_key(
    pg: &PgPool,
    secret: &str,
) -> Result<Option<KeyIdentity>, sqlx::Error> {
    if !secret.starts_with("tlk_") {
        return Ok(None); // not even a lookup — the prefix gate
    }
    let hash = sha256_hex(secret);
    {
        let c = identity_cache().lock().expect("identity cache");
        if let Some((at, hit)) = c.get(&hash)
            && at.elapsed() < IDENTITY_TTL
        {
            return Ok(hit.clone());
        }
    }
    // ::text for the uuid columns (KeyIdentity speaks String); ::float8 so
    // bigint/numeric caps read as f64 rather than strings.
    type KeyRow = (
        String,
        String,
        String,
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    );
    let row: Option<KeyRow> = sqlx::query_as(
        "select k.id::text, k.name, k.user_id::text, u.email, \
             k.spend_cap_tokens::float8, k.spend_cap_usd::float8, k.rate_limit_per_minute::float8 \
             from llm_api_keys k join users u on u.id = k.user_id \
             where k.key_hash = $1 and k.revoked_at is null",
    )
    .bind(&hash)
    .fetch_optional(pg)
    .await?;
    let identity = row.map(
        |(key_id, key_name, user_id, email, tokens, usd, rpm)| KeyIdentity {
            key_id,
            key_name,
            user_id,
            email,
            caps: KeyCaps { tokens, usd, rpm },
        },
    );
    let mut c = identity_cache().lock().expect("identity cache");
    if c.len() >= IDENTITY_CAP {
        c.clear();
    }
    c.insert(hash, (std::time::Instant::now(), identity.clone()));
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_the_known_vectors() {
        // `printf '%s' 'tlk_abc' | sha256sum` — lowercase hex, exactly as
        // key_hash stores it.
        assert_eq!(
            sha256_hex("tlk_abc"),
            "263d00eaf6bf7f006b141bfdd38d30d3aebb3e5d65badf8968daee880830a5f7"
        );
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(
            sha256_hex("")
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn bearer_parsing_matches_the_route_regex() {
        use bearer_secret as b;
        assert_eq!(b(Some("Bearer tlk_x")), Some("tlk_x"));
        assert_eq!(b(Some("bearer tlk_x")), Some("tlk_x")); // case-insensitive
        assert_eq!(b(Some("BEARER   tlk_x")), Some("tlk_x")); // all leading whitespace eaten
        assert_eq!(b(Some("Bearer tlk_a extra")), Some("tlk_a extra")); // one strip, at the front only
        assert_eq!(b(None), None);
        assert_eq!(b(Some("Basic dXNlcg==")), None);
        assert_eq!(b(Some("Bearer")), None); // needs one space
        assert_eq!(b(Some("Bearertlk_x")), None); // no whitespace, no match — must NOT authenticate
        assert_eq!(b(Some("x Bearer tlk_x")), None); // ^ anchored
    }
}
