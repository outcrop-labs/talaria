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
// rows are kept as they go by, the rest is never held) and upserted in one
// transaction, which also deletes the rows the index no longer carries. A
// search that finds the mirror empty nudges a sync itself, at most once per
// five minutes, so a fresh install's first open still answers.

use std::fmt;
use std::io::Read;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    if parsed.entries.len() < MIN_INSTALLABLE {
        return Err(format!(
            "Hermes' skills index carried only {} installable rows of {} — refusing to shrink the mirror to that",
            parsed.entries.len(),
            parsed.total
        ));
    }
    let stored = store(pg, &parsed.entries)
        .await
        .map_err(|e| e.to_string())?;
    st.etag = new_etag;
    st.generated_at = parsed.generated_at.clone();
    st.count = Some(parsed.entries.len());
    st.total = Some(parsed.total);
    Ok(format!(
        "skills index mirrored: {} installable of {} rows ({} gone){}",
        parsed.entries.len(),
        parsed.total,
        stored,
        parsed
            .generated_at
            .map(|g| format!(", generated {g}"))
            .unwrap_or_default()
    ))
}

/// Upsert every row and delete what the index no longer carries — one
/// transaction, so a search sees the old mirror or the new one, never half.
/// `now()` is the transaction's own timestamp: every upsert stamps it, and
/// the delete takes whatever is older. Returns how many rows went.
async fn store(pg: &PgPool, entries: &[CatalogEntry]) -> Result<u64, sqlx::Error> {
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
        .bind(&ids)
        .bind(&names)
        .bind(&descs)
        .bind(&sources)
        .bind(&trusts)
        .bind(&repos)
        .bind(&paths)
        .bind(&exacts)
        .bind(&tags)
        .bind(&ranks)
        .execute(&mut *tx)
        .await?;
    }
    let gone = sqlx::query("delete from skills_index_entries where synced_at < now()")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(gone)
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
/// with the query, text rank, then the index's own order.
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
       e.rank asc \
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
