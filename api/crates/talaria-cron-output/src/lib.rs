// A CRON RUN'S REPORT, LANDED IN THE THREAD THAT SCHEDULED IT — by either of
// its two roads.
//
// A schedule made from a conversation reports into that conversation. The
// first road is the push: the talaria-events plugin inside the agent posts the
// run's final text to /api/agents/cron-output at `post_llm_call`
// (talaria-routes-fleet's `cron_output`). That plugin is fire-and-forget by
// contract — a bounded queue, a 4 s POST, any failure drops the event — so an
// api restart or deploy at fire time, a timeout, or an agent stopped with the
// event still queued lost that run's report for good.
//
// The second road is the pull (`pull`): every five minutes the api asks each
// agent with a listening thread for its finished cron sessions
// (`GET /api/sessions?source=cron` on the agent's own API server), and lands
// any whose report never arrived. Both roads end in `land`: the same clamp,
// secret scrub, reply guard, responsible-user stamp and fan-out, so a pulled
// report is indistinguishable from a pushed one except for its marker.
//
// THE HERMES SESSION ID IS THE DEDUPE KEY. Every cron run is one Hermes
// session; the push carries its id, the pull reads it off the listing, and
// both write it into the message row's metadata (`cronSession`). The two
// roads take one Postgres advisory lock per (thread, session) around the
// check and the insert, so a late push and the pull can never both land the
// same run: the pull skips a session any row already carries, and the push
// skips one the pull already landed. The push keeps its own marker
// (`cron:<job>:<execution>:<turn>`) as before, so a re-delivered push is
// still the same row.
//
// A RUN THAT HAD NOTHING TO SAY LANDS NOTHING. The cron prompt tells the
// agent to answer exactly `[SILENT]` when there is nothing new, and Hermes
// then skips its own delivery — but both of Talaria's roads see the text
// before (the push) or without (the pull) that decision, and used to land
// the sentinel as a message. `land` now reads it with Hermes's own rule
// (`silence`) before anything else: a silent run writes no message, rings
// nobody, and is recorded in `cron_silent_runs` as delivered silent, under
// the same run key the two roads dedupe on (the session id, or the push's
// marker when it carried none), so the pull never takes a quiet run for a
// report the push lost.

pub mod pull;
pub mod silence;

use redis::aio::ConnectionManager;
use serde_json::{Map, Value};
use sqlx::PgPool;

use talaria_agent_crons::JobListener;
use talaria_body::truncate_utf16;
use talaria_gateway::guard::{
    GuardMode, Spread, guard_chat_reply, needs_redaction, redact_findings, redact_secrets,
};
use talaria_notify::{NotifyDeps, fan_conversation_event, notify_agent_reply};

pub use pull::{pull_pass, register_cron_pull_job};
pub use silence::is_silence;

const LOG: &str = "[cron-output]";

/// What the row may hold, regardless of what the plugin clamped to. UTF-16
/// units because that is the length every surface downstream counts in.
pub const MAX_TEXT: usize = 20_000;

/// The `metadata` stamp that names the Hermes session a cron turn came from.
pub const SESSION_STAMP: &str = "cronSession";
/// Which road landed it: `push` or `pull`.
pub const VIA_STAMP: &str = "cronVia";

/// Which road a report came by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The plugin's POST to /api/agents/cron-output.
    Push,
    /// The backstop's read of the agent's own session store.
    Pull,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Push => "push",
            Via::Pull => "pull",
        }
    }
}

/// One report to land.
#[derive(Debug, Clone)]
pub struct CronReport<'a> {
    /// The agent that ran the job (the caller's model on the push).
    pub agent_model: &'a str,
    /// The Hermes job the run belongs to.
    pub job_id: &'a str,
    /// The idempotency key `post_agent_turn` writes: the push's
    /// `cron:<job>:<execution>:<turn>`, or the pull's `pull_marker`.
    pub marker: String,
    /// The Hermes session the run was, when the road knows it. A push from a
    /// plugin that sent none lands exactly as it did before this existed.
    pub session_id: Option<&'a str>,
    pub via: Via,
    /// The run's final text, as the road received it (unclamped, unscrubbed).
    pub text: &'a str,
}

