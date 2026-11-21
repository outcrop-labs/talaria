// The catalog MIRROR — Hermes' official skills index, copied into Postgres
// and searched there.
//
// WHAT THE INDEX IS. `skills-index.json` on the Hermes docs site (the URL
// below is Hermes' own `HERMES_INDEX_URL`, tools/skills_hub_search.py),
// rebuilt daily by Hermes' CI: one row per skill across every registry
// Hermes' Skills Hub reads, each with its source, its trust level and — for
// the GitHub-backed ones — the repo and path its files live at. ~100k rows,
// ~43 MB of JSON (~11 MB gzipped).
//
// WHAT THE MIRROR KEEPS. Only what Talaria can install: rows whose files the
// index resolves to a GitHub repo and path — Hermes' official optional
// skills, its curated GitHub taps, and skills.sh (~20k rows). ClawHub,
// LobeHub and browse.sh rows live on registries with their own fetch
// protocols and are left out rather than listed uninstallable. The
// trust level is the index's own; the scan recomputes it from the
// identifier the way Hermes does (see `guard`), so a row is kept only when
// that identifier names the repo its files come from (`provenance_ok`).
//
// HOW IT FILLS. The `skills-index-refresh` job (leased, every 6 hours — the
// TTL Hermes itself caches the index for) fetches the index with the last
// ETag; a 304 is a no-op, anything else is parsed as a stream (the installable
// rows are kept as they go by, the rest is never held). The parsed rows'
// digest is compared with the last stored one: the docs site's ETag moves on
// every deploy whether or not a row did, and an index whose rows are the
// mirror's already is not written at all. Otherwise one transaction updates
// the rows that differ, inserts the new ones and deletes the ones the index
// no longer carries — an unchanged row is never rewritten. A search that
// finds the mirror empty nudges a sync itself, at most once per five
// minutes, so a fresh install's first open still answers.
//
// WHY NO STORED POSITION DRIVES THE ORDER. Hermes builds the index sorted by
// source (official, skills.sh, github) and then name
// (scripts/build_skills_index.py), so the listing sorts on those two
// columns itself. A stored position would shift for every row after a skill
// added or removed mid-list, which every crawl does: comparing it made each
// fresh crawl rewrite ~85% of the mirror to move ~40 rows. `rank` keeps the
// row's position on the day it arrived, for a release before this one,
// which still sorts by it.

use std::fmt;
use std::io::Read;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use talaria_safe_fetch::{SafeFetch, safe_fetch};
use talaria_scheduler::{JobName, JobSpec};

use crate::valid_repo;

/// Hermes' own `HERMES_INDEX_URL`. It 301s to the GitHub Pages copy;
/// safe_fetch validates every hop.
pub const INDEX_URL: &str = "https://hermes-agent.nousresearch.com/docs/api/skills-index.json";

const SYNC_KEY: &str = "skills_index_sync";
pub const REFRESH_EVERY_MS: u64 = 6 * 60 * 60_000;
const REFRESH_FIRST_RUN_DELAY_MS: u64 = 30_000;
/// One download (~11 MB gzipped) and one transaction of ~20k upserts.
const REFRESH_MAX_RUN_MS: u64 = 5 * 60_000;
const FETCH_TIMEOUT_MS: u64 = 120_000;
/// The wire cap: comfortably over today's identity size too, for a proxy
/// that ignores Accept-Encoding.
const FETCH_MAX_BYTES: u64 = 96 * 1024 * 1024;
/// The decoded cap — a gzip bomb stops here, not in memory.
const DECODED_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Fewer installable rows than this is a broken index, not a smaller one:
/// the sync refuses it rather than deleting the mirror down to it.
const MIN_INSTALLABLE: usize = 100;
const UPSERT_BATCH: usize = 1000;
/// A search on an empty mirror re-attempts a sync at most this often.
const NUDGE_EVERY_MS: i64 = 5 * 60_000;
/// The picker's page.
pub const PAGE: i64 = 60;

/// One installable catalog row — the picker's wire shape. `id` is Hermes'
/// identifier and the install's only input: the route resolves everything
/// else from the mirror, so an install can only ever name a listed skill.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// official | github | skills.sh
    pub source: String,
    /// builtin | trusted | community, as the index states it.
    pub trust_level: String,
    pub repo: String,
    /// The skill directory inside `repo` ("" = the repo root). When `exact`
    /// is false this is a skills.sh slug the index could not resolve, and
    /// the fetch looks for the one directory of that name.
    pub path: String,
    #[serde(skip)]
    pub exact: bool,
    pub tags: Vec<String>,
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// A field read leniently: a string is itself, anything else is "" — one
/// odd row must not fail a 100k-row parse.
fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        _ => String::new(),
    })
}

fn lenient_tags<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    })
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default, deserialize_with = "lenient")]
    name: String,
    #[serde(default, deserialize_with = "lenient")]
    description: String,
    #[serde(default, deserialize_with = "lenient")]
    source: String,
    #[serde(default, deserialize_with = "lenient")]
    identifier: String,
    #[serde(default, deserialize_with = "lenient")]
    trust_level: String,
    #[serde(default, deserialize_with = "lenient")]
    repo: String,
    #[serde(default, deserialize_with = "lenient")]
    path: String,
    #[serde(default, deserialize_with = "lenient_tags")]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "lenient")]
    resolved_github_id: String,
}

/// A relative path inside a repo: no traversal, no empty or hidden-dot
/// segments, no backslashes. "" (the root) is valid.
fn clean_rel(path: &str) -> Option<String> {
    let p = path.trim().trim_matches('/');
    if p.is_empty() {
        return Some(String::new());
    }
    if p.len() > 400 || p.contains('\\') || p.contains("//") {
        return None;
    }
    if p.split('/')
        .any(|seg| seg == "." || seg == ".." || seg.is_empty())
    {
        return None;
    }
    Some(p.to_string())
}

