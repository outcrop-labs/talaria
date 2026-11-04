// Symmetric envelope encryption for sealed secrets. The token grammar is a
// CROSS-LANGUAGE contract: the TS implementation of the same recipe seals and
// opens these rows, so every byte of the format is load-bearing. The shape:
//
//   KEK  scrypt(root secret, "talaria.secretbox.v1", 32) — node's default
//        parameters, N=16384 (log2 14), r=8, p=1. The only key material
//        outside the database. MUST stay stable or every wrapped DEK dies.
//   DEK  random 256-bit key per version, stored in `secret_keys` WRAPPED by
//        the KEK. Every version kept forever, so old ciphertext always opens.
//
//   v1:<iv>:<tag>:<data>         KEK-direct (legacy; also how a DEK is wrapped)
//   v2:<iv>:<tag>:<data>         DEK, unversioned (legacy — decrypts with the
//                                ACTIVE key, because it predates versioning)
//   v2:<ver>:<iv>:<tag>:<data>   DEK version <ver>  ← what seal() writes
//
// All parts are base64url WITHOUT padding. The AES-GCM IV is 12 bytes and the
// tag 16. One asymmetry that will silently corrupt everything if gotten
// wrong: the DEK wrap's plaintext is the STANDARD padded base64 STRING of the
// key bytes, not the raw bytes — a quirk of the first writer that is now part
// of the format. It has its own fixture case in tests/it/fixtures.
//
// This box OPENS, SEALS, and ROTATES, but never CREATES the first key: an
// empty secret_keys is a recorded failure, not a migration write this crate
// performs. The failure is recorded rather than thrown so nothing that
// doesn't need a key ever learns about it — and it records whether it can
// heal (`failure_is_transient`): a table not created yet, or not given its
// first key yet, is the migration pass still running; keys the root cannot
// unwrap are a different root, and no retry fixes that.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use scrypt::Params;
use sqlx::PgPool;
use std::collections::HashMap;

const SALT: &[u8] = b"talaria.secretbox.v1";
/// Public because the cross-language fixtures (tests/secretbox.rs) build IVs
/// from hex literals of exactly this length.
pub const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEK_LEN: usize = 32;

type Key = [u8; KEK_LEN];

#[derive(Debug, thiserror::Error)]
pub enum SecretboxError {
    #[error("secretbox: unrecognized token")]
    UnrecognizedToken,
    #[error(
        "secretbox: data key v{0} not loaded (rotated by another process? restart to pick it up)"
    )]
    VersionNotLoaded(u32),
    #[error("secretbox: {0}")]
    Unusable(String),
    #[error("secretbox: crypto failure ({0})")]
    Crypto(String),
}

/// kek Material derivation, published so rotation tooling (later phase) can
/// re-wrap without a second implementation. `root_source()` in config.rs names
/// where the material came from; it never leaves the process.
pub fn derive_kek(root_material: &str) -> Key {
    // Node's scryptSync defaults, spelled out: N=16384 (log2 14), r=8, p=1 —
    // the TS side derives with the same numbers or the KEK is not the same
    // key. scrypt's Params fixes the output at 32 bytes — exactly KEK_LEN.
    let params = Params::new(14, 8, 1).expect("params are node's defaults and always valid");
    let mut out = [0u8; KEK_LEN];
    scrypt::scrypt(root_material.as_bytes(), SALT, &params, &mut out)
        .expect("output buffer matches key_len");
    out
}

#[derive(Clone, Debug, Default)]
pub struct SecretBox {
    kek: Option<Key>,
    deks: HashMap<u32, Key>,
    active: Option<u32>,
    failure: Option<String>,
    /// Whether `failure` can heal without anyone touching the root secret:
    /// the table was not readable yet, or held no key yet. See
    /// [`SecretBox::failure_is_transient`].
    failure_transient: bool,
}

/// One `secret_keys` row as `load` reads it: (version, wrapped_dek, active).
type KeyRow = (i32, String, bool);