/// A report that is a row in the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub message_id: String,
    /// False when the run was already in the thread (the same marker, or the
    /// same session landed by the other road): no row was inserted and
    /// nobody was rung (an empty responsible-user slot is still filled).
    pub fresh: bool,
}

/// What landing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The report is a message in the thread.
    Landed(Landed),
    /// The run answered Hermes's silence sentinel: no message, nobody rung,
    /// and the run recorded as delivered silent. `fresh` is false when either
    /// road had already recorded it.
    Silent { fresh: bool },
}

/// The key a quiet run is recorded under: what the two roads dedupe on —
/// the Hermes session id, or the push's own marker when it carried none.
fn run_key<'a>(report: &'a CronReport<'_>) -> &'a str {
    report.session_id.unwrap_or(report.marker.as_str())
}

/// Record a quiet run as delivered silent. True when this call wrote it.
async fn record_silent(
    pg: &PgPool,
    conversation_id: &str,
    report: &CronReport<'_>,
) -> Result<bool, sqlx::Error> {
    let wrote: Option<i32> = sqlx::query_scalar(
        "insert into cron_silent_runs (conversation_id, run_key, job_id, agent_model, via) \
         values ($1::uuid, $2, $3, $4, $5) on conflict do nothing returning 1",
    )
    .bind(conversation_id)
    .bind(run_key(report))
    .bind(report.job_id)
    .bind(report.agent_model)
    .bind(report.via.as_str())
    .fetch_optional(pg)
    .await?;
    Ok(wrote.is_some())
}

/// What the row may hold, regardless of what the road clamped to.
pub fn clamp_text(text: &str) -> &str {
    truncate_utf16(text, MAX_TEXT)
}

/// The pull's marker. It keeps the push marker's grammar,
/// `cron:<job>:<execution>:<turn>`, so every reader that takes the job from
/// the second field (the thread's cron stamp) reads a pulled turn the same
/// way: the execution field holds the Hermes session the pull found the run
/// under (the execution id is not on the session listing), and the turn
/// field says it was pulled.
pub fn pull_marker(job: &str, session_id: &str) -> String {
    format!("cron:{job}:{session_id}:pull")
}

/// The row a road must not land beside, if one is already there. The pull
/// stands down for ANY row of the session; the push only for one the pull
/// landed, because a push's own repeat is already the marker's job (and a
/// run with two turns pushes two rows for one session).
async fn already_landed(
    pg: &PgPool,
    conversation_id: &str,
    session_id: &str,
    via: Via,
) -> Result<Option<String>, sqlx::Error> {
    let only_pulled = via == Via::Push;
    sqlx::query_scalar(
        "select id::text from messages \
         where conversation_id = $1::uuid and metadata->>'cronSession' = $2 \
           and (not $3 or metadata->>'cronVia' = 'pull') \
         order by seq limit 1",
    )
    .bind(conversation_id)
    .bind(session_id)
    .bind(only_pulled)
    .fetch_optional(pg)
    .await
}

async fn marker_row(
    pg: &PgPool,
    conversation_id: &str,
    marker: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select id::text from messages \
         where conversation_id = $1::uuid and metadata->>'marker' = $2 limit 1",
    )
    .bind(conversation_id)
    .bind(marker)
    .fetch_optional(pg)
    .await
}

