// What each GitHub host's App asks for, and what each installation it uses
// has accepted, kept in `git_host_installations`: the one record the mint,
// the alerts and Admin → Git hosts read.
//
// WHY A TABLE. GitHub answers both questions live (`GET /app` for what the
// App's registration asks for, `GET /app/installations/{id}` for what an
// installation accepted), and an api process used to ask on every cold
// cache. That left nothing for anyone else to read: an alert cannot ask
// GitHub on every page load, a second api process starts cold, and a change
// an org owner has not approved yet was visible nowhere. Postgres holds the
// last answer instead, with when it was read and why the last read failed.
//
// WHEN IT IS READ AGAIN (the refresh, `refresh_host`):
//   · every ten minutes, on the scheduler (`git-host-holdings`, leased, one
//     instance per interval)
//   · after GitHub refuses a mint with a 422 (the record may be out of date;
//     at most once a minute per host and process)
//   · when an admin adds or changes a host (the registry's writes)
//   · when an admin presses Refresh on the host's card
//
// WHAT A REFRESH DOES WITH WHAT IT LEARNS. Each installation the connection
// uses gets its row: the App's ask, what the installation holds, its
// repository selection, whether it is suspended, its settings page, and when
// it was read. A read that fails keeps the last good answer and says why in
// `error`. Each GitHub read is bounded, and of two refreshes that overlap the
// record keeps the read that began later, whichever lands first (`record`).
// A row for an installation no longer selected goes. A permission
// an installation no longer holds revokes every token this process keeps
// that carries it, and writes an audit row. A permission newly accepted that
// an admin had already given an agent on the host is effective from that
// agent's next token: each such agent gets a
// `workbench.github_permission_activated` row (crate::permissions).
//
// WHO READS IT, AND HOW.
//   · The mint (talaria-github) reads `held` through [`TableHoldings`] before
//     it asks GitHub, kept for a minute per process. A row the refresh could
//     not read, or one older than half an hour, is no answer, and the mint
//     asks GitHub as it always did.
//   · The alerts (talaria-alerts) read the rows and never GitHub: what is
//     waiting for an org owner, and what is unknown.
//   · Admin → Git hosts reads them with the host's live status.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_audit::{AuditEntry, log_audit};
use talaria_git_host_types::{GitHost, HostKind, HostStatus, InstallationStatus, PermissionSet};
use talaria_github::{Github, HeldFuture, HoldingsStore, InstallationSnapshot};
use talaria_secretbox::SecretBox;

use crate::registry::{HostRow, get_host, get_host_on, host_rows};
use crate::{Host, permissions};

/// How often the scheduler reads every host's App and installations again.
pub const REFRESH_EVERY_MS: u64 = 10 * 60_000;

/// A row whose last good read is older than this is not trusted: the mint
/// asks GitHub instead, and the alert calls the installation unknown. Three
/// refreshes in a row have to miss for it.
pub const STALE_AFTER_MS: i64 = 30 * 60_000;

/// At most one refresh a minute per host, per process, for refused mints: a
/// run of jobs on one repository GitHub refuses is one refresh, not one each.
const REFUSED_REFRESH_GAP_MS: i64 = 60_000;

// ── The rows ─────────────────────────────────────────────────────────────────

/// One installation's row: what the refresh last learned about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallationRow {
    pub host_id: String,
    /// As the host's config spells it.
    pub installation_id: String,
    pub account: String,
    /// What the App's registration asked for at the last good read.
    pub app_permissions: Option<PermissionSet>,
    /// What the installation held then. None: never read.
    pub held: Option<PermissionSet>,
    pub repository_selection: Option<String>,
    /// ISO.
    pub suspended_at: Option<String>,
    /// The installation's settings page on GitHub.
    pub html_url: Option<String>,
    /// When the last good read was, epoch ms.
    pub checked_at_ms: Option<i64>,
    /// Why the last read failed, when it did.
    pub error: Option<String>,
}

impl InstallationRow {
    /// What the App asks for that the installation has not accepted, both
    /// from the same read: each key it lacks, or holds below the asked
    /// level. Empty when either is unknown — "unknown" is the alert's other
    /// line, never a pending change.
    pub fn pending(&self) -> PermissionSet {
        match (&self.app_permissions, &self.held) {
            (Some(app), Some(held)) => app.missing_from(held),
            _ => PermissionSet::new(),
        }
    }

    /// Whether the mint may size a token to `held`: it was read, the last
    /// read did not fail, and it is not older than [`STALE_AFTER_MS`].
    pub fn trusted_held(&self, now_ms: i64) -> Option<&PermissionSet> {
        let fresh = self
            .checked_at_ms
            .is_some_and(|at| now_ms - at <= STALE_AFTER_MS);
        if fresh && self.error.is_none() {
            self.held.as_ref()
        } else {
            None
        }
    }

    /// The status wire's entry for this installation.
    pub fn status(&self) -> InstallationStatus {
        InstallationStatus {
            id: self.installation_id.clone(),
            account: self.account.clone(),
            app_permissions: self.app_permissions.clone(),
            held: self.held.clone(),
            pending: self.pending(),
            repository_selection: self.repository_selection.clone(),
            suspended_at: self.suspended_at.clone(),
            checked_at: self.checked_at_ms.map(epoch_ms_to_iso),
            error: self.error.clone(),
            url: self.html_url.clone(),
        }
    }
}

type Row = (
    String,
    i64,
    String,
    Option<Value>,
    Option<Value>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

const SELECT: &str = "select host_id, installation_id, account, app_permissions, held, \
       repository_selection, \
       (trunc(extract(epoch from suspended_at) * 1000))::bigint, \
       html_url, \
       (trunc(extract(epoch from checked_at) * 1000))::bigint, \
       error \
     from git_host_installations";

/// A stored permission set, or None when it is absent or cannot be read
/// (unknown, never "nothing").
fn stored_set(v: Option<Value>) -> Option<PermissionSet> {
    v.filter(|v| !v.is_null())
        .and_then(|v| PermissionSet::from_json(&v).ok())
}

fn row_of(r: Row) -> InstallationRow {
    InstallationRow {
        host_id: r.0,
        installation_id: r.1.to_string(),
        account: r.2,
        app_permissions: stored_set(r.3),
        held: stored_set(r.4),
        repository_selection: r.5,
        suspended_at: r.6.map(epoch_ms_to_iso),
        html_url: r.7,
        checked_at_ms: r.8,
        error: r.9,
    }
}

/// Every row of host `host_id`, by installation id.
pub async fn installation_rows(pg: &PgPool, host_id: &str) -> Result<Vec<InstallationRow>, String> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where host_id = $1 order by installation_id"
    )))
    .bind(host_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("git host installations read: {e}"))?;
    Ok(rows.into_iter().map(row_of).collect())
}