/// Low-level AES-256-GCM with an explicit IV (production callers generate a
/// random one; tests inject a fixed one to make fixtures deterministic).
fn enc_raw(
    key: &Key,
    iv: &[u8; IV_LEN],
    plaintext: &str,
) -> Result<(String, String, String), SecretboxError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("KEK/DEK are 32 bytes");
    let sealed = cipher
        .encrypt(&Nonce::from(*iv), plaintext.as_bytes())
        .map_err(|e| SecretboxError::Crypto(e.to_string()))?;
    let (data, tag) = sealed.split_at(sealed.len() - TAG_LEN); // RustCrypto appends the tag
    Ok((
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(data),
    ))
}

fn dec_raw(key: &Key, iv: &str, tag: &str, data: &str) -> Result<String, SecretboxError> {
    let iv: [u8; IV_LEN] = URL_SAFE_NO_PAD
        .decode(iv)
        .map_err(bad_part)?
        .try_into()
        .map_err(|_| SecretboxError::Crypto(format!("IV is not {IV_LEN} bytes")))?;
    let tag = URL_SAFE_NO_PAD.decode(tag).map_err(bad_part)?;
    let data = URL_SAFE_NO_PAD.decode(data).map_err(bad_part)?;
    let mut sealed = data;
    sealed.extend_from_slice(&tag);
    let cipher = Aes256Gcm::new_from_slice(key).expect("KEK/DEK are 32 bytes");
    let plain = cipher
        .decrypt(&Nonce::from(iv), sealed.as_ref())
        .map_err(|_| {
            SecretboxError::Crypto("authentication failed — wrong key or tampered token".into())
        })?;
    String::from_utf8(plain).map_err(|_| SecretboxError::Crypto("plaintext is not UTF-8".into()))
}

fn bad_part(e: base64::DecodeError) -> SecretboxError {
    SecretboxError::Crypto(format!("malformed base64url part: {e}"))
}

/// Wrap a DEK under a KEK. The plaintext is the standard padded base64 string
/// of the key bytes — the asymmetry above; unwrap is its mirror.
fn wrap_dek(kek: &Key, dek: &Key) -> Result<String, SecretboxError> {
    let (iv, tag, data) = enc_raw(kek, &random_iv()?, &STANDARD.encode(dek))?;
    Ok(["v1", &iv, &tag, &data].join(":"))
}

fn unwrap_dek(kek: &Key, wrapped: &str) -> Result<Key, SecretboxError> {
    let plain = dec_raw(kek, part(wrapped, 1)?, part(wrapped, 2)?, part(wrapped, 3)?)?;
    let bytes = STANDARD
        .decode(plain)
        .map_err(|e| SecretboxError::Crypto(format!("wrapped DEK is not base64: {e}")))?;
    bytes
        .try_into()
        .map_err(|_: Vec<u8>| SecretboxError::Crypto("wrapped DEK is not 32 bytes".into()))
}

fn part(token: &str, i: usize) -> Result<&str, SecretboxError> {
    token
        .split(':')
        .nth(i)
        .filter(|s| !s.is_empty())
        .ok_or(SecretboxError::UnrecognizedToken)
}

fn random_iv() -> Result<[u8; IV_LEN], SecretboxError> {
    let mut iv = [0u8; IV_LEN];
    getrandom::fill(&mut iv)
        .map_err(|e| SecretboxError::Crypto(format!("no entropy for IV: {e}")))?;
    Ok(iv)
}

impl SecretBox {
    /// Load every DEK version from `secret_keys`, unwrapping each with the
    /// KEK derived from `root_material`. `load` never writes — not even to
    /// create a first key on an empty table.
    pub async fn load(pg: &PgPool, root_material: &str) -> SecretBox {
        Self::load_with_kek(pg, derive_kek(root_material)).await
    }

    /// Read `secret_keys` again under this box's own KEK — the retry for a
    /// box whose failure [is transient](SecretBox::failure_is_transient).
    /// Same rules as `load`, minus the scrypt derivation: the root has not
    /// changed, only the table has.
    pub async fn reload(&self, pg: &PgPool) -> SecretBox {
        match self.kek {
            Some(kek) => Self::load_with_kek(pg, kek).await,
            None => self.clone(),
        }
    }