/// Land one cron run's report in the thread listening to its job — or, when
/// the run answered Hermes's silence sentinel, record it as delivered silent
/// and land nothing (see the header).
///
/// The clamp, the scrub and the reply guard happen HERE, at the boundary,
/// before the text becomes a message row that models and transcripts will
/// read back: a cron turn is nobody's open stream, so this is the only place
/// the chat path's landing pass can happen. Strict mode redacts the SAVED
/// copy before it is written; annotate/strict pin the scrubbed findings on
/// the row as the caveat the UI renders.
pub async fn land(
    pg: &PgPool,
    redis: Option<ConnectionManager>,
    listener: &JobListener,
    report: CronReport<'_>,
) -> Result<Outcome, sqlx::Error> {
    let conversation = listener.conversation_id.as_str();
    // Nothing to report: read on the text as the road received it, which is
    // the text Hermes itself judged (`post_llm_call` hands the push the
    // stored final response). No row, no guard pass, no bell.
    if is_silence(report.text) {
        let fresh = record_silent(pg, conversation, &report).await?;
        if fresh {
            tracing::debug!(
                "{LOG} {}: job {} answered Hermes's silence sentinel — nothing landed in thread \
                 {conversation}, recorded as delivered silent ({})",
                report.agent_model,
                report.job_id,
                report.via.as_str()
            );
        }
        return Ok(Outcome::Silent { fresh });
    }
    // Scrub at the boundary — the same shapes the transcript capture kills.
    // A cron turn is a stored, re-readable message, so a key echoed into the
    // agent's closing summary would otherwise outlive the run.
    let text = talaria_runs_work_session::scrub_secrets(clamp_text(report.text));
    // No tool names reach here (the cron ran inside the container), so the
    // record honestly says neither half is supplied; a conversation is a
    // contained audience.
    let (findings, mode) = guard_chat_reply(
        pg,
        &text,
        &[],
        "",
        &format!("cron:{}", report.agent_model),
        report.agent_model,
        Spread::Contained,
    )
    .await;
    let text = if mode == GuardMode::Strict && needs_redaction(&findings) {
        redact_secrets(&text, None).0
    } else {
        text
    };

    let mut stamps = Map::new();
    stamps.insert(VIA_STAMP.into(), Value::String(report.via.as_str().into()));
    // THE CHECK AND THE INSERT UNDER ONE LOCK per (thread, session): the
    // transaction holds nothing but the advisory lock, and it is released at
    // commit — or when the connection drops, so a crashed request never
    // wedges the next road.
    let mut lock = match report.session_id {
        Some(session) => {
            stamps.insert(SESSION_STAMP.into(), Value::String(session.to_string()));
            let mut tx = pg.begin().await?;
            sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("cron-turn:{conversation}:{session}"))
                .execute(&mut *tx)
                .await?;
            if let Some(id) = already_landed(pg, conversation, session, report.via).await? {
                tx.commit().await?;
                return Ok(Outcome::Landed(Landed {
                    message_id: id,
                    fresh: false,
                }));
            }
            Some(tx)
        }
        None => None,
    };
    let landed = async {
        if let Some(id) = marker_row(pg, conversation, &report.marker).await? {
            return Ok::<_, sqlx::Error>(Landed {
                message_id: id,
                fresh: false,
            });
        }
        let id = talaria_conversations::post_agent_turn_with(
            pg,
            conversation,
            &report.marker,
            &stamps,
            &text,
        )
        .await?;
        Ok(Landed {
            message_id: id,
            fresh: true,
        })
    }
    .await;
    if let Some(tx) = lock.take() {
        // A commit that fails here only releases the lock early; the row
        // (or the error) is what the caller is owed.
        if let Err(e) = tx.commit().await {
            tracing::warn!("{LOG} {conversation}: releasing the landing lock failed: {e}");
        }
    }
    let landed = landed?;
    let message_id = landed.message_id.clone();

    // THE RESPONSIBLE USER. A cron turn arrives with no person behind it —
    // nobody is chatting — so the turn is attributed to the person who set
    // the schedule up: `author_user_id` is the one slot a turn carries for a
    // human, and the UI labels only user turns from it, so an assistant row
    // gains an accountable person without gaining a byline. Stamped on a
    // repeat too: it only fills an empty slot, so a delivery that wrote the
    // row and died before this line is mended by the next one.
    if let Some(creator) = listener.created_by.as_deref()
        && let Err(e) = sqlx::query(
            "update messages set author_user_id = $2::uuid \
             where id = $1::uuid and author_user_id is null",
        )
        .bind(&message_id)
        .bind(creator)
        .execute(pg)
        .await
    {
        tracing::error!("{LOG} responsible-user stamp failed: {e}");
    }
    // The rest rings people, so it happens once, for the row this call
    // wrote. A repeat does not re-ring: a run seen twice (a re-delivered
    // push, the pull after a late push) is one report, and a second
    // notification for it would be noise. The cost is the rare delivery that
    // wrote its row and died before the fan-out: that report sits in the
    // thread without a notification, and the next reader still sees it.
    if !landed.fresh {
        return Ok(Outcome::Landed(landed));
    }
    if !findings.is_empty() && matches!(mode, GuardMode::Annotate | GuardMode::Strict) {
        let _ =
            talaria_conversations::set_message_guard(pg, &message_id, &redact_findings(&findings))
                .await;
    }
    // Exactly what the chat persist path does once a reply lands complete:
    // every reader's thread pill moves, and whoever was away gets the one
    // agent-reply row. Both are id-shaped and both are detached — a cron
    // turn is nobody's open stream, so the fan-out is the ONLY way this
    // conversation learns it moved.
    fan_conversation_event(
        NotifyDeps::publishing(pg.clone(), redis.clone()),
        conversation.to_string(),
    );
    {
        let notify = NotifyDeps::publishing(pg.clone(), redis);
        let conversation = conversation.to_string();
        let message_id = message_id.clone();
        tokio::spawn(async move {
            notify_agent_reply(&notify, &conversation, &message_id).await;
        });
    }
    Ok(Outcome::Landed(landed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_clamped_at_the_boundary() {
        let short = "the digest";
        assert_eq!(clamp_text(short), short);
        let long = "x".repeat(MAX_TEXT + 500);
        assert_eq!(clamp_text(&long).len(), MAX_TEXT);
        // UTF-16 units, not bytes or chars: an emoji is a surrogate pair, so
        // half the cap's worth of them fills it exactly and the cut never
        // lands inside a character.
        let emoji = "🛰".repeat(MAX_TEXT);
        let cut = clamp_text(&emoji);
        assert_eq!(cut.chars().count(), MAX_TEXT / 2);
        assert!(emoji.starts_with(cut));
    }

    // The pull's marker keeps the push's four-field grammar, so the thread's
    // cron stamp (the job, read from the second field) reads it the same way,
    // and it can never collide with a push marker for the same job: an
    // execution id is bare hex, a session id is not.
    #[test]
    fn the_pull_marker_names_the_job_and_the_session() {
        let m = pull_marker(
            "abc123def456",
            "local-9f0c4b0e8b5f4a1e9d2c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e",
        );
        assert_eq!(
            m,
            "cron:abc123def456:local-9f0c4b0e8b5f4a1e9d2c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e:pull"
        );
        let fields: Vec<&str> = m.split(':').collect();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[1], "abc123def456");
    }
}