/// Every row of every host.
pub async fn all_installation_rows(pg: &PgPool) -> Result<Vec<InstallationRow>, String> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} order by host_id, installation_id"
    )))
    .fetch_all(pg)
    .await
    .map_err(|e| format!("git host installations read: {e}"))?;
    Ok(rows.into_iter().map(row_of).collect())
}

/// The installations a host's record follows: a GitHub host connected as an
/// App with its ID, key and at least one installation selected — the
/// selected installations, in the config's order. Nothing for any other
/// host: a token or a Forgejo secret has no installation to accept anything.
pub fn tracked_installations(row: &HostRow) -> Vec<String> {
    match row.github_config() {
        Some(cfg)
            if cfg.mode.as_deref() == Some("app")
                && !cfg.app_id.is_empty()
                && cfg.private_key_enc.is_some() =>
        {
            cfg.installation_ids
        }
        _ => Vec::new(),
    }
}

/// The status entries for `selected`, in that order, from `rows`. A selected
/// installation the record has no row for is listed with nothing known
/// (never checked), so a page shows it rather than leaving it out.
pub fn installation_statuses(
    selected: &[String],
    rows: &[InstallationRow],
) -> Vec<InstallationStatus> {
    selected
        .iter()
        .map(|id| {
            rows.iter()
                .find(|r| &r.installation_id == id)
                .map(InstallationRow::status)
                .unwrap_or_else(|| InstallationStatus {
                    id: id.clone(),
                    ..InstallationStatus::default()
                })
        })
        .collect()
}

/// A host's live status (who it acts as, what its App asks for) with each
/// installation its connection uses as the record has it. The installations
/// are never asked of GitHub here: that is the refresh's job.
pub async fn host_status(pg: &PgPool, host: &Host, row: &HostRow) -> HostStatus {
    let mut st = host.status().await;
    let selected = tracked_installations(row);
    if selected.is_empty() {
        return st;
    }
    let rows = installation_rows(pg, &row.id).await.unwrap_or_else(|e| {
        tracing::warn!("[git-hosts] {e}");
        Vec::new()
    });
    st.installations = installation_statuses(&selected, &rows);
    st
}

/// One GitHub App host as the record has it, for the alerts: the host, the
/// installations its connection uses, and their rows.
#[derive(Debug, Clone)]
pub struct HostHoldings {
    pub host: HostRow,
    pub selected: Vec<String>,
    pub rows: Vec<InstallationRow>,
}

/// Every host the record follows, with its rows. The database only, never
/// GitHub: this is what the alerts read on every page load.
pub async fn recorded_holdings(pg: &PgPool) -> Result<Vec<HostHoldings>, String> {
    let hosts = host_rows(pg).await?;
    let mut rows = all_installation_rows(pg).await?;
    Ok(hosts
        .into_iter()
        .filter_map(|host| {
            let selected = tracked_installations(&host);
            if selected.is_empty() {
                return None;
            }
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut rows)
                .into_iter()
                .partition(|r| r.host_id == host.id && selected.contains(&r.installation_id));
            rows = rest;
            Some(HostHoldings {
                host,
                selected,
                rows: mine,
            })
        })
        .collect())
}

// ── The refresh ──────────────────────────────────────────────────────────────

/// What one refresh learned and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Installations read.
    pub checked: usize,
    /// Installations whose read failed (the App's, or their own).
    pub failed: usize,
    /// Installations with something waiting for an owner's approval.
    pub pending: usize,
    /// Per installation: what it held at the last read and no longer does.
    pub lost: Vec<(String, PermissionSet)>,
    /// Per installation: what it holds now and did not at the last read.
    pub gained: Vec<(String, PermissionSet)>,
    /// Tokens revoked because they carried a permission no longer held.
    pub revoked: usize,
    /// Tokens whose revoke GitHub did not confirm.
    pub revoke_failed: usize,
    /// Agents whose permissions on the host an installation's change made
    /// effective (one per agent and installation).
    pub activated: usize,
}

impl Refreshed {
    fn add(&mut self, other: Refreshed) {
        self.checked += other.checked;
        self.failed += other.failed;
        self.pending += other.pending;
        self.lost.extend(other.lost);
        self.gained.extend(other.gained);
        self.revoked += other.revoked;
        self.revoke_failed += other.revoke_failed;
        self.activated += other.activated;
    }
}

/// What changed for one installation between two reads: what it holds now
/// that it did not (gained), and what it held that it no longer does
/// (lost). Nothing when there was no earlier read to compare with.
pub fn held_change(
    before: Option<&PermissionSet>,
    now: &PermissionSet,
) -> (PermissionSet, PermissionSet) {
    match before {
        Some(before) => (now.missing_from(before), before.missing_from(now)),
        None => (PermissionSet::new(), PermissionSet::new()),
    }
}

/// How long one GitHub read of a refresh may take. The shared client bounds
/// only the connect, and a GitHub that accepts the connection and then
/// stalls must not hold the host's refresh for ever: past this the read is
/// recorded as failed, and the next pass reads again.
const GITHUB_READ_TIMEOUT: Duration = Duration::from_secs(20);

/// How long an admin's write to a host waits for the refresh it starts
/// before answering. The refresh goes on past it and records what it learns;
/// the card shows that on its next read.
const AFTER_WRITE_WAIT: Duration = Duration::from_secs(30);

/// One refresh of a host at a time in this process: a refused mint, the
/// scheduler and an admin's button can all ask at once, and reading GitHub
/// twice over for one answer is waste. Keyed per host, so a host whose
/// GitHub is slow to answer never holds up another, and a host with nothing
/// to read never waits at all. Which read the record keeps when two overlap
/// is not this lock's job: that is [`record`]'s guard, across processes.
static REFRESHING: LazyLock<HostLocks> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Each host's refresh lock, by host id.
type HostLocks = Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;