    async fn load_with_kek(pg: &PgPool, kek: Key) -> SecretBox {
        let rows = sqlx::query_as::<_, KeyRow>(
            "select version, wrapped_dek, active from secret_keys order by version asc",
        )
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string());
        Self::from_rows(kek, rows)
    }

    /// The box `load` builds from what the `secret_keys` read returned —
    /// pure, so every failure class is a unit test with no database.
    fn from_rows(kek: Key, rows: Result<Vec<KeyRow>, String>) -> SecretBox {
        let mut sb = SecretBox {
            kek: Some(kek),
            ..Default::default()
        };

        let rows = match rows {
            Ok(rows) => rows,
            Err(e) => {
                // A read failure is a diagnosis, not a crash: nothing that
                // doesn't need a key should learn about it. TRANSIENT: the
                // usual cause is a process that asked before the app's
                // migration pass created the table (`talaria dev` starts the
                // api first; a fresh or reset database), or a Postgres still
                // coming up — both heal on their own.
                sb.failure = Some(format!("secret_keys unreadable: {e}"));
                sb.failure_transient = true;
                return sb;
            }
        };

        if rows.is_empty() {
            // The first data key is the app's migration write (initSecretbox in
            // ui/src/server/db/pg.ts) and only happens under an explicit
            // TALARIA_SECRET_KEY — so an empty table after the app has run means
            // it refused, and its boot log says why. Name that, not a rerun.
            sb.failure = Some(
                "secret_keys is empty — the app's migration pass creates the first data key, \
                 and only under an explicit TALARIA_SECRET_KEY. If the app has already run \
                 against this database, its boot log names why it refused. (This service never \
                 creates key material — see docs/ENCRYPTION.md.)"
                    .into(),
            );
            // TRANSIENT too: the migration pass writes the first key moments
            // after it creates the table, and a box loaded in between would
            // otherwise refuse every seal until the process restarted.
            sb.failure_transient = true;
            return sb;
        }

        let row_count = rows.len(); // used in the all-failed diagnosis below
        for (version, wrapped, active) in &rows {
            match unwrap_dek(&kek, wrapped) {
                Ok(dek) => {
                    if *active {
                        sb.active = Some(*version as u32); // last active row wins
                    }
                    sb.deks.insert(*version as u32, dek);
                }
                Err(_) => {
                    // Loud, but don't brick the process: a version we can't
                    // unwrap means the root secret changed. Its ciphertext
                    // will fail to open; secrets under loadable versions keep
                    // working.
                    tracing::error!(
                        "[secretbox] cannot unwrap data key v{version} — TALARIA_SECRET_KEY/AUTH_SECRET \
                         differs from when it was created. Restore the original root secret to \
                         recover secrets sealed with v{version}."
                    );
                }
            }
        }

        if sb.active.is_none() && sb.deks.is_empty() {
            // Refusal to mislead: name the real cause (the root secret), not
            // the code path that noticed. Every row failed to unwrap — deks
            // is empty — so rows.len() IS the count.
            sb.failure = Some(format!(
                "this database has {row_count} data key(s) and none can be unwrapped with the current \
                     root secret. Restore it — every provider key, agent secret and OAuth token \
                     in this database is sealed with it. (See docs/ENCRYPTION.md.)"
            ));
            return sb;
        }
        if sb.active.is_none() {
            sb.active = sb.deks.keys().max().copied();
        }
        sb
    }

    /// A box for tests and tooling: keys in hand, nothing loaded from disk.
    pub fn from_parts(kek: Key, deks: HashMap<u32, Key>, active: Option<u32>) -> SecretBox {
        SecretBox {
            kek: Some(kek),
            deks,
            active,
            failure: None,
            failure_transient: false,
        }
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// A failure that a later read can clear: `secret_keys` was unreadable
    /// (not created yet, or Postgres not answering) or empty (the first key
    /// not written yet). The process that holds the box retries those
    /// (AppState::secretbox, throttled). A key set that will not unwrap is
    /// NOT transient — the root secret differs from the one that sealed this
    /// database, and that stays loud until a person fixes it.
    pub fn failure_is_transient(&self) -> bool {
        self.failure.is_some() && self.failure_transient
    }

    fn dek(&self, version: u32) -> Result<&Key, SecretboxError> {
        self.deks
            .get(&version)
            .ok_or(SecretboxError::VersionNotLoaded(version))
    }

    /// The recorded diagnosis wins over everything: it names the root
    /// secret, which is the real cause.
    fn active(&self) -> Result<(&Key, u32), SecretboxError> {
        if let Some(why) = &self.failure {
            return Err(SecretboxError::Unusable(why.clone()));
        }
        let v = self.active.ok_or(SecretboxError::Unusable(
            "not initialized (no data keys loaded)".into(),
        ))?;
        Ok((self.dek(v)?, v))
    }

    /// Encrypt with the active DEK; the token records its version.
    pub fn seal(&self, plaintext: &str) -> Result<String, SecretboxError> {
        let (key, version) = self.active()?;
        let iv = random_iv()?;
        let (iv, tag, data) = enc_raw(key, &iv, plaintext)?;
        Ok(["v2", &version.to_string(), &iv, &tag, &data].join(":"))
    }

    /// Decrypt any token in the grammar, whichever side produced it.
    pub fn open(&self, token: &str) -> Result<String, SecretboxError> {
        let p: Vec<&str> = token.split(':').collect();
        match (p.first().copied(), p.len()) {
            // v1 never consults the DEK registry — a failed DEK load must not
            // break KEK-direct tokens.
            (Some("v1"), 4) => dec_raw(
                &self
                    .kek
                    .ok_or_else(|| SecretboxError::Unusable("no KEK".into()))?,
                p[1],
                p[2],
                p[3],
            ),
            (Some("v2"), 5) => dec_raw(
                self.dek(
                    p[1].parse()
                        .map_err(|_| SecretboxError::UnrecognizedToken)?,
                )?,
                p[2],
                p[3],
                p[4],
            ),
            (Some("v2"), 4) => {
                let (key, _) = self.active()?;
                dec_raw(key, p[1], p[2], p[3])
            }
            _ => Err(SecretboxError::UnrecognizedToken),
        }
    }

    /// Deterministic seal for tests: same grammar as seal(), caller-chosen IV.
    pub fn seal_with_iv(
        &self,
        version: u32,
        iv: &[u8; IV_LEN],
        plaintext: &str,
    ) -> Result<String, SecretboxError> {
        let key = self.dek(version)?;
        let (iv, tag, data) = enc_raw(key, iv, plaintext)?;
        Ok(["v2", &version.to_string(), &iv, &tag, &data].join(":"))
    }

    /// Deterministic DEK wrap for tests (mirrors wrap_dek with a fixed IV).
    pub fn wrap_dek_with_iv(
        kek: &Key,
        dek: &Key,
        iv: &[u8; IV_LEN],
    ) -> Result<String, SecretboxError> {
        let (iv, tag, data) = enc_raw(kek, iv, &STANDARD.encode(dek))?;
        Ok(["v1", &iv, &tag, &data].join(":"))
    }

    pub fn kek(&self) -> Option<&Key> {
        self.kek.as_ref()
    }

    // ── Status surface (secret-health's reads) ──────────────────────────────
    // All of these answer rather than throw: an inventory of every secret on a
    // broken instance is the one read that MUST work while it is broken.

    /// A v2:<ver> token names its DEK, so the answer is a map lookup; v1 and
    /// legacy unversioned v2 have to actually try.
    pub fn token_readable(&self, token: &str) -> bool {
        let parts: Vec<&str> = token.split(':').collect();
        if parts.first() == Some(&"v2") && parts.len() == 5 {
            return parts[1]
                .parse::<u32>()
                .map(|v| self.deks.contains_key(&v))
                .unwrap_or(false);
        }
        self.open(token).is_ok()
    }

    /// The active DEK version, or None when there isn't one.
    pub fn active_key_version(&self) -> Option<u32> {
        self.active
    }

    /// Every DEK version in memory, ascending (load order is version order).
    pub fn loaded_versions(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.deks.keys().copied().collect();
        v.sort_unstable();
        v
    }

    // ── Rotation surface (secret-rotation's writes) ─────────────────────────

    /// The version a rotation's successor gets. Rotation is a write, and a
    /// write on an unusable key set SHOULD fail loudly.
    pub fn current_key_version(&self) -> Result<u32, SecretboxError> {
        Ok(self.active()?.1)
    }

    /// Re-wrap an in-memory DEK under a KEK derived from new root material
    /// (the DB row keeps the old wrap; the same transaction overwrites it
    /// with this).
    pub fn rewrap_version(
        &self,
        version: u32,
        root_material: &str,
    ) -> Result<String, SecretboxError> {
        let dek = self.dek(version)?;
        wrap_dek(&derive_kek(root_material), dek)
    }

    /// Encrypt with an explicit DEK + version (re-encryption under the new
    /// key; the box's own active version is not consulted).
    pub fn seal_with(
        &self,
        dek: &Key,
        version: u32,
        plaintext: &str,
    ) -> Result<String, SecretboxError> {
        let (iv, tag, data) = enc_raw(dek, &random_iv()?, plaintext)?;
        Ok(["v2", &version.to_string(), &iv, &tag, &data].join(":"))
    }

    /// Wrap a DEK under the current KEK, or one derived from new root
    /// material when the rotation also moves the root.
    pub fn wrap_dek_for(
        &self,
        dek: &Key,
        root_material: Option<&str>,
    ) -> Result<String, SecretboxError> {
        match root_material {
            Some(m) => Ok(wrap_dek(&derive_kek(m), dek)?),
            None => Ok(wrap_dek(
                self.kek
                    .as_ref()
                    .ok_or_else(|| SecretboxError::Unusable("no KEK".into()))?,
                dek,
            )?),
        }
    }

    /// An installed key as a NEW box — the process's box is swapped
    /// atomically, never mutated. Keeps every prior version; moves the KEK
    /// when the root changed.
    pub fn installed(&self, dek: Key, version: u32, root_material: Option<&str>) -> SecretBox {
        let mut next = self.clone();
        if let Some(m) = root_material {
            next.kek = Some(derive_kek(m));
        }
        next.deks.insert(version, dek);
        next.active = Some(version);
        next
    }
}

