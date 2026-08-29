// Bearer-key authentication — port of ui/src/server/llm-keys.ts's
// authenticateKey, the exact query the TS gateway runs, so a key works against
// whichever runtime serves /api/llm/v1 today. The secret is `tlk_<hex>`, shown
// once at mint; only sha256-hex(lowercase) is stored in llm_api_keys.key_hash.

use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// The per-key ceilings (#265), read on the same hot-path query as TS — the
/// chat relay (phase 2) will enforce them. None = unlimited.
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

/// Strip one leading `Bearer\s+` (case-insensitive) — the route's regex, minus
/// the regex. ASCII whitespace only: JS `\s` also matches exotic unicode
/// spaces, none of which can legally appear in an Authorization header value.
/// Returns the secret candidate, or None when the header isn't Bearer-shaped
/// (`Basic …`, missing, or `Bearer` with no whitespace — JS `\s+` requires at
/// least one, and a header like `Bearertlk_…` must NOT authenticate).
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

/// Resolve a `tlk_` secret to its owner. Ok(None) = unknown/revoked/malformed.
/// The caller owns the detached last_used_at update (a fire-and-forget write,
/// never worth failing a request over — same `.catch(() => {})` as TS).
pub async fn authenticate_key(
    pg: &PgPool,
    secret: &str,
) -> Result<Option<KeyIdentity>, sqlx::Error> {
    if !secret.starts_with("tlk_") {
        return Ok(None); // not even a lookup — TS's startsWith gate
    }
    // ::text for the uuid columns (KeyIdentity speaks String), ::float8 for
    // parity with llm-keys.ts: bigint/numeric arrive as strings in the TS
    // driver, so it casts; f64 is the same read in Rust.
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
    .bind(sha256_hex(secret))
    .fetch_optional(pg)
    .await?;
    Ok(row.map(
        |(key_id, key_name, user_id, email, tokens, usd, rpm)| KeyIdentity {
            key_id,
            key_name,
            user_id,
            email,
            caps: KeyCaps { tokens, usd, rpm },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_the_ts_digest() {
        // `printf '%s' 'tlk_abc' | sha256sum` — the value TS's digest('hex')
        // stores in key_hash, lowercase hex.
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
        assert_eq!(b(Some("bearer tlk_x")), Some("tlk_x")); // /i
        assert_eq!(b(Some("BEARER   tlk_x")), Some("tlk_x")); // \s+ eats all
        assert_eq!(b(Some("Bearer tlk_a extra")), Some("tlk_a extra")); // one replace, at the front only
        assert_eq!(b(None), None);
        assert_eq!(b(Some("Basic dXNlcg==")), None);
        assert_eq!(b(Some("Bearer")), None); // \s+ needs one
        assert_eq!(b(Some("Bearertlk_x")), None); // no whitespace, no match — must NOT authenticate
        assert_eq!(b(Some("x Bearer tlk_x")), None); // ^ anchored
    }
}