fn clip(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// The repo Hermes' official optional skills live in.
pub const OFFICIAL_REPO: &str = "NousResearch/hermes-agent";

/// Hermes' `_SOURCE_PREFIX_ALIASES` (tools/skills_guard.py): the prefixes
/// its trust resolver strips before matching a repo.
const SOURCE_PREFIX_ALIASES: [&str; 4] = ["skills-sh/", "skills.sh/", "skils-sh/", "skils.sh/"];

/// Whether a row's trust can be read from the same place its files come
/// from. The scan resolves trust from the identifier (or, for an official
/// row, from the word "official", which never blocks), while the fetch reads
/// `repo`. They have to agree, or a row could carry one repo's trust onto
/// another repo's files:
///   - official: only Hermes' own optional skills, in Hermes' own repo
///   - github / skills.sh: the identifier, with Hermes' aliases stripped,
///     is `repo` or a path inside it — compared exactly, as Hermes compares
///     it to TRUSTED_REPOS
///
/// Every row of the 2026-10-22 index passes (153 official, 1,093 GitHub,
/// 18,411 skills.sh). The check guards against a future row, not against
/// any row in the index today.
pub fn provenance_ok(source: &str, identifier: &str, repo: &str, path: &str) -> bool {
    if source == "official" {
        return repo == OFFICIAL_REPO
            && identifier.starts_with("official/")
            && path.starts_with("optional-skills/");
    }
    let bare = SOURCE_PREFIX_ALIASES
        .iter()
        .find_map(|p| identifier.strip_prefix(p))
        .unwrap_or(identifier);
    bare.strip_prefix(repo)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

impl RawEntry {
    /// The row as the mirror stores it, or None when Talaria could not
    /// install it — a registry with its own protocol, or a repo or path
    /// that does not validate.
    fn installable(self) -> Option<CatalogEntry> {
        let source = match self.source.as_str() {
            "official" => "official",
            "github" => "github",
            "skills.sh" | "skills-sh" => "skills.sh",
            _ => return None,
        };
        let repo = self.repo.trim();
        let identifier = self.identifier.trim();
        let name = self.name.trim();
        if !valid_repo(repo) || identifier.is_empty() || identifier.len() > 400 || name.is_empty() {
            return None;
        }
        // skills.sh rows carry the slug as `path` and the real location as
        // `resolved_github_id` (`owner/repo/<dir>`); a row the index never
        // resolved keeps the slug and is found by name at fetch time.
        let (path, exact) = if source == "skills.sh" {
            match self.resolved_github_id.trim().strip_prefix(repo) {
                Some("") => (String::new(), true),
                Some(rest) if rest.starts_with('/') => (clean_rel(rest)?, true),
                _ => (clean_rel(&self.path)?, false),
            }
        } else {
            (clean_rel(&self.path)?, true)
        };
        if !exact && path.is_empty() {
            return None;
        }
        if !provenance_ok(source, identifier, repo, &path) {
            return None;
        }
        let trust_level = match self.trust_level.as_str() {
            t @ ("builtin" | "trusted" | "community") => t,
            _ => "community",
        };
        Some(CatalogEntry {
            id: identifier.to_string(),
            name: clip(name, 120),
            description: clip(&self.description, 1000),
            source: source.to_string(),
            trust_level: trust_level.to_string(),
            repo: repo.to_string(),
            path,
            exact,
            tags: self
                .tags
                .iter()
                .map(|t| clip(t, 60))
                .filter(|t| !t.is_empty())
                .take(20)
                .collect(),
        })
    }
}

/// The parsed index: its stamp and the installable rows, in index order
/// (Hermes lists its official skills first), deduped by identifier.
#[derive(Debug, Default)]
pub struct ParsedIndex {
    pub generated_at: Option<String>,
    pub entries: Vec<CatalogEntry>,
    /// Every row the index carried, installable or not.
    pub total: usize,
}

struct Installable {
    entries: Vec<CatalogEntry>,
    total: usize,
}

impl<'de> Deserialize<'de> for Installable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct SeqV;
        impl<'de> Visitor<'de> for SeqV {
            type Value = Installable;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("the index's skills array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Installable, A::Error> {
                let mut entries: Vec<CatalogEntry> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let mut total = 0;
                while let Some(raw) = seq.next_element::<RawEntry>()? {
                    total += 1;
                    if let Some(e) = raw.installable()
                        && seen.insert(e.id.clone())
                    {
                        entries.push(e);
                    }
                }
                Ok(Installable { entries, total })
            }
        }
        d.deserialize_seq(SeqV)
    }
}

impl<'de> Deserialize<'de> for ParsedIndex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct MapV;
        impl<'de> Visitor<'de> for MapV {
            type Value = ParsedIndex;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("the skills index object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ParsedIndex, A::Error> {
                let mut out = ParsedIndex::default();
                let mut saw_skills = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generated_at" => {
                            out.generated_at = match map.next_value::<Value>()? {
                                Value::String(s) => Some(s),
                                _ => None,
                            }
                        }
                        "skills" => {
                            let inst = map.next_value::<Installable>()?;
                            out.entries = inst.entries;
                            out.total = inst.total;
                            saw_skills = true;
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                if !saw_skills {
                    return Err(de::Error::missing_field("skills"));
                }
                Ok(out)
            }
        }
        d.deserialize_map(MapV)
    }
}