/// The lock [`REFRESHING`] keeps for host `host_id`.
fn host_lock(host_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    REFRESHING
        .lock()
        .unwrap()
        .entry(host_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// `read`, or the sentence a row keeps when GitHub did not answer within
/// `limit`.
async fn within<T>(
    limit: Duration,
    read: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(limit, read).await.unwrap_or_else(|_| {
        Err(format!(
            "GitHub did not answer within {} s",
            limit.as_secs()
        ))
    })
}

/// The database's clock, epoch ms. A read is stamped with it when it
/// begins, so reads made by different api processes compare on one clock.
async fn db_now_ms(pg: &PgPool) -> Result<i64, String> {
    sqlx::query_scalar::<_, i64>(
        "select (trunc(extract(epoch from clock_timestamp()) * 1000))::bigint",
    )
    .fetch_one(pg)
    .await
    .map_err(|e| format!("git host installations read: {e}"))
}

/// The sentence a row keeps for an installation GitHub could not be asked
/// about.
fn read_failure(installation_id: &str, e: &str) -> String {
    let why = if e.contains("→ 404") {
        format!(
            "GitHub has no installation {installation_id} of this App (it was uninstalled, or belongs to another App) — pick the installation again in Admin → Git hosts"
        )
    } else {
        e.to_string()
    };
    clip(&why)
}

/// A sentence a row keeps, cut to 300 characters: GitHub's error bodies can
/// run long, and the row is read on every alert.
fn clip(s: &str) -> String {
    s.chars().take(300).collect()
}

/// One installation as a refresh read it.
struct Read {
    /// As the host's config spells it.
    id: String,
    n: i64,
    answer: Result<InstallationSnapshot, String>,
}

/// What [`record`] did with one read.
#[derive(Debug, PartialEq, Eq)]
enum Fate {
    /// The row now says what the read said. `before` is what the row held
    /// until then (None: never read).
    Recorded { before: Option<PermissionSet> },
    /// The row already holds a good read that began after this one did:
    /// this one says nothing about now, and is not acted on.
    Superseded,
}

/// Write what one refresh read, and say what each read replaced.
///
/// Every writer takes the host's advisory lock first, so the row each read
/// is measured against is the row it replaces. The reads themselves were
/// made before the lock, though, and two refreshes (two api processes, or a
/// write and a refused mint) can finish in either order. So:
///   · The host is read again here, under the lock. A read of an
///     installation it no longer uses is dropped, and so is every row of
///     one: an admin may have changed the host while GitHub was asked.
///   · A good read stamps the row with when the read began (`started_ms`),
///     and replaces only a row no good read since then has written. A
///     failed read records its error on the same terms. An older read that
///     lands second is [`Fate::Superseded`].
async fn record(
    pg: &PgPool,
    host_id: &str,
    started_ms: i64,
    app: Option<&PermissionSet>,
    reads: Vec<Read>,
) -> Result<Vec<(Read, Fate)>, String> {
    let write = |e: sqlx::Error| format!("git host installations write: {e}");
    let mut tx = pg.begin().await.map_err(write)?;
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("git_host_installations:{host_id}"))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("git host installations lock: {e}"))?;
    // None when the host is gone: its rows went with it.
    let selected: Vec<i64> = get_host_on(&mut tx, host_id)
        .await?
        .map(|row| {
            tracked_installations(&row)
                .iter()
                .filter_map(|id| id.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let before: HashMap<i64, Option<PermissionSet>> = sqlx::query_as::<_, (i64, Option<Value>)>(
        "select installation_id, held from git_host_installations where host_id = $1",
    )
    .bind(host_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| format!("git host installations read: {e}"))?
    .into_iter()
    .map(|(n, held)| (n, stored_set(held)))
    .collect();
    sqlx::query(
        "delete from git_host_installations where host_id = $1 and installation_id <> all($2::bigint[])",
    )
    .bind(host_id)
    .bind(&selected)
    .execute(&mut *tx)
    .await
    .map_err(write)?;
    let at = epoch_ms_to_iso(started_ms);
    let app_json = app.map(PermissionSet::to_json);
    let mut out = Vec::new();
    for read in reads {
        if !selected.contains(&read.n) {
            continue;
        }
        let written: Option<i64> = match &read.answer {
            Ok(s) => {
                sqlx::query_scalar::<_, i64>(
                    "insert into git_host_installations \
                       (host_id, installation_id, account, app_permissions, held, \
                        repository_selection, suspended_at, html_url, checked_at, error) \
                     values ($1, $2, $3, $4, $5, $6, $7::timestamptz, $8, $9::timestamptz, null) \
                     on conflict (host_id, installation_id) do update set \
                       account = excluded.account, app_permissions = excluded.app_permissions, \
                       held = excluded.held, repository_selection = excluded.repository_selection, \
                       suspended_at = excluded.suspended_at, html_url = excluded.html_url, \
                       checked_at = excluded.checked_at, error = null \
                     where git_host_installations.checked_at is null \
                        or git_host_installations.checked_at < excluded.checked_at \
                     returning installation_id",
                )
                .bind(host_id)
                .bind(read.n)
                .bind(&s.account)
                .bind(&app_json)
                .bind(s.permissions.to_json())
                .bind(&s.repository_selection)
                .bind(&s.suspended_at)
                .bind(&s.html_url)
                .bind(&at)
                .fetch_optional(&mut *tx)
                .await
            }
            // The last good read stays; the row says why this one failed.
            Err(why) => {
                sqlx::query_scalar::<_, i64>(
                    "insert into git_host_installations (host_id, installation_id, error) \
                     values ($1, $2, $3) \
                     on conflict (host_id, installation_id) do update set error = excluded.error \
                     where git_host_installations.checked_at is null \
                        or git_host_installations.checked_at < $4::timestamptz \
                     returning installation_id",
                )
                .bind(host_id)
                .bind(read.n)
                .bind(why)
                .bind(&at)
                .fetch_optional(&mut *tx)
                .await
            }
        }
        .map_err(write)?;
        let fate = match written {
            Some(_) => Fate::Recorded {
                before: before.get(&read.n).cloned().flatten(),
            },
            None => Fate::Superseded,
        };
        out.push((read, fate));
    }
    tx.commit().await.map_err(write)?;
    Ok(out)
}

/// Read host `host_id`'s App and each installation its connection uses
/// again, record what they say, and act on what changed (see the header).
/// Err is the database failing; GitHub failing is recorded on the rows, and
/// is an answer.
pub async fn refresh_host(pg: &PgPool, sb: &SecretBox, host_id: &str) -> Result<Refreshed, String> {
    let Some(row) = get_host(pg, host_id).await? else {
        return Ok(Refreshed::default());
    };
    let tracked: Vec<(String, i64)> = tracked_installations(&row)
        .into_iter()
        .filter_map(|id| match id.parse::<i64>() {
            Ok(n) => Some((id, n)),
            Err(_) => {
                tracing::warn!(
                    "[git-hosts] host {host_id} selects \"{id}\", which is not an installation id — left out of its record"
                );
                None
            }
        })
        .collect();
    if tracked.is_empty() {
        // Nothing to follow (not GitHub, a token, or an App not finished):
        // a row left from before would be read as this host's truth. No
        // GitHub to ask, so no other refresh of the host to wait for.
        record(pg, host_id, 0, None, Vec::new()).await?;
        return Ok(Refreshed::default());
    }

    let single = tracked.len() == 1;
    let lock = host_lock(host_id);
    let _one = lock.lock().await;
    let started_ms = db_now_ms(pg).await?;
    // GitHub first, outside any transaction, each read bounded. A failed App
    // read skips the installations: the same App JWT asks for both.
    let gh = Github::new(
        &row.id,
        &row.base_url,
        row.api_url.as_deref(),
        &row.auth,
        sb.clone(),
    );
    let app = within(GITHUB_READ_TIMEOUT, gh.app_permissions()).await;
    let mut reads: Vec<Read> = Vec::new();
    for (id, n) in tracked {
        let answer = match &app {
            Ok(_) => within(GITHUB_READ_TIMEOUT, gh.installation(&id))
                .await
                .map_err(|e| read_failure(&id, &e)),
            Err(e) => Err(clip(&format!("the App itself could not be read: {e}"))),
        };
        reads.push(Read { id, n, answer });
    }
    let recorded = record(pg, host_id, started_ms, app.as_ref().ok(), reads).await?;

    // What changed, acted on after the record is written.
    let mut out = Refreshed::default();
    let mut accepted: Vec<Accepted> = Vec::new();
    for (Read { id, answer, .. }, fate) in recorded {
        let snapshot = match answer {
            Ok(s) => s,
            Err(why) => {
                out.failed += 1;
                tracing::warn!(
                    "[git-hosts] installation {id} of host {host_id} could not be read: {why}"
                );
                continue;
            }
        };
        let Fate::Recorded { before } = fate else {
            // A read that began later is the row: what changed is its to say,
            // and a revoke on this older answer could be wrong.
            tracing::info!(
                "[git-hosts] installation {id} of host {host_id} was read again while this read was out; the newer read stands"
            );
            continue;
        };
        out.checked += 1;
        if let Ok(app) = &app
            && !app.missing_from(&snapshot.permissions).is_empty()
        {
            out.pending += 1;
        }
        let (gained, lost) = held_change(before.as_ref(), &snapshot.permissions);
        // Every token this process keeps for the installation is measured
        // against what it holds now, not only against the last read: a row
        // never read before has nothing to diff, and a token still carries
        // whatever it was minted with.
        let revocation = gh.revoke_tokens_beyond(&id, &snapshot.permissions).await;
        let lost = lost.union(&revocation.keys);
        if !gained.is_empty() {
            tracing::info!(
                "[git-hosts] installation {id} ({}) of host {host_id} now holds {gained}",
                snapshot.account
            );
            out.gained.push((id.clone(), gained));
            if let Some(before) = &before {
                accepted.push(Accepted {
                    installation: id.clone(),
                    account: snapshot.account.clone(),
                    before: before.clone(),
                    now: snapshot.permissions.clone(),
                });
            }
        }
        if !lost.is_empty() {
            log_audit(
                pg,
                AuditEntry {
                    actor: "talaria",
                    action: "github.installation_permission_lost",
                    target_type: "git_host",
                    target_id: Some(host_id),
                    target_label: Some(snapshot.account.as_str()),
                    before: None,
                    after: Some(json!({
                        "installation": id,
                        "account": snapshot.account,
                        "lost": lost.to_json(),
                        "held": snapshot.permissions.to_json(),
                        "tokensRevoked": revocation.revoked,
                        "revokeFailed": revocation.failed,
                    })),
                },
            )
            .await;
        }
        if revocation.failed > 0 {
            tracing::error!(
                "[git-hosts] ALERT: {} token(s) on installation {id} of host {host_id} carry {} the installation no longer holds, and GitHub did not confirm their revoke — they live until they expire, within the hour",
                revocation.failed,
                revocation.keys
            );
            log_audit(
                pg,
                AuditEntry {
                    actor: "talaria",
                    action: "github.token_revoke_failed",
                    target_type: "git_host",
                    target_id: Some(host_id),
                    target_label: Some(snapshot.account.as_str()),
                    before: None,
                    after: Some(json!({
                        "installation": id,
                        "keys": revocation.keys.to_json(),
                        "failed": revocation.failed,
                    })),
                },
            )
            .await;
        }
        if !lost.is_empty() {
            out.lost.push((id, lost));
        }
        out.revoked += revocation.revoked;
        out.revoke_failed += revocation.failed;
    }
    if !accepted.is_empty() {
        out.activated = record_activations(pg, host_id, single, &accepted).await;
    }
    Ok(out)
}

/// One installation's change that gave it something it did not hold.
struct Accepted {
    installation: String,
    account: String,
    before: PermissionSet,
    now: PermissionSet,
}

/// Every agent whose permissions on `host_id` an installation's change just
/// made effective, recorded: one `workbench.github_permission_activated` row
/// per agent and installation, naming what became effective and the repos
/// whose tokens carry it from the next mint. An admin gave it ahead of
/// GitHub, and an org owner's acceptance on GitHub is a decision of its own:
/// the row is how the two meet in the trail. `single`: the host uses one
/// installation, which mints every repo's token; with several, a repo's is
/// the one on its owner's account. Answers how many rows were written.
async fn record_activations(
    pg: &PgPool,
    host_id: &str,
    single: bool,
    accepted: &[Accepted],
) -> usize {
    let extras = match permissions::all_extras(pg).await {
        Ok(extras) => extras,
        Err(e) => {
            tracing::warn!(
                "[git-hosts] host {host_id} accepted new permissions, and the agents holding them could not be read: {e}"
            );
            return 0;
        }
    };
    let mut written = 0;
    for e in extras.iter().filter(|e| e.host_id == host_id) {
        for a in accepted {
            let repos: Vec<&String> = e
                .repos
                .iter()
                .filter(|r| single || permissions::on_account(r, &a.account))
                .collect();
            let now = permissions::activated(&e.permissions, &a.before, &a.now);
            if repos.is_empty() || now.is_empty() {
                continue;
            }
            tracing::info!(
                "[git-hosts] {} now carries {now} on {} ({host_id}): installation {} accepted it",
                e.agent_name,
                repos
                    .iter()
                    .map(|r| r.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                a.installation
            );
            log_audit(
                pg,
                AuditEntry {
                    actor: "talaria",
                    action: "workbench.github_permission_activated",
                    target_type: "agent",
                    target_id: Some(&e.agent_id),
                    target_label: Some(&e.agent_name),
                    before: None,
                    after: Some(json!({
                        "host": host_id,
                        "installation": a.installation,
                        "account": a.account,
                        "activated": now.to_json(),
                        "repos": repos,
                    })),
                },
            )
            .await;
            written += 1;
        }
    }
    written
}

/// [`refresh_host`] after an admin's write to the host: the write stands
/// whatever the refresh answers, and a failure is logged. The write's answer
/// waits for it at most [`AFTER_WRITE_WAIT`]: a slow GitHub must not hold an
/// admin's save. The refresh runs on its own task, so it is never cut off
/// part-way (a revoke half done, a transaction rolled back); past the wait
/// it finishes in the background and records what it learns.
pub async fn refresh_after_write(pg: &PgPool, sb: &SecretBox, host_id: &str) {
    let (pg, sb, id) = (pg.clone(), sb.clone(), host_id.to_string());
    let refresh = tokio::spawn(async move { refresh_host(&pg, &sb, &id).await });
    match tokio::time::timeout(AFTER_WRITE_WAIT, refresh).await {
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(e))) => tracing::warn!(
            "[git-hosts] host {host_id} changed, and its record could not be refreshed: {e}"
        ),
        Ok(Err(e)) => tracing::warn!(
            "[git-hosts] host {host_id} changed, and the refresh of its record stopped: {e}"
        ),
        Err(_) => tracing::info!(
            "[git-hosts] host {host_id} changed; GitHub is still being read after {} s, so the write is answered without it and the record lands when the read ends",
            AFTER_WRITE_WAIT.as_secs()
        ),
    }
}

/// Every host, refreshed: the scheduler's run. The line is what the log and
/// the job's status show; Err is a host whose record could not be written.
pub async fn refresh_all(pg: &PgPool, sb: &SecretBox) -> Result<Option<String>, String> {
    let mut total = Refreshed::default();
    let mut failures: Vec<String> = Vec::new();
    for row in host_rows(pg).await? {
        if row.kind != HostKind::Github {
            continue;
        }
        match refresh_host(pg, sb, &row.id).await {
            Ok(r) => total.add(r),
            Err(e) => failures.push(format!("{}: {e}", row.id)),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok((total.checked + total.failed > 0).then(|| {
        format!(
            "{} installation(s) checked, {} unreadable, {} waiting on an owner's approval, {} token(s) revoked",
            total.checked, total.failed, total.pending, total.revoked
        )
    }))
}

// ── The record the mint reads first ──────────────────────────────────────────

/// What `held` the record has for one installation that a mint may trust
/// ([`InstallationRow::trusted_held`]). None for anything else, including a
/// database that did not answer: the mint then asks GitHub.
pub async fn recorded_held(
    pg: &PgPool,
    host_id: &str,
    installation_id: &str,
) -> Option<PermissionSet> {
    let n: i64 = installation_id.parse().ok()?;
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where host_id = $1 and installation_id = $2"
    )))
    .bind(host_id)
    .bind(n)
    .fetch_optional(pg)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("[git-hosts] git host installations read: {e}");
        None
    });
    row.map(row_of)
        .and_then(|r| r.trusted_held(now_ms()).cloned())
}