/// A fresh random 256-bit data key.
pub fn new_dek() -> Result<Key, SecretboxError> {
    let mut dek = [0u8; KEK_LEN];
    getrandom::fill(&mut dek)
        .map_err(|e| SecretboxError::Crypto(format!("no entropy for a new data key: {e}")))?;
    Ok(dek)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cross-language fixtures live in tests/it/fixtures/secretbox.json (see
    // tests/secretbox.rs). These in-crate tests pin the round trips.

    fn kek_for(root: &str) -> Key {
        derive_kek(root)
    }

    #[test]
    fn kek_matches_the_recipe() {
        // 32 bytes, deterministic, salted by the literal — the exact value is
        // pinned in tests/it/fixtures/secretbox.json.
        assert_eq!(kek_for("root").len(), 32);
        assert_ne!(kek_for("root"), kek_for("other"));
    }

    #[test]
    fn seal_open_round_trip_all_token_grammars() {
        let kek = kek_for("root");
        let mut deks = HashMap::new();
        deks.insert(1u32, [7u8; 32]);
        deks.insert(2u32, [9u8; 32]);
        let sb = SecretBox::from_parts(kek, deks, Some(2));

        // v2 versioned — what seal() writes
        let token = sb.seal("sk-provider-key").unwrap();
        assert!(token.starts_with("v2:2:"), "{token}");
        assert_eq!(sb.open(&token).unwrap(), "sk-provider-key");

        // v1 KEK-direct
        let iv = [1u8; IV_LEN];
        let (ivs, tag, data) = enc_raw(&kek, &iv, "legacy").unwrap();
        let v1 = ["v1", &ivs, &tag, &data].join(":");
        assert_eq!(sb.open(&v1).unwrap(), "legacy");

        // legacy unversioned v2 → ACTIVE key
        let iv2 = [2u8; IV_LEN];
        let (ivs2, tag2, data2) = enc_raw(&[9u8; 32], &iv2, "old").unwrap();
        let v2legacy = ["v2", &ivs2, &tag2, &data2].join(":");
        assert_eq!(sb.open(&v2legacy).unwrap(), "old");

        // tampered ciphertext must fail the tag check
        let mut tampered = token.clone();
        let pos = tampered.len() - 2;
        let flipped = if tampered.as_bytes()[pos] == b'A' {
            'B'
        } else {
            'A'
        };
        tampered.replace_range(pos..pos + 1, &flipped.to_string());
        assert_ne!(tampered, token);
        assert!(sb.open(&tampered).is_err());
    }

    #[test]
    fn dek_wrap_uses_the_base64_string_as_plaintext() {
        // The asymmetry: wrap(kek, dek) is enc(kek, base64_std(dek)), so
        // opening the wrap yields the base64 STRING, not the raw key.
        let kek = kek_for("root");
        let dek = [3u8; 32];
        let wrapped = SecretBox::wrap_dek_with_iv(&kek, &dek, &[5u8; IV_LEN]).unwrap();
        // unwrap returns the raw key…
        assert_eq!(unwrap_dek(&kek, &wrapped).unwrap(), dek);
        // …but only via the standard-base64 string — the wrap's plaintext is
        // the string itself.
        let plain = dec_raw(
            &kek,
            part(&wrapped, 1).unwrap(),
            part(&wrapped, 2).unwrap(),
            part(&wrapped, 3).unwrap(),
        )
        .unwrap();
        assert_eq!(plain, STANDARD.encode(dek));
    }

    #[test]
    fn failure_state_blocks_only_what_needs_a_key() {
        let sb = SecretBox {
            kek: Some(kek_for("r")),
            deks: HashMap::new(),
            active: None,
            failure: Some("root changed".into()),
            failure_transient: false,
        };
        assert!(sb.seal("x").is_err());
        // legacy-unversioned v2 needs the ACTIVE key → blocked by the diagnosis
        assert!(sb.open("v2:AAAA:AAAA:AAAA").is_err());
        // v1 still works — it needs only the KEK.
        let (ivs, tag, data) = enc_raw(&kek_for("r"), &[0u8; IV_LEN], "still-readable").unwrap();
        let v1 = ["v1", &ivs, &tag, &data].join(":");
        assert_eq!(sb.open(&v1).unwrap(), "still-readable");
    }

    // ── Which failures heal (follow-up A6) ──────────────────────────────────
    // The api loads the box once and keeps it; `talaria dev` starts the api
    // BEFORE the app's migration pass creates `secret_keys` and its first key.
    // A box loaded in that window used to refuse every seal — the first hire's
    // agent key included — until someone restarted the api. These pin which
    // failures the holder may retry and which must stay loud.

    fn wrapped_row(kek: &Key, version: i32, dek: &Key, active: bool) -> KeyRow {
        (
            version,
            SecretBox::wrap_dek_with_iv(kek, dek, &[version as u8; IV_LEN]).unwrap(),
            active,
        )
    }

    #[test]
    fn an_unreadable_table_is_a_transient_failure() {
        let sb = SecretBox::from_rows(
            kek_for("root"),
            Err("relation \"secret_keys\" does not exist".into()),
        );
        assert!(
            sb.failure()
                .unwrap()
                .starts_with("secret_keys unreadable: ")
        );
        assert!(sb.failure_is_transient());
        assert!(sb.seal("x").is_err());
    }

    #[test]
    fn an_empty_table_is_a_transient_failure() {
        let sb = SecretBox::from_rows(kek_for("root"), Ok(vec![]));
        assert!(sb.failure().unwrap().starts_with("secret_keys is empty"));
        assert!(sb.failure_is_transient());
    }

    #[test]
    fn keys_the_root_cannot_unwrap_are_not_transient() {
        // Sealed under another root: the one failure no retry can fix.
        let other = kek_for("the-root-that-sealed-this-db");
        let sb = SecretBox::from_rows(
            kek_for("root"),
            Ok(vec![wrapped_row(&other, 1, &[4u8; 32], true)]),
        );
        assert!(sb.failure().unwrap().contains("none can be unwrapped"));
        assert!(!sb.failure_is_transient());
    }

    #[test]
    fn a_healthy_box_has_no_failure_to_retry() {
        let kek = kek_for("root");
        let sb = SecretBox::from_rows(kek, Ok(vec![wrapped_row(&kek, 1, &[4u8; 32], true)]));
        assert!(sb.failure().is_none());
        assert!(!sb.failure_is_transient());
        let token = sb.seal("agent-key").unwrap();
        assert_eq!(sb.open(&token).unwrap(), "agent-key");
        assert!(!SecretBox::from_parts(kek, HashMap::new(), None).failure_is_transient());
    }
}
