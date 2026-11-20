// The ack (I-18) and the deferred push (C9, WP-15).
//
// A write while one of the person's clients is looking or active does not
// push: it sets `push_due_at` 60 s out (deliver.rs). A client that SURFACED
// the row to the person — toasted it to someone looking, or posted an OS
// banner — acks it, and `surfaced_at` records the first time. Every 5 s the
// job below pushes the rows whose time came and that nobody surfaced or read
// meanwhile: the person who walked away from the screen still gets the push,
// the person who saw the toast does not get a second copy on their phone.
//
// CLAIM, THEN SEND. A pass takes its rows with `for update skip locked` and
// stamps `pushed_at` in the same statement, so two processes never push one
// row and no row lock is held across a push service's latency (a read of
// that row must not wait on someone's phone). A process that dies between
// the claim and the POST loses that one push — the row itself is still
// unread in the inbox — rather than ever sending it twice.

use sqlx::PgPool;
use std::sync::Arc;

use crate::classes::{class_def, notify_class_of};
use crate::meta::ChannelOverride;
use crate::prefs::{allow_needs_you, channels_with, dnd_state};

/// Record that the caller's client surfaced these rows. Only the caller's own
/// rows are touched (another person's id acks nothing), and the first ack
/// wins. Answers how many rows were the caller's.
pub async fn ack(pg: &PgPool, user_id: &str, ids: &[String]) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query(
        "update notifications set surfaced_at = coalesce(surfaced_at, now()) \
         where user_id = $1::uuid and id = any($2::uuid[])",
    )
    .bind(user_id)
    .bind(ids)
    .execute(pg)
    .await?;
    Ok(res.rows_affected())
}

/// How often the deferred-push pass runs. The deferral is 60 s, so a push
/// lands 60–65 s after its write.
pub const DEFERRED_PUSH_EVERY_MS: u64 = 5_000;
/// Rows one pass pushes at most.
pub const DEFERRED_PUSH_BATCH: i64 = 200;

type DueRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i32,
    i64,
    Option<serde_json::Value>,
    Option<String>,
    Option<bool>,
);

/// Whether a deferred push still goes, at send time: the instance's Push
/// switch, the row's Push channel — the class matrix as it reads NOW, through
/// the override the row was filed with (`push_override`, a person's setting
/// for a board as decided at fan-out; never re-read here) — and DND, unless
/// the class breaks through. Pure.
fn still_pushes(
    blob: &serde_json::Value,
    kind: &str,
    push_override: Option<bool>,
    switch_on: bool,
    zone: &str,
    now: i64,
) -> bool {
    let class = notify_class_of(kind);
    let o = ChannelOverride {
        push: push_override,
        ..Default::default()
    };
    let breaks = class_def(class).is_some_and(|d| d.breaks_through) && allow_needs_you(blob);
    let held = dnd_state(blob, zone, now).active && !breaks;
    switch_on && channels_with(blob, class, &o).push && !held
}

/// One pass: push every row whose deferral is due and that no client
/// surfaced and nobody read. The person's prefs are read again at send time,
/// so a pause begun, or a Push switch turned off, inside the minute is
/// obeyed; such rows leave the push plane (`push_due_at` null). A row filed
/// with a push override (a person's setting for a board) keeps it: the
/// override was decided once, at fan-out, and is not re-read.
pub async fn run_deferred_push(pg: &PgPool) -> Result<Option<String>, String> {
    if !talaria_push::push_plane_installed() {
        return Ok(None);
    }
    let rows: Vec<DueRow> = sqlx::query_as(
        "with due as ( \
           select id from notifications \
            where push_due_at <= now() and pushed_at is null and surfaced_at is null \
              and read_at is null and archived_at is null \
            order by push_due_at limit $1 \
            for update skip locked) \
         update notifications n set pushed_at = now() \
           from due, users u \
          where n.id = due.id and u.id = n.user_id \
         returning n.id::text, n.user_id::text, n.kind, n.title, n.body, n.href, n.count, \
                   (trunc(extract(epoch from n.created_at) * 1000))::bigint, \
                   u.notify_prefs, u.timezone, n.push_override",
    )
    .bind(DEFERRED_PUSH_BATCH)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("the deferred-push claim failed: {e}"))?;
    if rows.is_empty() {
        return Ok(None);
    }
    let switches = crate::mail::delivery_switches(pg).await;
    let workspace_zone = crate::prefs::workspace_zone(pg).await;
    let now = talaria_agent_auth::now_ms();
    let mut pushed = 0usize;
    let mut dropped: Vec<String> = Vec::new();
    for (id, user_id, kind, title, body, href, count, created_ms, blob, zone, push_override) in rows
    {
        let blob = blob.unwrap_or(serde_json::Value::Null);
        let zone = crate::prefs::person_zone(zone.as_deref(), &workspace_zone);
        if !still_pushes(&blob, &kind, push_override, switches.push, &zone, now) {
            dropped.push(id);
            continue;
        }
        talaria_push::push_notification(
            &user_id,
            crate::deliver::push_note(&id, &kind, &title, &body, &href, count > 1, created_ms),
        );
        pushed += 1;
    }
    if !dropped.is_empty()
        && let Err(e) = sqlx::query(
            "update notifications set pushed_at = null, push_due_at = null \
             where id = any($1::uuid[])",
        )
        .bind(&dropped)
        .execute(pg)
        .await
    {
        tracing::error!(
            "[notifications] could not retire {} deferred push(es): {e}",
            dropped.len()
        );
    }
    Ok(Some(match dropped.len() {
        0 => format!("pushed {pushed} deferred notification(s)"),
        n => {
            format!("pushed {pushed} deferred notification(s), dropped {n} (push off or DND since)")
        }
    }))
}