/// Parse an index body as served: gzip (the magic bytes say so — the
/// fetch asked for it, and safe_fetch hands back the wire bytes) or plain
/// JSON. Streamed either way: the decoded document is never held whole.
pub fn parse_index(body: &[u8]) -> Result<ParsedIndex, String> {
    let parsed = if body.starts_with(&[0x1f, 0x8b]) {
        let gz = flate2::read::GzDecoder::new(body).take(DECODED_MAX_BYTES);
        serde_json::from_reader::<_, ParsedIndex>(std::io::BufReader::new(gz))
    } else {
        serde_json::from_slice::<ParsedIndex>(body)
    };
    parsed.map_err(|e| format!("the skills index did not parse: {e}"))
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SyncState {
    etag: Option<String>,
    generated_at: Option<String>,
    /// Epoch ms — the last sync that completed (a 304 counts).
    last_synced_at: Option<i64>,
    /// Epoch ms — the last sync that STARTED; the empty-mirror nudge waits
    /// this out.
    last_attempt_at: Option<i64>,
    last_error: Option<String>,
    /// Installable rows the last full parse kept, and the rows it read.
    count: Option<usize>,
    total: Option<usize>,
    /// `entries_digest` of the rows the mirror holds: an index whose rows
    /// hash the same is not written again.
    entries_digest: Option<String>,
}

/// sha256 over the parsed rows in index order — every field the mirror
/// stores, each length-prefixed so no two lists hash alike, and not the
/// index's `generated_at`, which moves on every rebuild whether or not a row
/// did. An index that only reorders its rows hashes differently, and its
/// store then finds nothing to write.
fn entries_digest(entries: &[CatalogEntry]) -> String {
    fn field(h: &mut Sha256, f: &str) {
        h.update((f.len() as u64).to_le_bytes());
        h.update(f.as_bytes());
    }
    let mut h = Sha256::new();
    for e in entries {
        for f in [
            &e.id,
            &e.name,
            &e.description,
            &e.source,
            &e.trust_level,
            &e.repo,
            &e.path,
        ] {
            field(&mut h, f);
        }
        h.update([u8::from(e.exact)]);
        h.update((e.tags.len() as u64).to_le_bytes());
        for t in &e.tags {
            field(&mut h, t);
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn load_state(pg: &PgPool) -> SyncState {
    serde_json::from_value(talaria_settings::get_setting(pg, SYNC_KEY, Value::Null).await)
        .unwrap_or_default()
}

async fn save_state(pg: &PgPool, st: &SyncState) {
    if let Ok(v) = serde_json::to_value(st)
        && let Err(e) = talaria_settings::set_setting(pg, SYNC_KEY, &v).await
    {
        tracing::warn!("[skills-index] sync state write failed: {e}");
    }
}

/// One sync at a time in this process — the job and the empty-mirror nudge
/// share it. Across instances the job's lease does the same.
static SYNC_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Whether the mirror holds any row — an index probe, not a count.
async fn mirror_filled(pg: &PgPool) -> Result<bool, sqlx::Error> {
    let (filled,): (bool,) = sqlx::query_as("select exists (select 1 from skills_index_entries)")
        .fetch_one(pg)
        .await?;
    Ok(filled)
}

/// Fetch, parse and store the index. `Ok` carries the log line.
pub async fn sync_once(pg: &PgPool) -> Result<String, String> {
    let _guard = SYNC_LOCK.lock().await;
    let mut st = load_state(pg).await;
    st.last_attempt_at = Some(now_ms());
    save_state(pg, &st).await;
    let result = sync_inner(pg, &mut st).await;
    match &result {
        Ok(_) => {
            st.last_error = None;
            st.last_synced_at = Some(now_ms());
        }
        Err(e) => st.last_error = Some(e.clone()),
    }
    save_state(pg, &st).await;
    result
}

async fn sync_inner(pg: &PgPool, st: &mut SyncState) -> Result<String, String> {
    let have_rows = mirror_filled(pg).await.map_err(|e| e.to_string())?;
    let etag = st.etag.clone().filter(|_| have_rows);
    let mut headers = vec![("Accept-Encoding", "gzip")];
    if let Some(tag) = etag.as_deref() {
        headers.push(("If-None-Match", tag));
    }
    let resp = safe_fetch(
        INDEX_URL,
        SafeFetch {
            headers,
            timeout_ms: Some(FETCH_TIMEOUT_MS),
            max_bytes: Some(FETCH_MAX_BYTES),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| format!("Hermes' skills index unreachable: {e}"))?;
    if resp.status == 304 {
        return Ok("skills index unchanged (304)".into());
    }
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "Hermes' skills index answered {} — the mirror keeps what it has",
            resp.status
        ));
    }
    let new_etag = resp
        .headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.body;
    let parsed = tokio::task::spawn_blocking(move || parse_index(&body))
        .await
        .map_err(|e| format!("the index parse died: {e}"))??;
    apply_index(pg, st, parsed, new_etag).await
}

/// A parsed index → the mirror and the sync state. The fetch's half of a
/// sync stays in `sync_inner`; this half is what the live census drives.
async fn apply_index(
    pg: &PgPool,
    st: &mut SyncState,
    parsed: ParsedIndex,
    new_etag: Option<String>,
) -> Result<String, String> {
    if parsed.entries.len() < MIN_INSTALLABLE {
        return Err(format!(
            "Hermes' skills index carried only {} installable rows of {} — refusing to shrink the mirror to that",
            parsed.entries.len(),
            parsed.total
        ));
    }
    let digest = entries_digest(&parsed.entries);
    let filled = mirror_filled(pg).await.map_err(|e| e.to_string())?;
    // The same rows as the mirror's: nothing to write. The new ETag is still
    // kept, so the next fetch can be a 304.
    let stored = if filled && st.entries_digest.as_deref() == Some(digest.as_str()) {
        None
    } else {
        Some(
            store(pg, &parsed.entries)
                .await
                .map_err(|e| e.to_string())?,
        )
    };
    st.etag = new_etag;
    st.generated_at = parsed.generated_at.clone();
    st.count = Some(parsed.entries.len());
    st.total = Some(parsed.total);
    st.entries_digest = Some(digest);
    let generated = parsed
        .generated_at
        .map(|g| format!(", generated {g}"))
        .unwrap_or_default();
    Ok(match stored {
        None => format!(
            "skills index unchanged: {} installable of {} rows, the mirror's already{generated}",
            parsed.entries.len(),
            parsed.total,
        ),
        Some(w) => format!(
            "skills index mirrored: {} installable of {} rows ({} changed, {} new, {} gone){generated}",
            parsed.entries.len(),
            parsed.total,
            w.updated,
            w.inserted,
            w.gone,
        ),
    })
}

/// What a store wrote: rows that differed and were updated, rows that were
/// new, rows the index no longer carries.
#[derive(Debug, Default, PartialEq)]
struct Stored {
    updated: u64,
    inserted: u64,
    gone: u64,
}

/// The rows of a batch that differ from the mirror's in a stored field; a
/// row equal in every one is not touched, wherever it now sits in the list
/// (the position is not compared: a skill added above it moves it, and
/// nothing about it changed). `synced_at` is when a row last changed.
const UPDATE_CHANGED: &str = "update skills_index_entries e set \
       name = u.n, description = u.d, source = u.s, trust_level = u.t, repo = u.r, \
       path = u.p, exact = u.x, tags = u.g::jsonb, synced_at = now() \
     from unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::text[], $8::bool[], $9::text[]) \
       as u(i, n, d, s, t, r, p, x, g) \
     where e.identifier = u.i \
       and (e.name, e.description, e.source, e.trust_level, e.repo, e.path, e.exact, e.tags) \
           is distinct from (u.n, u.d, u.s, u.t, u.r, u.p, u.x, u.g::jsonb)";

/// The rows of a batch the mirror lacks, each with its position in this
/// index as `rank` (read only by a release before this one).
const INSERT_NEW: &str = "insert into skills_index_entries \
       (identifier, name, description, source, trust_level, repo, path, exact, tags, rank, synced_at) \
     select i, n, d, s, t, r, p, x, g::jsonb, k, now() \
     from unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::text[], $8::bool[], $9::text[], $10::int4[]) \
       as u(i, n, d, s, t, r, p, x, g, k) \
     on conflict (identifier) do nothing";

/// The rows the index no longer carries: an anti-join against the whole
/// list (hashed), not `identifier <> all(…)`, which is rows × list.
const DELETE_GONE: &str = "delete from skills_index_entries e \
     where not exists (select 1 from unnest($1::text[]) as i(id) where i.id = e.identifier)";

/// Update the rows that differ, insert the new ones, delete what the index
/// no longer carries — one transaction, so a search sees the old mirror or
/// the new one, never half. A row equal to its index entry is never
/// rewritten, however far the rows above it moved it.
async fn store(pg: &PgPool, entries: &[CatalogEntry]) -> Result<Stored, sqlx::Error> {
    let mut out = Stored::default();
    let mut tx = pg.begin().await?;
    for (batch_no, batch) in entries.chunks(UPSERT_BATCH).enumerate() {
        let base = (batch_no * UPSERT_BATCH) as i32;
        let ids: Vec<&str> = batch.iter().map(|e| e.id.as_str()).collect();
        let names: Vec<&str> = batch.iter().map(|e| e.name.as_str()).collect();
        let descs: Vec<&str> = batch.iter().map(|e| e.description.as_str()).collect();
        let sources: Vec<&str> = batch.iter().map(|e| e.source.as_str()).collect();
        let trusts: Vec<&str> = batch.iter().map(|e| e.trust_level.as_str()).collect();
        let repos: Vec<&str> = batch.iter().map(|e| e.repo.as_str()).collect();
        let paths: Vec<&str> = batch.iter().map(|e| e.path.as_str()).collect();
        let exacts: Vec<bool> = batch.iter().map(|e| e.exact).collect();
        let tags: Vec<String> = batch
            .iter()
            .map(|e| serde_json::to_string(&e.tags).unwrap_or_else(|_| "[]".into()))
            .collect();
        let ranks: Vec<i32> = (0..batch.len() as i32).map(|i| base + i).collect();
        // The batch's columns, bound alike to both statements.
        macro_rules! rows {
            ($sql:expr) => {
                sqlx::query($sql)
                    .bind(&ids)
                    .bind(&names)
                    .bind(&descs)
                    .bind(&sources)
                    .bind(&trusts)
                    .bind(&repos)
                    .bind(&paths)
                    .bind(&exacts)
                    .bind(&tags)
            };
        }
        out.updated += rows!(UPDATE_CHANGED)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        out.inserted += rows!(INSERT_NEW)
            .bind(&ranks)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    out.gone = sqlx::query(DELETE_GONE)
        .bind(&ids)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(out)
}

// ── Reading ──────────────────────────────────────────────────────────────────

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    sqlx::types::Json<Vec<String>>,
);

fn from_row(r: Row) -> CatalogEntry {
    CatalogEntry {
        id: r.0,
        name: r.1,
        description: r.2,
        source: r.3,
        trust_level: r.4,
        repo: r.5,
        path: r.6,
        exact: r.7,
        tags: r.8.0,
    }
}

/// Ranking, in order: the name IS the query, trust (builtin, then trusted,
/// then community — the order Hermes' own search cuts by), the name starts
/// with the query, text rank, then the index's own order, read off the row
/// as Hermes sorts it: source (official, skills.sh, github — `source_order`
/// in Hermes' scripts/build_skills_index.py), then name, compared as Python
/// compares strings (code point order, which is `collate "C"`), then the
/// identifier for a name two skills share.
const SEARCH_SQL: &str = "with q as (select websearch_to_tsquery('simple', $1) as tq, $1::text as raw) \
     select identifier, name, description, source, trust_level, repo, path, exact, tags \
     from skills_index_entries e, q \
     where $1 = '' \
        or e.search @@ q.tq \
        or e.name ilike '%' || q.raw || '%' \
        or e.repo ilike '%' || q.raw || '%' \
     order by \
       (lower(e.name) = lower(q.raw)) desc, \
       case e.trust_level when 'builtin' then 0 when 'trusted' then 1 else 2 end, \
       (e.name ilike q.raw || '%') desc, \
       ts_rank_cd(e.search, q.tq) desc, \
       case e.source when 'official' then 0 when 'skills.sh' then 1 else 2 end, \
       e.name collate \"C\", \
       e.identifier \
     limit $2";

/// Search the mirror. An empty mirror nudges a sync first (once per five
/// minutes, in-process single-flight); a failed one is the error the picker
/// shows — "the catalog is unreachable" and "nothing matches" are different
/// facts.
pub async fn search(pg: &PgPool, query: &str) -> Result<Vec<CatalogEntry>, String> {
    if !mirror_filled(pg).await.map_err(|e| e.to_string())? {
        nudge(pg).await?;
    }
    let q = query.trim();
    let rows: Vec<Row> = sqlx::query_as(SEARCH_SQL)
        .bind(q)
        .bind(PAGE)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(from_row).collect())
}

async fn nudge(pg: &PgPool) -> Result<(), String> {
    let st = load_state(pg).await;
    let recent = st
        .last_attempt_at
        .is_some_and(|at| now_ms() - at < NUDGE_EVERY_MS);
    if recent {
        // A sync started moments ago: wait for it (the lock) and read what
        // it left, rather than starting a second.
        drop(SYNC_LOCK.lock().await);
        if mirror_filled(pg).await.map_err(|e| e.to_string())? {
            return Ok(());
        }
        let st = load_state(pg).await;
        return Err(st
            .last_error
            .unwrap_or_else(|| "the skills catalog is still syncing — try again shortly".into()));
    }
    sync_once(pg).await.map(|_| ())
}

/// One catalog row by its identifier — the install's resolver.
pub async fn entry(pg: &PgPool, id: &str) -> Result<Option<CatalogEntry>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(
        "select identifier, name, description, source, trust_level, repo, path, exact, tags \
         from skills_index_entries where identifier = $1",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(from_row))
}