/// The record, as a connection's [`HoldingsStore`].
pub(crate) struct TableHoldings {
    pg: PgPool,
    sb: SecretBox,
}

/// When each host last had a refused mint refresh its record, in this
/// process.
static REFUSED_AT: LazyLock<Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl HoldingsStore for TableHoldings {
    fn held<'a>(&'a self, host_id: &'a str, installation_id: &'a str) -> HeldFuture<'a> {
        Box::pin(recorded_held(&self.pg, host_id, installation_id))
    }

    fn refused(&self, host_id: &str, _installation_id: &str) {
        let now = now_ms();
        {
            let mut at = REFUSED_AT.lock().unwrap();
            if at
                .get(host_id)
                .is_some_and(|last| now - last < REFUSED_REFRESH_GAP_MS)
            {
                return;
            }
            at.insert(host_id.to_string(), now);
        }
        let (pg, sb, host) = (self.pg.clone(), self.sb.clone(), host_id.to_string());
        tokio::spawn(async move {
            if let Err(e) = refresh_host(&pg, &sb, &host).await {
                tracing::warn!(
                    "[git-hosts] host {host}'s record could not be refreshed after a refused mint: {e}"
                );
            }
        });
    }
}

/// The store a connection to a registered host reads its holdings from.
pub(crate) fn store(pg: &PgPool, sb: &SecretBox) -> Arc<dyn HoldingsStore> {
    Arc::new(TableHoldings {
        pg: pg.clone(),
        sb: sb.clone(),
    })
}