// ── Live DB: the two roads against one thread ──────────────────────────────
// What a unit test cannot see is the dedupe itself: the advisory lock, the
// metadata stamps the check reads, and the marker check, all against the real
// messages table. #[ignore]d; CI's live job runs it on its migrated scratch
// database:
//
//   DATABASE_URL=postgres://… cargo test -p talaria-cron-output --lib -- --ignored live_db
//
// Every row it writes hangs off one user it deletes at the end (users →
// conversations → messages cascade).
#[cfg(test)]
mod live_db {
    use super::*;
    use crate::pull::{CronSession, Listener, delivered, landed_in_any};

    async fn pg() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        PgPool::connect(&url).await.expect("connect")
    }

    const JOB: &str = "abcdef012345";

    fn report<'a>(
        marker: &str,
        session: Option<&'a str>,
        via: Via,
        text: &'a str,
    ) -> CronReport<'a> {
        CronReport {
            agent_model: "cron-output-live-agent",
            job_id: JOB,
            marker: marker.to_string(),
            session_id: session,
            via,
            text,
        }
    }

    /// `land` for the cases whose text is a report: the row it wrote.
    async fn land(
        pg: &PgPool,
        redis: Option<ConnectionManager>,
        listener: &JobListener,
        report: CronReport<'_>,
    ) -> Result<Landed, sqlx::Error> {
        match super::land(pg, redis, listener, report).await? {
            Outcome::Landed(landed) => Ok(landed),
            other => panic!("a report landed as {other:?}"),
        }
    }

    /// A user and a thread of their own, every row of which goes with the
    /// user at the end.
    async fn thread(pg: &PgPool, what: &str) -> (String, String, JobListener) {
        let tag = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let email = format!("cron-output-{what}-{tag}@link-test.invalid");
        let user: String = sqlx::query_scalar(
            "insert into users (sub, email, name, role) values ($1, $1, 'Cron Live', 'member') \
             returning id::text",
        )
        .bind(&email)
        .fetch_one(pg)
        .await
        .unwrap();
        let conversation: String = sqlx::query_scalar(
            "insert into conversations (user_id, agent_model) \
             values ($1::uuid, 'cron-output-live-agent') returning id::text",
        )
        .bind(&user)
        .fetch_one(pg)
        .await
        .unwrap();
        let listener = JobListener {
            conversation_id: conversation.clone(),
            created_by: Some(user.clone()),
        };
        (user, conversation, listener)
    }

    async fn rows(
        pg: &PgPool,
        conversation: &str,
    ) -> Vec<(String, Option<String>, Option<String>)> {
        sqlx::query_as(
            "select content, metadata->>'cronSession', metadata->>'cronVia' from messages \
             where conversation_id = $1::uuid order by seq",
        )
        .bind(conversation)
        .fetch_all(pg)
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_two_roads_land_each_run_once() {
        let pg = pg().await;
        let (user, conversation, listener) = thread(&pg, "live").await;
        let job = JOB;

        // Push first, then the pull finds the same session: one row.
        let a = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e1:t1"),
                Some("local-s1"),
                Via::Push,
                "run one",
            ),
        )
        .await
        .unwrap();
        assert!(a.fresh);
        let a2 = land(
            &pg,
            None,
            &listener,
            report(
                &pull_marker(job, "local-s1"),
                Some("local-s1"),
                Via::Pull,
                "run one",
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            a2,
            Landed {
                message_id: a.message_id.clone(),
                fresh: false
            }
        );

        // The pull first, then a late push of that run: one row, the pull's.
        let b = land(
            &pg,
            None,
            &listener,
            report(
                &pull_marker(job, "local-s2"),
                Some("local-s2"),
                Via::Pull,
                "run two",
            ),
        )
        .await
        .unwrap();
        assert!(b.fresh);
        let b2 = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e2:t2"),
                Some("local-s2"),
                Via::Push,
                "run two",
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            b2,
            Landed {
                message_id: b.message_id.clone(),
                fresh: false
            }
        );

        // One run, two pushed turns: both land (the marker tells them
        // apart), and a re-delivered push is the first one's row.
        let c1 = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e3:t1"),
                Some("local-s3"),
                Via::Push,
                "turn one",
            ),
        )
        .await
        .unwrap();
        let c2 = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e3:t2"),
                Some("local-s3"),
                Via::Push,
                "turn two",
            ),
        )
        .await
        .unwrap();
        assert!(c1.fresh && c2.fresh && c1.message_id != c2.message_id);
        let c3 = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e3:t1"),
                Some("local-s3"),
                Via::Push,
                "turn one",
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            c3,
            Landed {
                message_id: c1.message_id.clone(),
                fresh: false
            }
        );

        // A push with no session id lands on its marker alone, as before.
        let d = land(
            &pg,
            None,
            &listener,
            report(&format!("cron:{job}:e4:"), None, Via::Push, "no session"),
        )
        .await
        .unwrap();
        assert!(d.fresh);

        // The race: a push and the pull of one run at the same moment.
        let (p, q) = tokio::join!(
            land(
                &pg,
                None,
                &listener,
                report(
                    &format!("cron:{job}:e5:t1"),
                    Some("local-s5"),
                    Via::Push,
                    "raced"
                )
            ),
            land(
                &pg,
                None,
                &listener,
                report(
                    &pull_marker(job, "local-s5"),
                    Some("local-s5"),
                    Via::Pull,
                    "raced"
                )
            ),
        );
        let (p, q) = (p.unwrap(), q.unwrap());
        assert_eq!(p.message_id, q.message_id, "one row for one run");
        assert!(p.fresh != q.fresh, "exactly one road wrote it");

        let all = rows(&pg, &conversation).await;
        let texts: Vec<&str> = all.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "run one",
                "run two",
                "turn one",
                "turn two",
                "no session",
                "raced"
            ]
        );
        assert_eq!(all[0].1.as_deref(), Some("local-s1"));
        assert_eq!(all[0].2.as_deref(), Some("push"));
        assert_eq!(all[1].2.as_deref(), Some("pull"));
        assert_eq!(all[4].1, None);
        // A delivery that wrote its row and died before the stamp: the next
        // delivery of that run writes nothing and mends the stamp.
        sqlx::query("update messages set author_user_id = null where id = $1::uuid")
            .bind(&c1.message_id)
            .execute(&pg)
            .await
            .unwrap();
        let c4 = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{job}:e3:t1"),
                Some("local-s3"),
                Via::Push,
                "turn one",
            ),
        )
        .await
        .unwrap();
        assert!(!c4.fresh && c4.message_id == c1.message_id);
        // The responsible user is the schedule's creator.
        let stamped: i64 = sqlx::query_scalar(
            "select count(*) from messages where conversation_id = $1::uuid and author_user_id = $2::uuid",
        )
        .bind(&conversation)
        .bind(&user)
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(stamped, 6);

        // ALREADY LANDED, the pull's own check: the session id, or a row of
        // the job inside the run's window (a report pushed before session
        // ids were stamped). Nothing of another job, or outside the window.
        let l = Listener {
            job_id: job.into(),
            conversation_id: conversation.clone(),
            created_by: None,
            listening_since_ms: 0,
        };
        let now = talaria_agent_auth::now_ms();
        let run = |id: &str, started: i64, ended: i64| CronSession {
            id: id.into(),
            title: String::new(),
            started_ms: started,
            ended_ms: ended,
        };
        assert!(delivered(&pg, &l, &run("local-s2", 0, 1)).await.unwrap());
        // The tiebreak's pre-check: the session id in any of the threads.
        assert!(
            landed_in_any(&pg, &[conversation.as_str()], "local-s2")
                .await
                .unwrap()
        );
        assert!(
            !landed_in_any(&pg, &[conversation.as_str()], "local-nope")
                .await
                .unwrap()
        );
        let legacy: String = sqlx::query_scalar(
            "insert into messages (conversation_id, seq, role, content, metadata, created_at) \
             values ($1::uuid, 100, 'assistant', 'legacy', $2, now() - interval '1 hour') returning id::text",
        )
        .bind(&conversation)
        .bind(serde_json::json!({"marker": format!("cron:{job}:e0:t0")}))
        .fetch_one(&pg)
        .await
        .unwrap();
        assert!(!legacy.is_empty());
        let hour_ago = now - 3_600_000;
        assert!(
            delivered(
                &pg,
                &l,
                &run("local-old", hour_ago - 20_000, hour_ago + 5_000)
            )
            .await
            .unwrap()
        );
        assert!(
            !delivered(
                &pg,
                &l,
                &run("local-older", hour_ago - 600_000, hour_ago - 300_000)
            )
            .await
            .unwrap()
        );
        let other = Listener {
            job_id: "fedcba543210".into(),
            ..l.clone()
        };
        assert!(
            !delivered(
                &pg,
                &other,
                &run("local-old", hour_ago - 20_000, hour_ago + 5_000)
            )
            .await
            .unwrap()
        );

        sqlx::query("delete from users where id = $1::uuid")
            .bind(&user)
            .execute(&pg)
            .await
            .unwrap();
    }

    async fn silent_rows(pg: &PgPool, conversation: &str) -> Vec<(String, String)> {
        sqlx::query_as(
            "select run_key, via from cron_silent_runs where conversation_id = $1::uuid \
             order by created_at, run_key",
        )
        .bind(conversation)
        .fetch_all(pg)
        .await
        .unwrap()
    }

    // A QUIET RUN, BY BOTH ROADS: nothing lands, nobody is rung, and the run
    // is recorded once under the key the roads dedupe on — which is what the
    // pull's own "already delivered" check then reads.
    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_quiet_run_lands_nothing_and_counts_as_delivered() {
        let pg = pg().await;
        let (user, conversation, listener) = thread(&pg, "quiet").await;
        let silent = |o: Outcome| match o {
            Outcome::Silent { fresh } => fresh,
            other => panic!("a quiet run landed as {other:?}"),
        };

        // The push of a quiet run, then its re-delivery, then the pull of
        // the same session: one record, the push's.
        let push = report(
            &format!("cron:{JOB}:e1:t1"),
            Some("local-q1"),
            Via::Push,
            "[SILENT]",
        );
        assert!(silent(
            super::land(&pg, None, &listener, push.clone())
                .await
                .unwrap()
        ));
        assert!(!silent(
            super::land(&pg, None, &listener, push).await.unwrap()
        ));
        let pulled = report(
            &pull_marker(JOB, "local-q1"),
            Some("local-q1"),
            Via::Pull,
            "[SILENT]",
        );
        assert!(!silent(
            super::land(&pg, None, &listener, pulled).await.unwrap()
        ));

        // The push lost: the pull records the run itself, and a late push of
        // it (the sentinel with a note, as models write it) adds nothing.
        let pulled = report(
            &pull_marker(JOB, "local-q2"),
            Some("local-q2"),
            Via::Pull,
            "Checked every feed.\n[SILENT]",
        );
        assert!(silent(
            super::land(&pg, None, &listener, pulled).await.unwrap()
        ));
        let late = report(
            &format!("cron:{JOB}:e2:t1"),
            Some("local-q2"),
            Via::Push,
            "[SILENT] nothing new",
        );
        assert!(!silent(
            super::land(&pg, None, &listener, late).await.unwrap()
        ));

        // A push from a plugin that sent no session id: recorded under its
        // marker, and a repeat of it is the same record.
        let bare = report(&format!("cron:{JOB}:e3:"), None, Via::Push, "NO_REPLY");
        assert!(silent(
            super::land(&pg, None, &listener, bare.clone())
                .await
                .unwrap()
        ));
        assert!(!silent(
            super::land(&pg, None, &listener, bare).await.unwrap()
        ));

        // Nothing reached the thread, and nobody was rung.
        assert!(rows(&pg, &conversation).await.is_empty());
        let rung: i64 =
            sqlx::query_scalar("select count(*) from notifications where user_id = $1::uuid")
                .bind(&user)
                .fetch_one(&pg)
                .await
                .unwrap();
        assert_eq!(rung, 0);
        assert_eq!(
            silent_rows(&pg, &conversation).await,
            vec![
                ("local-q1".to_string(), "push".to_string()),
                ("local-q2".to_string(), "pull".to_string()),
                (format!("cron:{JOB}:e3:"), "push".to_string()),
            ]
        );

        // The pull's "already delivered": the record by session id, or a
        // record of the job inside the run's window; never another job's,
        // never outside the window.
        let l = Listener {
            job_id: JOB.into(),
            conversation_id: conversation.clone(),
            created_by: None,
            listening_since_ms: 0,
        };
        let now = talaria_agent_auth::now_ms();
        let run = |id: &str, started: i64, ended: i64| CronSession {
            id: id.into(),
            title: String::new(),
            started_ms: started,
            ended_ms: ended,
        };
        assert!(delivered(&pg, &l, &run("local-q1", 0, 1)).await.unwrap());
        assert!(
            delivered(&pg, &l, &run("local-unstamped", now - 20_000, now + 5_000))
                .await
                .unwrap()
        );
        assert!(
            !delivered(&pg, &l, &run("local-old", now - 7_200_000, now - 3_600_000))
                .await
                .unwrap()
        );
        let other = Listener {
            job_id: "fedcba543210".into(),
            ..l.clone()
        };
        assert!(
            !delivered(
                &pg,
                &other,
                &run("local-unstamped", now - 20_000, now + 5_000)
            )
            .await
            .unwrap()
        );
        assert!(
            landed_in_any(&pg, &[conversation.as_str()], "local-q2")
                .await
                .unwrap()
        );

        // A report that only MENTIONS the sentinel is a report.
        let real = land(
            &pg,
            None,
            &listener,
            report(
                &format!("cron:{JOB}:e4:t1"),
                Some("local-q4"),
                Via::Push,
                "Yesterday was [SILENT]; today 2 tickets moved.",
            ),
        )
        .await
        .unwrap();
        assert!(real.fresh);
        assert_eq!(rows(&pg, &conversation).await.len(), 1);

        // The prune drops what no pass can consult any more, and only that.
        sqlx::query(
            "update cron_silent_runs set created_at = now() - interval '3 days' \
             where conversation_id = $1::uuid and run_key = 'local-q1'",
        )
        .bind(&conversation)
        .execute(&pg)
        .await
        .unwrap();
        crate::pull::prune_silent(&pg).await.unwrap();
        let keys: Vec<String> = silent_rows(&pg, &conversation)
            .await
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec!["local-q2".to_string(), format!("cron:{JOB}:e3:")]
        );

        sqlx::query("delete from users where id = $1::uuid")
            .bind(&user)
            .execute(&pg)
            .await
            .unwrap();
        // The records go with the thread.
        assert!(silent_rows(&pg, &conversation).await.is_empty());
    }
}