/// The catalog sync job. Leased, not per-instance: the rows are shared.
pub fn skills_index_refresh_job_spec(pg: PgPool) -> JobSpec {
    JobSpec {
        name: JobName::SkillsIndexRefresh,
        every_ms: REFRESH_EVERY_MS,
        first_run_delay_ms: Some(REFRESH_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(REFRESH_MAX_RUN_MS),
        per_instance: false,
        run: std::sync::Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { sync_once(&pg).await.map(Some) })
        }),
    }
}

/// Register the sync with the scheduler — the boot list's one call.
pub fn register_skills_index_refresh_job(pg: PgPool) {
    talaria_scheduler::register_job(skills_index_refresh_job_spec(pg));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn index(rows: Value) -> Value {
        serde_json::json!({
            "version": 1,
            "generated_at": "2026-10-22T07:33:53Z",
            "skill_count": 9,
            "skills": rows,
        })
    }

    /// Shapes copied from the live index (2026-10-22), one per source.
    fn live_rows() -> Value {
        serde_json::json!([
            {"name":"1password","description":"Set up op CLI, sign in, and read or inject secrets.","source":"official","identifier":"official/security/1password","trust_level":"builtin","repo":"NousResearch/hermes-agent","path":"optional-skills/security/1password","tags":["security","secrets"],"extra":{}},
            {"name":"13c-metabolic-flux","description":"Estimates intracellular metabolic fluxes.","source":"github","identifier":"K-Dense-AI/scientific-agent-skills/skills/13c-metabolic-flux","trust_level":"community","repo":"K-Dense-AI/scientific-agent-skills","path":"skills/13c-metabolic-flux","tags":[],"extra":{"category":"science"}},
            {"name":"1-3-1","description":"Indexed by skills.sh from timschoch/skilly","source":"skills.sh","identifier":"skills-sh/timschoch/skilly/1-3-1","trust_level":"community","repo":"timschoch/skilly","path":"1-3-1","tags":[],"extra":{"detail_url":"https://skills.sh/timschoch/skilly/1-3-1"},"resolved_github_id":"timschoch/skilly/bundles/workflow/skills/1-3-1"},
            {"name":"unresolved","description":"","source":"skills.sh","identifier":"skills-sh/a/b/unresolved","trust_level":"community","repo":"a/b","path":"unresolved","tags":[],"extra":{}},
            {"name":"rooted","description":"","source":"skills.sh","identifier":"skills-sh/a/rooted/rooted","trust_level":"trusted","repo":"a/rooted","path":"rooted","resolved_github_id":"a/rooted"},
            {"name":"ai-pm","description":"x","source":"clawhub","identifier":"ai-pm-prd-suite","trust_level":"community","repo":"","path":"","tags":[],"extra":{}},
            {"name":"9-somboon","description":"x","source":"lobehub","identifier":"lobehub/9-somboon","trust_level":"community","repo":"","path":""},
            {"name":"account","description":"x","source":"browse-sh","identifier":"browse-sh/x/y","trust_level":"community","repo":"","path":"","extra":{"proxies":true}},
            {"name":"escape","description":"x","source":"github","identifier":"evil/repo/../etc","trust_level":"community","repo":"evil/repo","path":"../etc"},
            {"name":42,"description":null,"source":"github","identifier":"odd/types/x","trust_level":"galaxy-brain","repo":"odd/types","path":"x","tags":"not-a-list"}
        ])
    }

    #[test]
    fn the_index_keeps_only_what_talaria_can_install() {
        let body = serde_json::to_vec(&index(live_rows())).unwrap();
        let parsed = parse_index(&body).unwrap();
        assert_eq!(parsed.total, 10);
        assert_eq!(parsed.generated_at.as_deref(), Some("2026-10-22T07:33:53Z"));
        let ids: Vec<&str> = parsed.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "official/security/1password",
                "K-Dense-AI/scientific-agent-skills/skills/13c-metabolic-flux",
                "skills-sh/timschoch/skilly/1-3-1",
                "skills-sh/a/b/unresolved",
                "skills-sh/a/rooted/rooted",
            ],
            "clawhub, lobehub, browse-sh, a traversal and a nameless row stay out"
        );
        let official = &parsed.entries[0];
        assert_eq!(official.trust_level, "builtin");
        assert_eq!(official.path, "optional-skills/security/1password");
        assert!(official.exact);
        assert_eq!(official.tags, vec!["security", "secrets"]);
        // skills.sh: the resolved location wins over the slug…
        let resolved = &parsed.entries[2];
        assert_eq!(resolved.source, "skills.sh");
        assert_eq!(resolved.path, "bundles/workflow/skills/1-3-1");
        assert!(resolved.exact);
        // …an unresolved row keeps its slug for a by-name find…
        assert_eq!(parsed.entries[3].path, "unresolved");
        assert!(!parsed.entries[3].exact);
        // …and a row resolved to the repo itself is a root skill.
        assert_eq!(parsed.entries[4].path, "");
        assert!(parsed.entries[4].exact);
        assert_eq!(parsed.entries[4].trust_level, "trusted");
    }

    #[test]
    fn a_gzipped_index_parses_like_a_plain_one_and_dedupes() {
        let mut rows = live_rows();
        let dup = rows[0].clone();
        rows.as_array_mut().unwrap().push(dup);
        let raw = serde_json::to_vec(&index(rows)).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        let parsed = parse_index(&gz.finish().unwrap()).unwrap();
        assert_eq!(parsed.total, 11);
        assert_eq!(
            parsed.entries.len(),
            5,
            "the repeated identifier is kept once"
        );
    }

    #[test]
    fn a_document_without_skills_is_an_error_not_an_empty_mirror() {
        assert!(
            parse_index(br#"{"version":1}"#)
                .unwrap_err()
                .contains("skills")
        );
        assert!(parse_index(b"<html>").is_err());
    }

    #[test]
    fn odd_field_types_degrade_the_row_not_the_parse() {
        let body = serde_json::to_vec(&index(serde_json::json!([
            {"name":"ok","source":"github","identifier":"o/k/ok","trust_level":"wat","repo":"o/k","path":"ok","tags":[1,"two",null]}
        ])))
        .unwrap();
        let parsed = parse_index(&body).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].trust_level, "community");
        assert_eq!(parsed.entries[0].tags, vec!["two"]);
    }

    #[test]
    fn a_row_whose_identifier_names_another_repo_stays_out() {
        let body = serde_json::to_vec(&index(serde_json::json!([
            // The identifier would resolve as trusted (openai/skills); the
            // files would come from somewhere else.
            {"name":"x","source":"github","identifier":"openai/skills/skills/x","trust_level":"trusted","repo":"evil/repo","path":"skills/x"},
            {"name":"y","source":"skills.sh","identifier":"skills-sh/anthropics/skills/y","trust_level":"trusted","repo":"evil/repo","path":"y","resolved_github_id":"evil/repo/y"},
            // A sibling that shares the prefix is a different repo.
            {"name":"z","source":"github","identifier":"evil/repository/z","trust_level":"community","repo":"evil/repo","path":"z"},
            // "official" is never blocked, so it must be Hermes' own skill.
            {"name":"o","source":"official","identifier":"official/x/o","trust_level":"builtin","repo":"evil/repo","path":"optional-skills/x/o"},
            {"name":"p","source":"official","identifier":"official/x/p","trust_level":"builtin","repo":"NousResearch/hermes-agent","path":"skills/x/p"},
            {"name":"q","source":"official","identifier":"evil/repo/q","trust_level":"builtin","repo":"NousResearch/hermes-agent","path":"optional-skills/x/q"},
            // The shapes that are fine: Hermes' aliases, the repo root.
            {"name":"ok","source":"skills.sh","identifier":"skills.sh/good/repo/ok","trust_level":"community","repo":"good/repo","path":"ok","resolved_github_id":"good/repo/skills/ok"},
            {"name":"root","source":"github","identifier":"good/root","trust_level":"community","repo":"good/root","path":""}
        ])))
        .unwrap();
        let ids: Vec<String> = parse_index(&body)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, vec!["skills.sh/good/repo/ok", "good/root"]);
        assert!(provenance_ok(
            "official",
            "official/security/1password",
            OFFICIAL_REPO,
            "optional-skills/security/1password"
        ));
    }

    #[test]
    fn rel_paths_refuse_traversal_and_keep_the_root() {
        assert_eq!(clean_rel("").as_deref(), Some(""));
        assert_eq!(clean_rel("/skills/a/").as_deref(), Some("skills/a"));
        assert_eq!(clean_rel("a/../b"), None);
        assert_eq!(clean_rel("a//b"), None);
        assert_eq!(clean_rel("a\\b"), None);
        assert_eq!(clean_rel("./a"), None);
    }
}