/// The pass as a scheduler job: leased (rows every instance can reach), every
/// 5 s, from 5 s after boot — so a deferral that came due while the api was
/// down fires once on the way back up.
pub fn deferred_push_job_spec(pg: PgPool) -> talaria_scheduler::JobSpec {
    use talaria_scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::NotifyDeferredPush,
        every_ms: DEFERRED_PUSH_EVERY_MS,
        first_run_delay_ms: Some(5_000),
        // A claim and up to 200 detached pushes: the POSTs run on the push
        // plane's own bounded tasks, so the pass itself is a query. Short on
        // purpose: this is also how long a process killed mid-pass holds the
        // lease, i.e. how late the first deferred push after a crash can be
        // (a pass that outlives it is reported hung, and a second instance
        // starting meanwhile is harmless — the claim skips locked rows).
        max_run_ms: Some(20_000),
        per_instance: false,
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { run_deferred_push(&pg).await })
        }),
    }
}

pub fn register_deferred_push_job(pg: PgPool) {
    talaria_scheduler::register_job(deferred_push_job_spec(pg));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_767_268_800_000; // 2026-01-01T12:00:00Z

    // The send-time re-check reads the matrix as it is now, through the
    // override the row was filed with — so a deliberate watch on a board set
    // to Off, filed with that board's push override, is pushed when its
    // minute is up, and a matrix Push turned off since is obeyed only where
    // nothing overrode it.
    #[test]
    fn the_deferred_push_rechecks_the_matrix_through_the_rows_override() {
        let none = json!({});
        // Following (task-status) does not push by default…
        assert!(!still_pushes(&none, "task-status", None, true, "UTC", NOW));
        // …unless the row was filed with a push override.
        assert!(still_pushes(
            &none,
            "task-status",
            Some(true),
            true,
            "UTC",
            NOW
        ));
        // A mention pushes by default; a row filed with push off does not.
        assert!(still_pushes(&none, "mention", None, true, "UTC", NOW));
        assert!(!still_pushes(
            &none,
            "mention",
            Some(false),
            true,
            "UTC",
            NOW
        ));
        // The matrix turned off inside the minute: obeyed without an
        // override, not over one (the board's choice was made at fan-out).
        let off = json!({ "channels": { "mention": { "push": false } } });
        assert!(!still_pushes(&off, "mention", None, true, "UTC", NOW));
        assert!(still_pushes(&off, "mention", Some(true), true, "UTC", NOW));
        // The instance switch and a pause still win over an override.
        assert!(!still_pushes(
            &none,
            "mention",
            Some(true),
            false,
            "UTC",
            NOW
        ));
        let paused = json!({ "dnd": { "pausedUntil": crate::prefs::FOREVER } });
        assert!(!still_pushes(
            &paused,
            "task-status",
            Some(true),
            true,
            "UTC",
            NOW
        ));
        // A breaks-through class still breaks through.
        assert!(still_pushes(
            &paused,
            "approval_pending",
            None,
            true,
            "UTC",
            NOW
        ));
    }
}