// ── The job ──────────────────────────────────────────────────────────────────

/// The process's current secretbox, read on each run: a key rotation
/// installs a new one, and a box taken at boot could not open what the
/// rotation re-sealed.
pub type SecretBoxSource =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<SecretBox, String>> + Send>> + Send + Sync>;

/// Every ten minutes, leased (one instance per interval), first run ninety
/// seconds after boot so a restart does not wait ten minutes to learn what
/// an org owner changed.
pub fn job_spec(pg: PgPool, sb: SecretBoxSource) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::GitHostHoldings,
        every_ms: REFRESH_EVERY_MS,
        first_run_delay_ms: Some(90_000),
        max_run_ms: Some(5 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let (pg, sb) = (pg.clone(), sb.clone());
            Box::pin(async move {
                let sb = sb().await?;
                refresh_all(&pg, &sb).await
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_host_types::Level;

    fn set(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    fn row(app: Option<PermissionSet>, held: Option<PermissionSet>) -> InstallationRow {
        InstallationRow {
            host_id: "github".into(),
            installation_id: "91".into(),
            account: "acme".into(),
            app_permissions: app,
            held,
            checked_at_ms: Some(1_000_000),
            ..InstallationRow::default()
        }
    }

    #[test]
    fn pending_is_the_apps_ask_minus_what_the_installation_accepted() {
        let app = set(&[
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
            ("workflows", Level::Write),
            ("checks", Level::Read),
        ]);
        // It took pull_requests at read only, and never took workflows.
        let held = set(&[
            ("checks", Level::Read),
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Read),
        ]);
        assert_eq!(
            row(Some(app.clone()), Some(held.clone())).pending(),
            set(&[("pull_requests", Level::Write), ("workflows", Level::Write)])
        );
        assert!(
            row(Some(app.clone()), Some(app.clone()))
                .pending()
                .is_empty()
        );
        // Either side unknown is not a pending change: it is unknown.
        assert!(row(None, Some(held.clone())).pending().is_empty());
        assert!(row(Some(app), None).pending().is_empty());
    }

    #[test]
    fn the_mint_trusts_only_a_good_recent_read() {
        let held = set(&[("contents", Level::Write)]);
        let at = 1_000_000;
        let good = row(None, Some(held.clone()));
        assert_eq!(good.trusted_held(at + 60_000), Some(&held));
        assert_eq!(good.trusted_held(at + STALE_AFTER_MS + 1), None, "too old");
        let failed = InstallationRow {
            error: Some("GitHub GET /app/installations/91 → 502".into()),
            ..good.clone()
        };
        assert_eq!(
            failed.trusted_held(at + 60_000),
            None,
            "the last read failed"
        );
        let never = InstallationRow {
            checked_at_ms: None,
            held: None,
            ..good
        };
        assert_eq!(never.trusted_held(at), None);
    }

    #[test]
    fn a_change_between_two_reads_is_what_was_gained_and_what_was_lost() {
        let before = set(&[("contents", Level::Write), ("pull_requests", Level::Write)]);
        let now = set(&[
            ("contents", Level::Read),
            ("pull_requests", Level::Write),
            ("workflows", Level::Write),
        ]);
        let (gained, lost) = held_change(Some(&before), &now);
        assert_eq!(gained, set(&[("workflows", Level::Write)]));
        assert_eq!(
            lost,
            set(&[("contents", Level::Write)]),
            "a lowered level is lost"
        );
        let (gained, lost) = held_change(None, &now);
        assert!(
            gained.is_empty() && lost.is_empty(),
            "nothing to compare with"
        );
    }

    fn host(auth: Value) -> HostRow {
        HostRow {
            id: "github".into(),
            kind: HostKind::Github,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            api_url: None,
            auth,
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn only_a_finished_app_connection_has_installations_to_follow() {
        let app = |ids: &[&str], key: bool| {
            json!({
                "mode": "app",
                "app": { "appId": "1", "installationIds": ids, "privateKeyEnc": key.then_some("sealed") },
            })
        };
        assert_eq!(
            tracked_installations(&host(app(&["91", "92"], true))),
            ["91", "92"]
        );
        assert!(
            tracked_installations(&host(app(&["91"], false))).is_empty(),
            "no key"
        );
        assert!(tracked_installations(&host(app(&[], true))).is_empty());
        assert!(
            tracked_installations(&host(json!({ "mode": "pat", "pat": { "tokenEnc": "x" } })))
                .is_empty()
        );
        let mut forge = host(json!({ "secret": "bot" }));
        forge.kind = HostKind::Forgejo;
        assert!(tracked_installations(&forge).is_empty());
    }

    /// The installations the host card reads: in the config's order, each
    /// with its own pending set, and a selected installation the record
    /// never read listed as unknown rather than left out.
    #[test]
    fn the_wire_lists_every_selected_installation_with_its_own_pending_set() {
        let app = set(&[("contents", Level::Write), ("workflows", Level::Write)]);
        let rows = vec![InstallationRow {
            installation_id: "92".into(),
            account: "beta".into(),
            app_permissions: Some(app.clone()),
            held: Some(set(&[("contents", Level::Write)])),
            html_url: Some(
                "https://github.com/organizations/beta/settings/installations/92".into(),
            ),
            checked_at_ms: Some(0),
            ..InstallationRow::default()
        }];
        let wire = serde_json::to_value(installation_statuses(&["91".into(), "92".into()], &rows))
            .unwrap();
        assert_eq!(wire[0]["id"], "91");
        assert_eq!(wire[0]["held"], Value::Null, "never checked is unknown");
        assert_eq!(wire[0]["checkedAt"], Value::Null);
        assert_eq!(wire[0]["pending"], json!({}));
        assert_eq!(wire[1]["account"], "beta");
        assert_eq!(wire[1]["held"], json!({ "contents": "write" }));
        assert_eq!(wire[1]["pending"], json!({ "workflows": "write" }));
        assert_eq!(wire[1]["checkedAt"], "1970-01-01T00:00:00.000Z");
        assert_eq!(
            wire[1]["url"],
            "https://github.com/organizations/beta/settings/installations/92"
        );
    }

    #[test]
    fn a_missing_installation_says_so_and_how_to_fix_it() {
        let why = read_failure(
            "91",
            "GitHub GET /app/installations/91 → 404: {\"message\":\"Not Found\"}",
        );
        assert!(
            why.contains("no installation 91") && why.contains("Admin → Git hosts"),
            "{why}"
        );
        let other = read_failure("91", "github request: connection refused");
        assert_eq!(other, "github request: connection refused");
    }

    /// A GitHub that takes the connection and never answers is a failed
    /// read once its bound passes, never a refresh that waits for ever.
    #[tokio::test]
    async fn a_read_github_never_answers_fails_at_its_bound() {
        let stalled = within(
            Duration::from_millis(20),
            std::future::pending::<Result<(), String>>(),
        )
        .await;
        assert!(
            stalled
                .as_ref()
                .is_err_and(|e| e.contains("did not answer")),
            "{stalled:?}"
        );
        assert_eq!(
            within(Duration::from_secs(5), async { Ok::<_, String>(7) }).await,
            Ok(7)
        );
    }

    /// A refresh holds its own host's lock only: one host GitHub is slow
    /// to answer never holds up another.
    #[tokio::test]
    async fn each_host_refreshes_under_its_own_lock() {
        let a = host_lock("lock-test-a");
        let _held = a.lock().await;
        assert!(
            Arc::ptr_eq(&a, &host_lock("lock-test-a")),
            "one lock per host"
        );
        assert!(
            host_lock("lock-test-a").try_lock().is_err(),
            "the same host waits"
        );
        assert!(
            host_lock("lock-test-b").try_lock().is_ok(),
            "another host does not"
        );
    }
}

/// LIVE: the refresh against a real database and a loopback GitHub — the
/// record it writes, the status wire the host card reads, the mint that reads
/// the record instead of GitHub, the revoke when an installation loses a
/// permission, and the rows a deselected installation leaves. `#[ignore]`d;
/// ci.yml's api job runs it with the ignored lib tests in its live-DB steps.
///
///   DATABASE_URL=postgres://… cargo test -p talaria-git-hosts --lib -- --ignored live
#[cfg(test)]
mod live {
    use std::sync::atomic::{AtomicBool, Ordering};

    use talaria_git_host_types::Level;
    use talaria_github::GithubConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Each request the stub answered: method, path, authorization.
    type Log = Arc<Mutex<Vec<(String, String, String)>>>;

    const APP: &str = r#"{"name":"Talaria Live","permissions":{"contents":"write","pull_requests":"write","workflows":"write","metadata":"read"}}"#;

    /// GitHub, as far as the refresh and one mint need it. Installation
    /// 93001 is acme's and holds contents and pull_requests until `lost` is
    /// set, then contents alone; 93002 is gone (404).
    async fn github(lost: Arc<AtomicBool>) -> (String, Log) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener binds");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (seen, lost) = (seen.clone(), lost.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break (
                                    text[..end].to_string(),
                                    text[end + 4..end + 4 + len].to_string(),
                                );
                            }
                        }
                    };
                    let mut words = head.lines().next().unwrap_or_default().split(' ');
                    let method = words.next().unwrap_or_default().to_string();
                    let path = words.next().unwrap_or_default().to_string();
                    let auth = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("authorization")
                                .then(|| v.trim().to_string())
                        })
                        .unwrap_or_default();
                    seen.lock()
                        .unwrap()
                        .push((method.clone(), path.clone(), auth));
                    let (status, answer) = match (method.as_str(), path.as_str()) {
                        ("GET", "/app") => (200, APP.to_string()),
                        ("GET", "/app/installations/93001") => {
                            let held = if lost.load(Ordering::SeqCst) {
                                json!({ "contents": "write" })
                            } else {
                                json!({ "contents": "write", "pull_requests": "write" })
                            };
                            (
                                200,
                                json!({
                                    "id": 93001,
                                    "account": { "login": "acme" },
                                    "permissions": held,
                                    "repository_selection": "selected",
                                    "suspended_at": null,
                                    "html_url": "https://github.com/organizations/acme/settings/installations/93001",
                                })
                                .to_string(),
                            )
                        }
                        ("GET", "/app/installations/93002") => {
                            (404, r#"{"message":"Not Found"}"#.to_string())
                        }
                        ("GET", "/repos/acme/widgets/installation") => {
                            (200, r#"{"id":93001}"#.to_string())
                        }
                        ("POST", "/app/installations/93001/access_tokens") => {
                            let asked: Value = serde_json::from_str(&body).unwrap_or_default();
                            (
                                201,
                                json!({
                                    "token": "tok-live",
                                    "expires_at": "2099-01-01T00:00:00Z",
                                    "permissions": asked["permissions"],
                                    "repository_selection": "selected",
                                    "repositories": [{ "full_name": "acme/widgets" }],
                                })
                                .to_string(),
                            )
                        }
                        ("DELETE", "/installation/token") => (204, String::new()),
                        _ => (500, r#"{"message":"unexpected"}"#.to_string()),
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (base, log)
    }

    fn count(log: &Log, method: &str, path: &str) -> usize {
        log.lock()
            .unwrap()
            .iter()
            .filter(|(m, p, _)| m == method && p == path)
            .count()
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_refresh_writes_the_record_the_card_the_mint_and_the_revoke_read() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let pg = PgPool::connect(&url).await.expect("connect");
        let lost = Arc::new(AtomicBool::new(false));
        let (base, log) = github(lost.clone()).await;
        let sb = SecretBox::from_parts(
            [7u8; 32],
            HashMap::from([(1u32, talaria_secretbox::new_dek().expect("entropy"))]),
            Some(1),
        );
        let pem = include_str!("../../../tests/it/fixtures/github-jwt-traditional.pem");
        let cfg = GithubConfig {
            mode: Some("app".into()),
            pat_token_enc: None,
            app_id: "12345".into(),
            installation_ids: vec!["93001".into(), "93002".into()],
            private_key_enc: Some(sb.seal(pem).expect("the key seals")),
        };
        let id = format!("ghp-live-{}", now_ms() % 1_000_000_000);
        sqlx::query(
            "insert into git_hosts (id, kind, name, base_url, api_url, auth) \
             values ($1, 'github', 'Live GitHub', $2, $3, $4)",
        )
        .bind(&id)
        .bind(format!("https://{id}.example.invalid"))
        .bind(&base)
        .bind(cfg.to_auth())
        .execute(&pg)
        .await
        .expect("the host row");
        let (w, r) = (Level::Write, Level::Read);

        // The first read: acme's installation is recorded with what waits,
        // and the one GitHub no longer has is recorded as failed.
        let first = refresh_host(&pg, &sb, &id)
            .await
            .expect("the record is written");
        assert_eq!(
            (first.checked, first.failed, first.pending),
            (1, 1, 1),
            "{first:?}"
        );
        let rows = installation_rows(&pg, &id).await.expect("read");
        assert_eq!(rows.len(), 2, "{rows:?}");
        let acme = &rows[0];
        assert_eq!(
            (acme.installation_id.as_str(), acme.account.as_str()),
            ("93001", "acme")
        );
        let held = PermissionSet::of(&[("contents", w), ("metadata", r), ("pull_requests", w)]);
        assert_eq!(acme.held.as_ref(), Some(&held));
        assert_eq!(acme.pending(), PermissionSet::of(&[("workflows", w)]));
        assert!(
            acme.checked_at_ms.is_some() && acme.error.is_none(),
            "{acme:?}"
        );
        assert_eq!(acme.repository_selection.as_deref(), Some("selected"));
        let gone = &rows[1];
        assert!(
            gone.error
                .as_deref()
                .is_some_and(|e| e.contains("no installation 93002")),
            "{gone:?}"
        );
        assert_eq!((gone.held.as_ref(), gone.checked_at_ms), (None, None));
        assert_eq!(recorded_held(&pg, &id, "93001").await, Some(held.clone()));
        assert_eq!(
            recorded_held(&pg, &id, "93002").await,
            None,
            "never read is no answer"
        );

        // Two refreshes that overlap can land in either order, and the
        // record keeps the newer read. One that began before the row's last
        // good read is not written over it, and neither is its failure;
        // neither is acted on. A read of an installation the host does not
        // use is dropped.
        let began = acme.checked_at_ms.expect("read") - 60_000;
        let narrower = InstallationSnapshot {
            id: "93001".into(),
            account: "acme".into(),
            permissions: PermissionSet::of(&[("contents", w), ("metadata", r)]),
            repository_selection: Some("selected".into()),
            suspended_at: None,
            html_url: None,
        };
        let read = |id: &str, answer: Result<InstallationSnapshot, String>| Read {
            id: id.into(),
            n: id.parse().unwrap(),
            answer,
        };
        let fates: Vec<Fate> = record(
            &pg,
            &id,
            began,
            None,
            vec![
                read("93001", Ok(narrower.clone())),
                read("93001", Err("an older failure".into())),
                read("93003", Ok(narrower)),
            ],
        )
        .await
        .expect("the record is written")
        .into_iter()
        .map(|(_, fate)| fate)
        .collect();
        assert_eq!(fates, [Fate::Superseded, Fate::Superseded]);
        assert_eq!(
            installation_rows(&pg, &id).await.expect("read"),
            rows,
            "the newer read stands, and no row is made for 93003"
        );

        // The card's wire: the App's ask live, each installation from the
        // record, in the config's order.
        let row = get_host(&pg, &id).await.expect("read").expect("the row");
        let host = crate::connect(&pg, &sb, &row).await;
        let wire = row.wire_with(&host_status(&pg, &host, &row).await);
        let status = &wire["status"];
        assert_eq!(status["configured"], true, "{wire}");
        assert_eq!(status["appPermissions"]["workflows"], "write");
        let insts = status["installations"].as_array().expect("installations");
        assert_eq!(insts.len(), 2, "{wire}");
        assert_eq!(insts[0]["id"], "93001");
        assert_eq!(insts[0]["held"]["pull_requests"], "write");
        assert_eq!(insts[0]["pending"], json!({ "workflows": "write" }));
        assert_eq!(
            insts[0]["url"],
            "https://github.com/organizations/acme/settings/installations/93001"
        );
        assert!(insts[0]["checkedAt"].is_string(), "{wire}");
        assert_eq!(insts[1]["id"], "93002");
        assert_eq!(insts[1]["held"], Value::Null);
        assert!(insts[1]["error"].is_string(), "{wire}");

        // The mint reads the record, not GitHub: the process's own memory of
        // the refresh's read is forgotten first.
        talaria_github::forget_host(&id);
        let reads = count(&log, "GET", "/app/installations/93001");
        let token = host
            .github()
            .expect("a GitHub host")
            .token("acme/widgets", &held)
            .await
            .expect("minted");
        assert_eq!(token.as_deref(), Some("tok-live"));
        assert_eq!(
            count(&log, "GET", "/app/installations/93001"),
            reads,
            "the mint was sized to the record"
        );

        // An org owner takes pull_requests away: the next read records it,
        // revokes the token that carries it, and writes an audit row.
        lost.store(true, Ordering::SeqCst);
        let second = refresh_host(&pg, &sb, &id)
            .await
            .expect("the record is written");
        assert_eq!(
            second.lost,
            vec![(
                "93001".to_string(),
                PermissionSet::of(&[("pull_requests", w)])
            )]
        );
        assert_eq!((second.revoked, second.revoke_failed), (1, 0), "{second:?}");
        let revoked: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p, _)| m == "DELETE" && p == "/installation/token")
            .map(|(_, _, auth)| auth.clone())
            .collect();
        assert_eq!(revoked, ["Bearer tok-live"]);
        let audited: i64 = sqlx::query_scalar(
            "select count(*) from audit_log \
             where action = 'github.installation_permission_lost' and target_id = $1",
        )
        .bind(&id)
        .fetch_one(&pg)
        .await
        .expect("read");
        assert_eq!(audited, 1);
        let acme = installation_rows(&pg, &id).await.expect("read").remove(0);
        assert_eq!(
            acme.pending(),
            PermissionSet::of(&[("pull_requests", w), ("workflows", w)])
        );

        // The alerts' read sees the host and both installations.
        let followed = recorded_holdings(&pg).await.expect("read");
        let mine = followed
            .iter()
            .find(|h| h.host.id == id)
            .expect("the host is followed");
        assert_eq!(mine.selected, ["93001", "93002"]);
        assert_eq!(mine.rows.len(), 2);

        // The admin stops using 93002: its row goes with the next read.
        let narrowed = GithubConfig {
            installation_ids: vec!["93001".into()],
            ..cfg
        };
        sqlx::query("update git_hosts set auth = $2 where id = $1")
            .bind(&id)
            .bind(narrowed.to_auth())
            .execute(&pg)
            .await
            .expect("the config");
        refresh_host(&pg, &sb, &id)
            .await
            .expect("the record is written");
        let ids: Vec<String> = installation_rows(&pg, &id)
            .await
            .expect("read")
            .into_iter()
            .map(|r| r.installation_id)
            .collect();
        assert_eq!(ids, ["93001"]);

        sqlx::query("delete from audit_log where target_id = $1")
            .bind(&id)
            .execute(&pg)
            .await
            .expect("cleanup");
        sqlx::query("delete from git_hosts where id = $1")
            .bind(&id)
            .execute(&pg)
            .await
            .expect("cleanup");
        assert!(
            installation_rows(&pg, &id).await.expect("read").is_empty(),
            "a removed host's rows go with it"
        );
    }
}