// Live CENSUS of the mirror's writes — a measurement, not a behavior test:
// the store half of a sync (`apply_index`) over a production-sized synthetic
// index of 19,742 installable rows, sorted as Hermes sorts it, in the shapes
// a refresh meets (an identical index, 1% of the rows changed, one row
// inserted at the top, and a real crawl: a few skills added and removed at
// their alphabetical places and a few descriptions edited), printed for the
// pull request that changes it. A CHECKPOINT before each
// sync stands for the six hours between them. Per sync: write transactions
// (the `pg_snapshot_xmax` delta), WAL bytes (`pg_current_wal_lsn`),
// `pg_stat_wal`'s records and full-page images, and the rows written (xmin
// moved, inserted, deleted). The WAL counters are the cluster's, so it wants
// a database nothing else is connected to; it returns at once without
// TALARIA_WRITE_CENSUS=1, so CI's --ignored step passes over it:
//
//   TALARIA_WRITE_CENSUS=1 DATABASE_URL=postgres://… \
//     cargo test -p talaria-skills-marketplace --lib -- --ignored --nocapture skills_store_live_census
#[cfg(test)]
mod store_census {
    use std::collections::HashMap;
    use std::str::FromStr;

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;

    const ROWS: usize = 19_742;
    const SCHEMA: &str = "skills_index_census";
    const WORDS: [&str; 40] = [
        "agent",
        "browser",
        "calendar",
        "code",
        "data",
        "deploy",
        "design",
        "docs",
        "email",
        "files",
        "finance",
        "git",
        "image",
        "infra",
        "json",
        "kubernetes",
        "legal",
        "logs",
        "maps",
        "markdown",
        "memory",
        "metrics",
        "notes",
        "pdf",
        "python",
        "research",
        "review",
        "rust",
        "search",
        "security",
        "sheets",
        "slides",
        "sql",
        "testing",
        "travel",
        "video",
        "weather",
        "web",
        "writing",
        "youtube",
    ];

    /// splitmix64: a fixed stream per row, so every census builds the same index.
    fn mix(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }

    fn word(i: usize, k: u64) -> &'static str {
        WORDS[(mix(i as u64 * 131 + k) % WORDS.len() as u64) as usize]
    }

    /// Row `i` of the synthetic index: the first 153 are Hermes' own
    /// optional skills, the next 1,093 GitHub taps, the rest skills.sh.
    /// `hermes_sorted` puts a list of them in the index's order.
    pub(super) fn synthetic(i: usize) -> CatalogEntry {
        let (source, trust, repo, path, id) = if i < 153 {
            let path = format!("optional-skills/{}/{}-{i}", word(i, 1), word(i, 2));
            let id = format!("official/{}/{}-{i}", word(i, 1), word(i, 2));
            ("official", "builtin", OFFICIAL_REPO.to_string(), path, id)
        } else if i < 153 + 1_093 {
            let repo = format!("tap-{}/{}-skills", i % 37, word(i, 3));
            let path = format!("skills/{}-{i}", word(i, 2));
            let id = format!("{repo}/{path}");
            let trust = if i % 9 == 0 { "trusted" } else { "community" };
            ("github", trust, repo, path, id)
        } else {
            let repo = format!("owner-{}/{}-{}", i % 2_311, word(i, 4), i % 97);
            let path = format!("skills/{}-{i}", word(i, 2));
            let id = format!("skills-sh/{repo}/{}-{i}", word(i, 2));
            ("skills.sh", "community", repo, path, id)
        };
        let description: String = (0..34)
            .map(|k| word(i, 100 + k))
            .collect::<Vec<_>>()
            .join(" ");
        CatalogEntry {
            id,
            name: format!("{}-{}-{i}", word(i, 2), word(i, 5)),
            description,
            source: source.into(),
            trust_level: trust.into(),
            repo,
            path,
            exact: i % 11 != 0,
            tags: (0..(i % 4) as u64)
                .map(|k| word(i, 50 + k).to_string())
                .collect(),
        }
    }

    /// The order Hermes builds its index in (`source_order`, then name, in
    /// scripts/build_skills_index.py) — the order SEARCH_SQL reads back.
    pub(super) fn hermes_sorted(mut entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        let group = |s: &str| match s {
            "official" => 0,
            "skills.sh" => 1,
            _ => 2,
        };
        entries.sort_by(|a, b| (group(&a.source), &a.name).cmp(&(group(&b.source), &b.name)));
        entries
    }

    fn census_url() -> Option<String> {
        if std::env::var("TALARIA_WRITE_CENSUS").ok().as_deref() != Some("1") {
            eprintln!("census skipped: set TALARIA_WRITE_CENSUS=1 and DATABASE_URL to run it");
            return None;
        }
        Some(std::env::var("DATABASE_URL").expect("DATABASE_URL for the census"))
    }

    #[derive(Clone, Copy, Default)]
    struct Mark {
        xmax: i64,
        lsn: i64,
        records: i64,
        fpi: i64,
    }

    async fn mark(pg: &PgPool) -> Mark {
        sqlx::query("select pg_stat_force_next_flush()")
            .execute(pg)
            .await
            .unwrap();
        let (xmax, lsn, records, fpi): (i64, i64, i64, i64) = sqlx::query_as(
            "select pg_snapshot_xmax(pg_current_snapshot())::text::int8, \
               pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0')::int8, \
               w.wal_records, w.wal_fpi from pg_stat_wal w",
        )
        .fetch_one(pg)
        .await
        .unwrap();
        Mark {
            xmax,
            lsn,
            records,
            fpi,
        }
    }

    async fn xmins(pg: &PgPool) -> HashMap<String, String> {
        sqlx::query_as::<_, (String, String)>(
            "select identifier, xmin::text from skills_index_entries",
        )
        .fetch_all(pg)
        .await
        .unwrap()
        .into_iter()
        .collect()
    }

    async fn exec(pg: &PgPool, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(pg)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    fn parsed(entries: &[CatalogEntry], stamp: &str) -> ParsedIndex {
        ParsedIndex {
            generated_at: Some(stamp.into()),
            entries: entries.to_vec(),
            total: 100_000,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn skills_store_live_census() {
        let Some(url) = census_url() else { return };
        let admin = PgPool::connect(&url).await.expect("the database");
        exec(&admin, &format!("drop schema if exists {SCHEMA} cascade")).await;
        exec(&admin, &format!("create schema {SCHEMA}")).await;
        exec(
            &admin,
            &format!(
                "create table {SCHEMA}.skills_index_entries \
                 (like public.skills_index_entries including all)"
            ),
        )
        .await;
        let opts = PgConnectOptions::from_str(&url)
            .expect("DATABASE_URL parses")
            .options([("search_path", SCHEMA)]);
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("census pool");
        let base = hermes_sorted((0..ROWS).map(synthetic).collect());
        let mut st = SyncState::default();
        apply_index(
            &pg,
            &mut st,
            parsed(&base, "2026-11-20T00:00:00Z"),
            Some("W/\"0\"".into()),
        )
        .await
        .expect("the first sync fills the mirror");
        exec(&pg, "vacuum (analyze) skills_index_entries").await;

        // 1% of the rows change their description; then one new official
        // skill lands at the top of the list, above every other row; then a
        // crawl the size of two real consecutive ones: 19 skills added at
        // their alphabetical places, 17 gone, 5 descriptions edited.
        let mut changed = base.clone();
        for e in changed.iter_mut().step_by(100) {
            e.description.push_str(" (updated)");
        }
        let mut top = synthetic(0);
        top.name = "0-first-skill".into();
        top.id = "official/new/0-first-skill".into();
        top.path = "optional-skills/new/0-first-skill".into();
        let mut inserted = changed.clone();
        inserted.insert(0, top);
        assert_eq!(
            hermes_sorted(inserted.clone()),
            inserted,
            "the top row sorts first"
        );
        // `n` distinct positions in 0..len, the same every census.
        let spots = |n: usize, len: usize, seed: u64| {
            let mut at = std::collections::BTreeSet::new();
            let mut k = seed;
            while at.len() < n {
                at.insert((mix(k) % len as u64) as usize);
                k += 1;
            }
            at
        };
        let removed = spots(17, inserted.len(), 2_000);
        let mut crawl: Vec<CatalogEntry> = inserted
            .iter()
            .enumerate()
            .filter(|(k, _)| !removed.contains(k))
            .map(|(_, e)| e.clone())
            .collect();
        for at in spots(5, crawl.len(), 3_000) {
            crawl[at].description.push_str(" (edited upstream)");
        }
        let added: Vec<CatalogEntry> = (0..19).map(|k| synthetic(ROWS + 100 + k)).collect();
        crawl.extend(added.iter().cloned());
        let crawl = hermes_sorted(crawl);
        let first_new = crawl
            .iter()
            .position(|e| added.iter().any(|a| a.id == e.id))
            .unwrap_or_default();
        println!(
            "census skills: {ROWS} rows, one CHECKPOINT before each sync; the crawl adds 19 \
             (the first at position {first_new}), removes 17, edits 5"
        );
        for (n, (label, entries)) in [
            ("identical index", &base),
            ("1% of rows changed", &changed),
            ("one row inserted at the top", &inserted),
            ("a real crawl", &crawl),
        ]
        .into_iter()
        .enumerate()
        {
            exec(&pg, "checkpoint").await;
            let before = xmins(&pg).await;
            let m0 = mark(&pg).await;
            let report = apply_index(
                &pg,
                &mut st,
                parsed(entries, &format!("2026-11-20T0{}:00:00Z", n + 1)),
                Some(format!("W/\"{}\"", n + 1)),
            )
            .await
            .expect("a sync");
            let m1 = mark(&pg).await;
            let after = xmins(&pg).await;
            let rewritten = after
                .iter()
                .filter(|(id, x)| before.get(*id).is_some_and(|b| b != *x))
                .count();
            let inserted_rows = after.keys().filter(|id| !before.contains_key(*id)).count();
            let deleted = before.keys().filter(|id| !after.contains_key(*id)).count();
            let wal = m1.lsn - m0.lsn;
            println!(
                "census skills per sync | {label} | {report} | write xacts {} | rows written {} \
                 (updated {rewritten}, inserted {inserted_rows}, deleted {deleted}) | \
                 WAL {:.2} MB ({wal} B) | records {} | FPI {} | per hour (÷6): WAL {:.2} MB, rows {:.0}",
                m1.xmax - m0.xmax,
                rewritten + inserted_rows + deleted,
                wal as f64 / 1e6,
                m1.records - m0.records,
                m1.fpi - m0.fpi,
                wal as f64 / 6e6,
                (rewritten + inserted_rows + deleted) as f64 / 6.0,
            );
        }
        pg.close().await;
        exec(&admin, &format!("drop schema if exists {SCHEMA} cascade")).await;
    }
}

// Live-DB proof that the mirror writes only what changed: an identical index
// takes the digest's skip and writes no row; without the digest the store
// itself finds nothing to write; one changed entry is one row version; a
// skill added mid-list is one new row and moves no other; a vanished entry
// is deleted; and the listing reads the same rows in the same order as the
// store before it did, which sorted by each row's stored position. `#[ignore]`d — it needs a database — and
// CI's live-tests job runs it (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-skills-marketplace --lib -- --ignored skills_store_live
#[cfg(test)]
mod store_live {
    use std::collections::HashMap;
    use std::str::FromStr;

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::store_census::{hermes_sorted, synthetic};
    use super::*;

    /// 40 official, 130 GitHub and 130 skills.sh rows, in the index's order.
    fn base_index() -> Vec<CatalogEntry> {
        hermes_sorted(
            (0..40)
                .chain(153..283)
                .chain(1_246..1_376)
                .map(synthetic)
                .collect(),
        )
    }

    /// The store as it was before it learned to leave an unchanged row
    /// alone: every row upserted with its position as `rank`, which that
    /// release's listing sorted by, then whatever the transaction did not
    /// stamp deleted. The listing must not tell the two apart.
    async fn old_store(pg: &PgPool, entries: &[CatalogEntry]) {
        let mut tx = pg.begin().await.unwrap();
        for (batch_no, batch) in entries.chunks(UPSERT_BATCH).enumerate() {
            let base = (batch_no * UPSERT_BATCH) as i32;
            let col = |f: fn(&CatalogEntry) -> &str| batch.iter().map(f).collect::<Vec<&str>>();
            let exacts: Vec<bool> = batch.iter().map(|e| e.exact).collect();
            let tags: Vec<String> = batch
                .iter()
                .map(|e| serde_json::to_string(&e.tags).unwrap())
                .collect();
            let ranks: Vec<i32> = (0..batch.len() as i32).map(|i| base + i).collect();
            sqlx::query(
                "insert into skills_index_entries \
                   (identifier, name, description, source, trust_level, repo, path, exact, tags, rank, synced_at) \
                 select i, n, d, s, t, r, p, x, g::jsonb, k, now() \
                 from unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                             $6::text[], $7::text[], $8::bool[], $9::text[], $10::int4[]) \
                   as u(i, n, d, s, t, r, p, x, g, k) \
                 on conflict (identifier) do update set \
                   name = excluded.name, description = excluded.description, \
                   source = excluded.source, trust_level = excluded.trust_level, \
                   repo = excluded.repo, path = excluded.path, exact = excluded.exact, \
                   tags = excluded.tags, rank = excluded.rank, synced_at = excluded.synced_at",
            )
            .bind(col(|e| &e.id))
            .bind(col(|e| &e.name))
            .bind(col(|e| &e.description))
            .bind(col(|e| &e.source))
            .bind(col(|e| &e.trust_level))
            .bind(col(|e| &e.repo))
            .bind(col(|e| &e.path))
            .bind(&exacts)
            .bind(&tags)
            .bind(&ranks)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        sqlx::query("delete from skills_index_entries where synced_at < now()")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn scratch(admin: &PgPool, url: &str, tag: &str) -> (PgPool, String) {
        let schema = format!("skills_store_live_{tag}_{}", std::process::id());
        for sql in [
            format!("drop schema if exists {schema} cascade"),
            format!("create schema {schema}"),
            format!(
                "create table {schema}.skills_index_entries \
                 (like public.skills_index_entries including all)"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .execute(admin)
                .await
                .expect(&sql);
        }
        let opts = PgConnectOptions::from_str(url)
            .expect("DATABASE_URL parses")
            .options([("search_path", schema.as_str())]);
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("scratch pool");
        (pg, schema)
    }

    async fn xmins(pg: &PgPool) -> HashMap<String, String> {
        sqlx::query_as::<_, (String, String)>(
            "select identifier, xmin::text from skills_index_entries",
        )
        .fetch_all(pg)
        .await
        .unwrap()
        .into_iter()
        .collect()
    }

    /// (inserted, updated, deleted) row versions so far, this session's own
    /// counts flushed first.
    async fn writes(pg: &PgPool) -> (i64, i64, i64) {
        sqlx::query("select pg_stat_force_next_flush()")
            .execute(pg)
            .await
            .unwrap();
        sqlx::query_as(
            "select n_tup_ins, n_tup_upd, n_tup_del from pg_stat_user_tables \
             where relid = 'skills_index_entries'::regclass",
        )
        .fetch_one(pg)
        .await
        .unwrap()
    }

    fn delta(a: (i64, i64, i64), b: (i64, i64, i64)) -> (i64, i64, i64) {
        (b.0 - a.0, b.1 - a.1, b.2 - a.2)
    }

    fn index(entries: &[CatalogEntry], n: usize) -> ParsedIndex {
        ParsedIndex {
            generated_at: Some(format!("2026-11-2{n}T00:00:00Z")),
            entries: entries.to_vec(),
            total: entries.len() * 5,
        }
    }

    /// Every stored column but `synced_at` and `rank` (the old store's
    /// moving position; this one keeps a row's first), in identifier order.
    async fn contents(pg: &PgPool) -> Vec<(String, String, String, String, bool, String)> {
        sqlx::query_as(
            "select identifier, name || '|' || description || '|' || source || '|' || trust_level, \
               repo, path, exact, tags::text \
             from skills_index_entries order by identifier",
        )
        .fetch_all(pg)
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore]
    async fn skills_store_live_identical_index_writes_nothing() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
        let admin = PgPool::connect(&url).await.expect("the database");
        let (pg, schema) = scratch(&admin, &url, "new").await;
        let (old, old_schema) = scratch(&admin, &url, "old").await;
        let base = base_index();
        let mut st = SyncState::default();

        let report = apply_index(&pg, &mut st, index(&base, 0), Some("W/\"a\"".into()))
            .await
            .unwrap();
        assert!(report.contains("(0 changed, 300 new, 0 gone)"), "{report}");

        // The same rows under a new ETag and a new generated_at: the digest
        // matches, nothing is written, the new ETag is kept.
        let rows0 = xmins(&pg).await;
        let w0 = writes(&pg).await;
        let report = apply_index(&pg, &mut st, index(&base, 1), Some("W/\"b\"".into()))
            .await
            .unwrap();
        assert!(report.starts_with("skills index unchanged"), "{report}");
        assert_eq!(xmins(&pg).await, rows0, "no row version moved");
        assert_eq!(delta(w0, writes(&pg).await), (0, 0, 0));
        assert_eq!(st.etag.as_deref(), Some("W/\"b\""));

        // A state saved before the digest existed: the store runs, and finds
        // every row equal to its entry.
        st.entries_digest = None;
        let report = apply_index(&pg, &mut st, index(&base, 2), Some("W/\"c\"".into()))
            .await
            .unwrap();
        assert!(report.contains("(0 changed, 0 new, 0 gone)"), "{report}");
        assert_eq!(xmins(&pg).await, rows0);
        assert_eq!(delta(w0, writes(&pg).await), (0, 0, 0));
        assert!(
            st.entries_digest.is_some(),
            "the digest is stored for next time"
        );

        // One entry changed: one row version.
        let mut one = base.clone();
        one[5].description.push_str(" (changed upstream)");
        let w1 = writes(&pg).await;
        let report = apply_index(&pg, &mut st, index(&one, 3), None)
            .await
            .unwrap();
        assert!(report.contains("(1 changed, 0 new, 0 gone)"), "{report}");
        let rows1 = xmins(&pg).await;
        let moved: Vec<&String> = rows1
            .iter()
            .filter(|(id, x)| rows0.get(*id) != Some(*x))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(moved, vec![&one[5].id]);
        assert_eq!(delta(w1, writes(&pg).await), (0, 1, 0));

        // A skill added mid-list, at its alphabetical place: every row under
        // it moves down one place in the index, and the mirror writes the
        // one new row and nothing else.
        let added = synthetic(5_000);
        let more = hermes_sorted([one.clone(), vec![added.clone()]].concat());
        let at = more.iter().position(|e| e.id == added.id).unwrap();
        assert!(
            (1..more.len() - 100).contains(&at),
            "mid-list, with rows under it: {at}"
        );
        let rows1 = xmins(&pg).await;
        let w2 = writes(&pg).await;
        let report = apply_index(&pg, &mut st, index(&more, 4), None)
            .await
            .unwrap();
        assert!(report.contains("(0 changed, 1 new, 0 gone)"), "{report}");
        assert_eq!(delta(w2, writes(&pg).await), (1, 0, 0));
        let rows2 = xmins(&pg).await;
        assert!(
            rows1.iter().all(|(id, x)| rows2.get(id) == Some(x)),
            "no row under the new one is rewritten"
        );

        // The last entry left the index: deleted, and nothing else written.
        let mut fewer = more.clone();
        let gone = fewer.pop().unwrap();
        let w3 = writes(&pg).await;
        let report = apply_index(&pg, &mut st, index(&fewer, 5), None)
            .await
            .unwrap();
        assert!(report.contains("(0 changed, 0 new, 1 gone)"), "{report}");
        assert_eq!(delta(w3, writes(&pg).await), (0, 0, 1));
        assert!(entry(&pg, &gone.id).await.unwrap().is_none());

        // The old store over the same history reads back the same mirror:
        // every column but its moving rank, and every listing in the same
        // order (official, then skills.sh, then GitHub within each trust
        // level, and by name within each).
        old_store(&old, &base).await;
        old_store(&old, &one).await;
        old_store(&old, &more).await;
        old_store(&old, &fewer).await;
        assert_eq!(contents(&pg).await, contents(&old).await);
        for q in [
            "",
            "rust",
            "agent search",
            fewer[3].name.as_str(),
            "skills",
            added.name.as_str(),
        ] {
            assert_eq!(
                search(&pg, q).await.unwrap(),
                search(&old, q).await.unwrap(),
                "the listing for {q:?}"
            );
        }

        pg.close().await;
        old.close().await;
        for s in [schema, old_schema] {
            let drop = format!("drop schema if exists {s} cascade");
            sqlx::query(sqlx::AssertSqlSafe(drop.as_str()))
                .execute(&admin)
                .await
                .unwrap();
        }
    }
}
