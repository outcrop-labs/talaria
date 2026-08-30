// THE TWO RETRIEVAL REPAIR RUNS, on the durable runtime. These are the last
// two kinds the flip's census waits on — with them, every kind TS's
// runs/boot.ts imports has a definition here and the scheduler handover is
// armable. Port of ui/src/server/runs/defs/reindex.ts.
//
// WHAT THEY REPLACE, and it is the same bug twice:
//
//   retrieval/backfill.ts   `let backfillRunning = false` plus a status blob in
//   retrieval/migrate.ts    `app_settings`, driven by a bare `void fn()`. A
//                           deploy in the middle of either one leaves
//                           state:'running' in that blob FOREVER — nothing is
//                           driving it, nothing will ever notice, and the admin
//                           panel polls a row that will never change again. The
//                           only way out is to press the button a second time,
//                           which starts the whole thing from zero.
//
// So the status blob is gone and the `runs` row is the source of truth. The
// shapes the panel reads (`BackfillStatus`, `ReindexStatus`) stay TS for now —
// they are projections of this row and cross with the admin rag route.
//
// ── WHAT ONE STEP IS ─────────────────────────────────────────────────────────
//
// BACKFILL: one PAGE of one source. The checkpoint is a source plus a keyset
// cursor, so a resumed run re-indexes at most the page that was in flight —
// never the sources behind it. Keyset (`id > cursor`) rather than OFFSET on
// purpose: rows are written while a backfill runs, and an OFFSET page silently
// SKIPS a row for every insert that lands ahead of the cursor. A backfill that
// quietly misses documents is worse than one that is slow.
//
// REINDEX: one COLLECTION rebuilt, then the backfill's own steps. The rebuild
// is the most destructive thing in the product — it DROPS a Qdrant collection
// and purges the content-hash bookkeeping — so the re-entry rules are written
// into the machine below rather than assumed.
//
// ── AT-LEAST-ONCE, AND WHY THESE TWO SURVIVE IT ──────────────────────────────
//
//   BACKFILL IS CONTENT-HASH IDEMPOTENT by construction: `sync_kb_doc`,
//   `index_ticket` and `index_activity` all no-op on an unchanged document. A
//   page that ran and did not checkpoint costs its embeddings a second time and
//   changes nothing else, which is the cheapest possible answer to rule 1.
//
//   THE REBUILD IS NOT, and the dangerous ordering is precise. Re-dropping a
//   collection that was JUST dropped and recreated is harmless — nothing has
//   refilled it yet. Re-dropping one the BACKFILL HAS ALREADY REFILLED destroys
//   work. The whole guard is therefore that the `rebuilding → backfilling` flip
//   is A STEP OF ITS OWN WITH NO OUTWARD EFFECT IN IT: the last collection's
//   rebuild checkpoints `rebuilt: [...all]`, and the step after it does nothing
//   but return the new phase. A crash anywhere in that window re-enters a step
//   that drops nothing. Put the flip in the same step as the last drop and the
//   window becomes "we rebuilt everything, the write was lost, now we rebuild
//   over a half-filled index" — which is the one failure this module exists to
//   make impossible. (There is a test pinning exactly that step, because a
//   future edit folding it back into the last rebuild would look like a
//   simplification.)
//
//   THE DROP COMES BEFORE ITS CHECKPOINT, deliberately, and the alternative is
//   worse. Marking a collection rebuilt BEFORE dropping it would mean a crash in
//   between leaves a collection that was never rebuilt but is recorded as done:
//   its vectors keep the old dimension, every index and search call against it
//   goes on failing, and nothing will ever look at it again. A repeated drop
//   costs one empty collection recreated twice.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use crate::retrieval::artifact_routing::apply_artifact_routing;
use crate::retrieval::backfill::{RagHealth, rag_health};
use crate::retrieval::collections::ensure_qdrant_for;
use crate::retrieval::embed::{EmbedDeps, embed_info, embed_one};
use crate::retrieval::index::IndexDoc;
use crate::retrieval::qdrant::{QdrantDeps, delete_collection, ensure_hybrid_collection};
use crate::retrieval::sources::{
    CommentSrc, EFFECTIVE_DOC_SELECT, KbDocSync, TicketSrc, index_activity, index_personal,
    index_ticket, index_ticket_comment, sync_kb_doc,
};
use crate::runs::define::{
    Authority, DEFAULT_MAX_ATTEMPTS, RunDefinition, RunStepContext, StepError, StepResult,
    StepSignal, register_run,
};
use crate::runs::run::{EnqueueOptions, enqueue};
use crate::runs::store::active_run_of_kind;
use crate::state::AppState;

const LOG: &str = "[retrieval/runs]";

/// ONE PAGE. Big enough that the checkpoint write is noise against the
/// embeddings it covers, small enough that a cancel lands within a minute or
/// so and a reclaim re-buys at most this many embeddings.
pub const BACKFILL_PAGE: usize = 100;

/// How long one page (or one collection rebuild) may take before the driver
/// abandons it. It is also the lease TTL, so a crashed driver's run is
/// reclaimable roughly this long after it stops renewing — four minutes is a
/// page of a hundred slow embeddings with room, and a recovery delay an admin
/// watching a repair job will accept.
const MAX_STEP_MS: u64 = 240_000;

/// Come back when the retrieval services are up. A `retry` rather than an
/// error: a dead Qdrant is a fact about the deployment, and failing the run
/// for it would mean an admin has to notice the services came back AND
/// remember to press the button again. The row stays visible with the reason
/// as its phase.
const SERVICES_DOWN_RETRY_MS: u64 = 60_000;

/// The zero uuid, as the "before every row" cursor. Keyset paging needs a
/// lower bound and `id > null` is null; coalescing in SQL keeps the predicate
/// one expression instead of two query shapes per source.
const NO_CURSOR: &str = "00000000-0000-0000-0000-000000000000";

// ── The backfill checkpoint ──────────────────────────────────────────────────

/// The systems of record, in the order a backfill walks them. Order is part of
/// the checkpoint's meaning — a resumed run continues at `source` and never
/// revisits the ones behind it — so the array is the sequence, not a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackfillSource {
    Collections,
    KbDocs,
    ChannelMessages,
    Tickets,
    Comments,
    PlanTurns,
    Artifacts,
}

pub const BACKFILL_SOURCES: &[BackfillSource] = &[
    BackfillSource::Collections,
    BackfillSource::KbDocs,
    BackfillSource::ChannelMessages,
    BackfillSource::Tickets,
    BackfillSource::Comments,
    BackfillSource::PlanTurns,
    BackfillSource::Artifacts,
];

impl BackfillSource {
    /// What a person reads while they wait.
    pub fn label(self) -> &'static str {
        match self {
            BackfillSource::Collections => "preparing the collections",
            BackfillSource::KbDocs => "knowledge base docs",
            BackfillSource::ChannelMessages => "channel messages",
            BackfillSource::Tickets => "tickets",
            BackfillSource::Comments => "ticket comments",
            BackfillSource::PlanTurns => "plan turns",
            BackfillSource::Artifacts => "docs and research",
        }
    }
}

fn next_source(source: BackfillSource) -> Option<BackfillSource> {
    let at = BACKFILL_SOURCES.iter().position(|s| *s == source)?;
    BACKFILL_SOURCES.get(at + 1).copied()
}

/// The per-source tally the admin panel prints. Carried in the checkpoint
/// rather than recomputed, because "how much did this backfill actually
/// index" is not derivable from a cursor. BTreeMap so a checkpoint's bytes are
/// the same from any process — TS's insertion order was walk order, and the
/// walk order is fixed.
pub type Counts = BTreeMap<String, i64>;

fn bump(counts: &mut Counts, key: &str) {
    *counts.entry(key.to_string()).or_insert(0) += 1;
}

fn total_of(counts: &Counts) -> i64 {
    counts.values().sum()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackfillCheckpoint {
    /// The source the NEXT page comes from.
    pub source: BackfillSource,
    /// The id of the last row indexed in `source`; null at the start of one.
    /// Keyset, not an offset — see the module header.
    pub cursor: Option<String>,
    pub counts: Counts,
}

fn fresh_backfill() -> BackfillCheckpoint {
    BackfillCheckpoint {
        source: BackfillSource::Collections,
        cursor: None,
        counts: Counts::new(),
    }
}

// ── One page of one source ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Nothing left in this source.
    pub done: bool,
    /// The last id indexed, so a resume starts after it. Unchanged from the
    /// one handed in when the page indexed nothing.
    pub cursor: Option<String>,
    pub counts: Counts,
}

/// A `json!` object as the payload Map (preserve_order keeps insertion order —
/// the shape TS's object literals wrote into the column).
fn obj(v: Value) -> serde_json::Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("the literal is always an object"),
    }
}

/// Index one page. Every write goes through the SAME indexer an ordinary save
/// runs, which is what makes a re-entered page free: the content hash makes an
/// unchanged document a no-op.
///
/// `signal` is honored BEFORE every outward call, not merely awaited on. A
/// step the driver abandoned (deadline, lost lease) keeps running — nothing
/// can stop code that ignores its signal — and a page that went on embedding
/// after another instance took the run is the doubled side effect this
/// runtime exists to prevent.
///
/// The SELECTs themselves propagate failures (TS's `await sql` throws too);
/// every INDEX call is `let _ =` (TS's `.catch(() => {})`) — one bad document
/// must not cost a page.
async fn index_page(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    source: BackfillSource,
    cursor: Option<String>,
    mut counts: Counts,
    signal: &StepSignal,
) -> Result<Page, StepError> {
    let from = cursor.clone().unwrap_or_else(|| NO_CURSOR.to_string());
    let mut last = cursor;
    let page = |done: bool, last: &Option<String>, counts: &Counts| Page {
        done,
        cursor: last.clone(),
        counts: counts.clone(),
    };

    match source {
        BackfillSource::Collections => {
            // Every registered collection gets its Qdrant collection in its
            // registered shape — they may have been registered while Qdrant
            // was down. A handful of rows, so it is one step rather than a
            // paged source.
            let dim = embed_one(ed, "dim probe").await?.len() as i64;
            // ::int8 — the column is integer, the tuple is i64 (see COL_SELECT).
            let cols = sqlx::query_as::<_, (String, i64)>(
                "select qdrant_name, schema_version::int8 from rag_collections",
            )
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            for (name, schema_version) in cols {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let _ = ensure_qdrant_for(qd, &name, schema_version, dim).await;
            }
            Ok(Page {
                done: true,
                cursor: None,
                counts,
            })
        }

        BackfillSource::KbDocs => {
            // EFFECTIVE visibility, the same resolution every save runs
            // (kb.syncDocEffective). Reading `d.visibility` raw here is how
            // private docs got into the org brain: perms_inherited defaults
            // true and visibility defaults 'org', so a doc in a private space
            // reads 'org' until you resolve it through the space. The paging
            // predicate rides on the shared SELECT rather than replacing it —
            // this loop is the batch twin of the live path and the two must
            // not disagree about who may see a document.
            //
            // AssertSqlSafe: the interpolated fragment is this crate's own
            // EFFECTIVE_DOC_SELECT plus a fixed limit, never caller input.
            let sql = format!(
                "{EFFECTIVE_DOC_SELECT} where d.id > $1::uuid order by d.id asc limit {BACKFILL_PAGE}"
            );
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(&from)
                .fetch_all(pg)
                .await
                .map_err(|e| e.to_string())?;
            let short = rows.len() < BACKFILL_PAGE;
            for r in &rows {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = r.try_get::<String, _>("id").unwrap_or_default();
                let doc = KbDocSync {
                    id: id.clone(),
                    space_id: r.try_get::<Option<String>, _>("spaceId").unwrap_or(None),
                    title: r.try_get::<String, _>("title").unwrap_or_default(),
                    body: r.try_get::<String, _>("body").unwrap_or_default(),
                    visibility: r.try_get::<String, _>("visibility").unwrap_or_default(),
                    official: r.try_get::<bool, _>("official").unwrap_or(false),
                    owner_user_id: r
                        .try_get::<Option<String>, _>("ownerUserId")
                        .unwrap_or(None),
                };
                let _ = sync_kb_doc(pg, qd, ed, &doc).await;
                bump(&mut counts, "kbDocs");
                last = Some(id);
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }

        BackfillSource::ChannelMessages => {
            // Relay summaries ride along — they are messages too.
            let rows = sqlx::query(
                "select m.id::text as id, m.channel_id::text as \"channelId\", \
                       m.author_type as \"authorType\", m.author, m.content, c.name \
                 from channel_messages m join channels c on c.id = m.channel_id \
                 where m.status = 'complete' and m.content <> '' and c.kind <> 'dm' \
                   and m.id > $1::uuid \
                 order by m.id asc limit 100",
            )
            .bind(&from)
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let short = rows.len() < BACKFILL_PAGE;
            for r in &rows {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = r.try_get::<String, _>("id").unwrap_or_default();
                let author_type = r.try_get::<String, _>("authorType").unwrap_or_default();
                let author_id = r.try_get::<String, _>("author").unwrap_or_default();
                let author = if author_type == "agent" {
                    crate::fleet::describe_agent(&author_id).label
                } else {
                    author_id
                };
                let _ = index_activity(
                    pg,
                    qd,
                    ed,
                    &IndexDoc {
                        source_type: "channel".into(),
                        source_id: id.clone(),
                        title: Some(format!(
                            "#{} · {}",
                            r.try_get::<String, _>("name").unwrap_or_default(),
                            author
                        )),
                        text: r.try_get::<String, _>("content").unwrap_or_default(),
                        payload: Some(obj(json!({
                            "channelId": r.try_get::<String, _>("channelId").unwrap_or_default(),
                        }))),
                        href: Some("/channels".into()),
                    },
                )
                .await;
                bump(&mut counts, "channelMessages");
                last = Some(id);
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }

        BackfillSource::Tickets => {
            let rows = sqlx::query(
                "select t.id::text as id, t.board_id::text as \"boardId\", t.title, \
                       t.description \
                 from tasks t \
                 where t.archived_at is null and t.id > $1::uuid \
                 order by t.id asc limit 100",
            )
            .bind(&from)
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let short = rows.len() < BACKFILL_PAGE;
            for r in &rows {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = r.try_get::<String, _>("id").unwrap_or_default();
                let board_id = r.try_get::<String, _>("boardId").unwrap_or_default();
                let _ = index_ticket(
                    pg,
                    qd,
                    ed,
                    &TicketSrc {
                        id: &id,
                        board_id: &board_id,
                        ticket_ref: None,
                        title: &r.try_get::<String, _>("title").unwrap_or_default(),
                        description: r
                            .try_get::<Option<String>, _>("description")
                            .unwrap_or(None)
                            .as_deref(),
                    },
                )
                .await;
                bump(&mut counts, "tickets");
                last = Some(id);
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }

        BackfillSource::Comments => {
            let rows = sqlx::query(
                "select c.id::text as id, c.task_id::text as \"taskId\", \
                       t.board_id::text as \"boardId\", c.author, c.content \
                 from task_comments c join tasks t on t.id = c.task_id \
                 where t.archived_at is null and c.id > $1::uuid \
                 order by c.id asc limit 100",
            )
            .bind(&from)
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let short = rows.len() < BACKFILL_PAGE;
            for r in &rows {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = r.try_get::<String, _>("id").unwrap_or_default();
                let task_id = r.try_get::<String, _>("taskId").unwrap_or_default();
                let board_id = r.try_get::<String, _>("boardId").unwrap_or_default();
                let _ = index_ticket_comment(
                    pg,
                    qd,
                    ed,
                    &CommentSrc {
                        id: &id,
                        task_id: &task_id,
                        board_id: &board_id,
                        ticket_ref: None,
                        author: &r.try_get::<String, _>("author").unwrap_or_default(),
                        content: &r.try_get::<String, _>("content").unwrap_or_default(),
                    },
                )
                .await;
                bump(&mut counts, "comments");
                last = Some(id);
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }

        BackfillSource::PlanTurns => {
            let rows = sqlx::query(
                "select m.id::text as id, m.conversation_id::text as \"planId\", \
                       c.user_id::text as \"ownerId\", c.title, m.content, \
                       coalesce(u.name, u.email) as author \
                 from messages m \
                 join conversations c on c.id = m.conversation_id and c.kind = 'plan' \
                 left join users u on u.id = m.author_user_id \
                 where m.role = 'user' and m.content <> '' and m.id > $1::uuid \
                 order by m.id asc limit 100",
            )
            .bind(&from)
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let short = rows.len() < BACKFILL_PAGE;
            for r in &rows {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = r.try_get::<String, _>("id").unwrap_or_default();
                // TS's `m.title || 'Untitled'`: null and '' both fall through.
                let title = r
                    .try_get::<Option<String>, _>("title")
                    .unwrap_or(None)
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "Untitled".into());
                let _ = index_activity(
                    pg,
                    qd,
                    ed,
                    &IndexDoc {
                        source_type: "plan".into(),
                        source_id: id.clone(),
                        title: Some(format!(
                            "Plan ({title}) · {}",
                            r.try_get::<Option<String>, _>("author")
                                .unwrap_or(None)
                                .as_deref()
                                .unwrap_or("someone")
                        )),
                        text: r.try_get::<String, _>("content").unwrap_or_default(),
                        payload: Some(obj(json!({
                            "planId": r.try_get::<String, _>("planId").unwrap_or_default(),
                            "planOwnerId": r.try_get::<Option<String>, _>("ownerId").unwrap_or(None),
                        }))),
                        href: Some("/plan".into()),
                    },
                )
                .await;
                bump(&mut counts, "planTurns");
                last = Some(id);
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }

        BackfillSource::Artifacts => {
            // PAGED BY ARTIFACT AND NOT BY LINK ROW. The pre-run query was one
            // left join over artifact_links, which yields a row per link — so
            // a page boundary landing inside an artifact with two links would
            // advance the cursor past the artifact and DROP the second link
            // silently. Two queries keep the page artifact-aligned and produce
            // exactly the same set of (artifact, link) pairs the join did.
            let arts = sqlx::query(
                "select a.id::text as id, a.title, a.body, a.visibility, \
                        a.owner_user_id::text as \"ownerId\", a.rag_routing as \"ragRouting\" \
                 from artifacts a \
                 where a.kind = 'doc' and a.body <> '' and a.id > $1::uuid \
                 order by a.id asc limit 100",
            )
            .bind(&from)
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let short = arts.len() < BACKFILL_PAGE;
            let ids: Vec<String> = arts
                .iter()
                .map(|r| r.try_get::<String, _>("id").unwrap_or_default())
                .collect();
            let links = if ids.is_empty() {
                Vec::new()
            } else {
                sqlx::query(
                    "select artifact_id::text as \"artifactId\", target_type as \"targetType\", \
                            target_id::text as \"targetId\" \
                     from artifact_links \
                     where artifact_id = any($1) and target_type in ('plan', 'research')",
                )
                .bind(&ids)
                .fetch_all(pg)
                .await
                .map_err(|e| e.to_string())?
            };

            for a in &arts {
                if signal.is_aborted() {
                    return Ok(page(false, &last, &counts));
                }
                let id = a.try_get::<String, _>("id").unwrap_or_default();
                last = Some(id.clone());
                let title = a.try_get::<String, _>("title").unwrap_or_default();
                let body = a.try_get::<String, _>("body").unwrap_or_default();
                let visibility = a.try_get::<String, _>("visibility").unwrap_or_default();
                let owner_id = a.try_get::<Option<String>, _>("ownerId").unwrap_or(None);
                let rag_routing = a.try_get::<String, _>("ragRouting").unwrap_or_default();
                // Routed artifacts (explicit brain / none) are placed by their
                // routing, not by the activity flows.
                if !rag_routing.is_empty() && rag_routing != "auto" {
                    if let Ok(Some(full)) = crate::artifacts::get_artifact(pg, &id).await {
                        let _ = apply_artifact_routing(pg, qd, ed, &full).await;
                    }
                    bump(&mut counts, "routedArtifacts");
                    continue;
                }
                for l in links
                    .iter()
                    .filter(|l| l.try_get::<String, _>("artifactId").unwrap_or_default() == id)
                {
                    if signal.is_aborted() {
                        return Ok(page(false, &last, &counts));
                    }
                    let target_type = l.try_get::<String, _>("targetType").unwrap_or_default();
                    let target_id = l.try_get::<String, _>("targetId").unwrap_or_default();
                    if target_type == "plan" {
                        let _ = index_activity(
                            pg,
                            qd,
                            ed,
                            &IndexDoc {
                                source_type: "plan-doc".into(),
                                source_id: id.clone(),
                                title: Some(title.clone()),
                                text: format!("{title}\n\n{body}"),
                                payload: Some(obj(json!({
                                    "planId": target_id,
                                    "planOwnerId": owner_id,
                                }))),
                                href: Some(format!("/artifacts?a={id}")),
                            },
                        )
                        .await;
                        bump(&mut counts, "planDocs");
                    } else if target_type == "research" {
                        // Owned research lives in the owner's private brain;
                        // org research in the ambient index, marked orgWide so
                        // scopes match it.
                        let doc = IndexDoc {
                            source_type: "research".into(),
                            source_id: id.clone(),
                            title: Some(title.clone()),
                            text: format!("{title}\n\n{body}"),
                            payload: Some(obj(if owner_id.is_some() {
                                json!({ "runId": target_id })
                            } else {
                                json!({ "runId": target_id, "orgWide": true })
                            })),
                            href: Some(format!("/research/{target_id}")),
                        };
                        if visibility == "private"
                            && let Some(owner) = &owner_id
                        {
                            let _ = index_personal(pg, qd, ed, owner, &doc).await;
                        } else if visibility != "private" {
                            let _ = index_activity(pg, qd, ed, &doc).await;
                        }
                        bump(&mut counts, "research");
                    }
                }
            }
            Ok(Page {
                done: short,
                cursor: last,
                counts,
            })
        }
    }
}

// ── The backfill step, shared by both kinds ──────────────────────────────────

/// ONE PAGE OF PROGRESS, or the reason there is none yet. In this shape
/// rather than as a `StepResult` because BOTH kinds run it: the backfill run
/// IS this, and the reindex run's second phase is this wrapped in a
/// checkpoint that also remembers which collections it rebuilt. Two copies of
/// the paging would be two places for a source to be forgotten.
#[derive(Debug, Clone, PartialEq)]
pub enum BackfillProgress {
    Next {
        checkpoint: BackfillCheckpoint,
        phase: String,
    },
    Done {
        counts: Counts,
    },
    Retry {
        after_ms: u64,
        reason: String,
    },
}

/// The two edges a backfill page touches outside itself, injected so a test
/// drives the whole paging state machine with no Postgres, no Qdrant and no
/// embedding service. Same pattern and same reason as the other defs' seams.
#[derive(Clone)]
pub struct BackfillDeps {
    pub health: Arc<dyn Fn() -> BoxFuture<'static, RagHealth> + Send + Sync>,
    /// The page dep takes an OWNED signal handle: the real page checks it
    /// before every outward call (the way TS threads `signal` into
    /// `indexPage`), so it needs a live receiver of its own, not a borrow of
    /// the step's.
    pub page: PageFn,
}

/// One page of one source, as an injected edge.
pub type PageFn = Arc<
    dyn Fn(
            BackfillSource,
            Option<String>,
            Counts,
            StepSignal,
        ) -> BoxFuture<'static, Result<Page, StepError>>
        + Send
        + Sync,
>;

pub async fn step_backfill(
    prior: Option<BackfillCheckpoint>,
    signal: &StepSignal,
    deps: &BackfillDeps,
) -> Result<BackfillProgress, StepError> {
    let cp = prior.unwrap_or_else(fresh_backfill);

    // Checked at the START OF EACH SOURCE rather than on every page: the probe
    // costs an embedding call, and paying for one per hundred documents to
    // rediscover a service that was up a second ago is a tax on the healthy
    // path.
    if cp.cursor.is_none() {
        let health = (deps.health)().await;
        if !health.qdrant || !health.embeddings {
            return Ok(BackfillProgress::Retry {
                after_ms: SERVICES_DOWN_RETRY_MS,
                reason: format!(
                    "waiting for retrieval (qdrant: {}, embeddings: {}); resumes at {}",
                    if health.qdrant { "up" } else { "down" },
                    if health.embeddings { "up" } else { "down" },
                    cp.source.label(),
                ),
            });
        }
    }

    let page = (deps.page)(
        cp.source,
        cp.cursor.clone(),
        cp.counts.clone(),
        signal.share(),
    )
    .await?;
    if !page.done {
        return Ok(BackfillProgress::Next {
            checkpoint: BackfillCheckpoint {
                source: cp.source,
                cursor: page.cursor,
                counts: page.counts.clone(),
            },
            phase: format!("{}: {} indexed", cp.source.label(), total_of(&page.counts)),
        });
    }

    match next_source(cp.source) {
        None => Ok(BackfillProgress::Done {
            counts: page.counts,
        }),
        Some(after) => Ok(BackfillProgress::Next {
            checkpoint: BackfillCheckpoint {
                source: after,
                cursor: None,
                counts: page.counts,
            },
            phase: after.label().to_string(),
        }),
    }
}

// ── The backfill run ─────────────────────────────────────────────────────────

pub const BACKFILL_KIND: &str = "rag-backfill";

pub async fn step_backfill_run(
    ctx: RunStepContext,
    deps: &BackfillDeps,
) -> Result<StepResult, StepError> {
    let prior = if ctx.checkpoint.is_null() {
        None
    } else {
        Some(
            serde_json::from_value(ctx.checkpoint.clone())
                .map_err(|e| format!("rag-backfill checkpoint does not parse: {e}"))?,
        )
    };
    match step_backfill(prior, &ctx.signal, deps).await? {
        BackfillProgress::Done { counts } => Ok(StepResult::Done {
            result: json!({ "counts": counts }),
        }),
        BackfillProgress::Retry { after_ms, reason } => Ok(StepResult::Retry {
            after: Duration::from_millis(after_ms),
            reason,
        }),
        BackfillProgress::Next { checkpoint, phase } => Ok(StepResult::Next {
            checkpoint: serde_json::to_value(checkpoint).map_err(|e| e.to_string())?,
            phase: Some(phase),
        }),
    }
}

// ── The reindex checkpoint and run ───────────────────────────────────────────

/// The same two words `ReindexStatus.phase` has always used, because the
/// admin panel prints them and the run row is where they come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReindexPhase {
    Rebuilding,
    Backfilling,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReindexCheckpoint {
    pub phase: ReindexPhase,
    /// `rag_collections.id` for every collection already rebuilt. The whole
    /// re-entry argument rests on this list and on the empty transition step
    /// that follows it — see the module header.
    pub rebuilt: Vec<String>,
    /// The dimension the rebuild committed to, kept so the phase line can say
    /// it and so a resumed rebuild cannot silently switch models mid-run.
    /// camelCase on the wire (`embedDim`), as TS's column holds it.
    pub embed_dim: Option<i64>,
    /// Null until the rebuild is finished.
    pub backfill: Option<BackfillCheckpoint>,
}

pub const REINDEX_KIND: &str = "rag-reindex";

fn fresh_reindex() -> ReindexCheckpoint {
    ReindexCheckpoint {
        phase: ReindexPhase::Rebuilding,
        rebuilt: Vec::new(),
        embed_dim: None,
        backfill: None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredCollection {
    pub id: String,
    pub qdrant_name: String,
}

/// Every edge the rebuild touches, injected — `rebuild` above all, because the
/// property worth a test here is "a reclaim does not drop a collection twice"
/// and asserting it against a real Qdrant is not a unit test.
#[derive(Clone)]
pub struct ReindexDeps {
    pub embed_dim: Arc<dyn Fn() -> BoxFuture<'static, Option<i64>> + Send + Sync>,
    pub collections: Arc<
        dyn Fn() -> BoxFuture<'static, Result<Vec<RegisteredCollection>, StepError>> + Send + Sync,
    >,
    /// THE DESTRUCTIVE UNIT, as one verb: drop the collection, recreate it in
    /// the new shape, purge the content-hash bookkeeping so nothing skips the
    /// re-embed, and stamp the registry. One verb rather than four because
    /// they are one step's worth of work and splitting them across steps
    /// would only add windows in which a collection exists in neither shape.
    pub rebuild: RebuildFn,
    pub invalidate: Arc<dyn Fn() + Send + Sync>,
    pub backfill: BackfillDeps,
}

/// One collection rebuilt at one dimension, as an injected edge.
pub type RebuildFn = Arc<
    dyn Fn(RegisteredCollection, i64) -> BoxFuture<'static, Result<(), StepError>> + Send + Sync,
>;

pub async fn step_reindex(
    ctx: RunStepContext,
    deps: &ReindexDeps,
) -> Result<StepResult, StepError> {
    let cp = if ctx.checkpoint.is_null() {
        fresh_reindex()
    } else {
        serde_json::from_value(ctx.checkpoint.clone())
            .map_err(|e| format!("rag-reindex checkpoint does not parse: {e}"))?
    };

    if cp.phase == ReindexPhase::Rebuilding {
        let Some(dim) = (deps.embed_dim)().await else {
            // A `retry`, not an error. The pre-run code threw here and filed
            // the whole reindex as failed for the crime of TEI being down for
            // a minute — and then the admin had to notice it came back and
            // press the button again.
            return Ok(StepResult::Retry {
                after: Duration::from_millis(SERVICES_DOWN_RETRY_MS),
                reason: "waiting for the embedding service before rebuilding".into(),
            });
        };
        if let Some(prev) = cp.embed_dim
            && prev != dim
        {
            // THE EMBEDDING MODEL MOVED UNDER THE RUN. A dimension change is
            // exactly what this run exists to repair, so a change DURING it
            // means every collection rebuilt so far is now in the wrong
            // shape. Start the rebuild over at the new dimension rather than
            // finishing it in two: half a plane at 384 and half at 1024 fails
            // every index and search call against the wrong half, with
            // nothing on /alerts that says which half or why.
            //
            // A pure step — it rewrites the checkpoint and drops nothing — so
            // it is safe to re-enter and cannot itself destroy anything.
            tracing::warn!(
                "{LOG} {}: the embedding dimension changed from {prev} to {dim} mid-rebuild — \
                 rebuilding every collection again",
                ctx.run.id
            );
            let checkpoint = ReindexCheckpoint {
                rebuilt: Vec::new(),
                embed_dim: Some(dim),
                ..cp
            };
            return Ok(StepResult::Next {
                checkpoint: serde_json::to_value(checkpoint).map_err(|e| e.to_string())?,
                phase: Some(format!(
                    "the embedding dimension changed to {dim}; starting the rebuild again"
                )),
            });
        }

        let cols = (deps.collections)().await?;
        let next = cols.iter().find(|c| !cp.rebuilt.contains(&c.id)).cloned();

        let Some(next) = next else {
            // THE TRANSITION, AND IT DOES NOTHING ELSE ON PURPOSE. This is the
            // step that makes a reclaim safe: between "every collection is
            // rebuilt" and "the checkpoint says backfilling" there is no
            // outward effect at all, so a driver that dies in that window
            // re-enters a step that drops nothing. Fold this into the last
            // rebuild step and the window becomes "we rebuilt everything, the
            // write was lost, now we drop a half-refilled index" — see the
            // module header. (`invalidate` is a process-local cache reset,
            // not an outward effect: repeating it costs one status refetch.)
            (deps.invalidate)();
            let checkpoint = ReindexCheckpoint {
                phase: ReindexPhase::Backfilling,
                embed_dim: Some(dim),
                ..cp
            };
            return Ok(StepResult::Next {
                checkpoint: serde_json::to_value(checkpoint).map_err(|e| e.to_string())?,
                phase: Some("refilling from the systems of record".into()),
            });
        };

        if ctx.signal.is_aborted() {
            return Ok(StepResult::Retry {
                after: Duration::ZERO,
                reason: "the driver gave this step up before the rebuild started".into(),
            });
        }
        if ctx.attempt > 0 && !cp.rebuilt.is_empty() {
            tracing::warn!(
                "{LOG} {}: resuming a rebuild at attempt {} — {} of {} collection(s) were \
                 already rebuilt and are NOT dropped again",
                ctx.run.id,
                ctx.attempt,
                cp.rebuilt.len(),
                cols.len()
            );
        }

        // ONE COLLECTION, then the checkpoint.
        (deps.rebuild)(next.clone(), dim).await?;
        (deps.invalidate)();
        let checkpoint = ReindexCheckpoint {
            rebuilt: cp
                .rebuilt
                .iter()
                .cloned()
                .chain(std::iter::once(next.id.clone()))
                .collect(),
            embed_dim: Some(dim),
            ..cp
        };
        return Ok(StepResult::Next {
            checkpoint: serde_json::to_value(checkpoint).map_err(|e| e.to_string())?,
            phase: Some(format!(
                "rebuilt {} ({} of {})",
                next.qdrant_name,
                cp.rebuilt.len() + 1,
                cols.len()
            )),
        });
    }

    match step_backfill(cp.backfill.clone(), &ctx.signal, &deps.backfill).await? {
        BackfillProgress::Done { counts } => Ok(StepResult::Done {
            result: json!({ "counts": counts, "rebuilt": cp.rebuilt.len() }),
        }),
        BackfillProgress::Retry { after_ms, reason } => Ok(StepResult::Retry {
            after: Duration::from_millis(after_ms),
            reason,
        }),
        BackfillProgress::Next { checkpoint, phase } => Ok(StepResult::Next {
            checkpoint: serde_json::to_value(ReindexCheckpoint {
                backfill: Some(checkpoint),
                ..cp
            })
            .map_err(|e| e.to_string())?,
            phase: Some(phase),
        }),
    }
}

// ── Registration, arming, real deps ──────────────────────────────────────────

/// The real step deps. The Rust driver does not exist until the flip arms it,
/// so the deps sit empty until the boot wiring (which owns the AppState)
/// installs them; an unarmed step is the loud refusal in the getter — reached
/// only by a driver armed before its deps.
static ARMED_BACKFILL: OnceLock<BackfillDeps> = OnceLock::new();
static ARMED_REINDEX: OnceLock<ReindexDeps> = OnceLock::new();

pub fn arm_backfill_step(deps: BackfillDeps) {
    let _ = ARMED_BACKFILL.set(deps);
}

pub fn arm_reindex_step(deps: ReindexDeps) {
    let _ = ARMED_REINDEX.set(deps);
}

pub fn real_backfill_deps(state: AppState) -> BackfillDeps {
    BackfillDeps {
        health: {
            let st = state.clone();
            Arc::new(move || {
                let _ = &st;
                Box::pin(async move {
                    let qd = crate::retrieval::qdrant::real_deps();
                    let ed = crate::retrieval::embed::real_deps();
                    rag_health(&qd, &ed).await
                })
            })
        },
        page: {
            let pg = state.pg.clone();
            Arc::new(
                move |source: BackfillSource,
                      cursor: Option<String>,
                      counts: Counts,
                      signal: StepSignal| {
                    let pg = pg.clone();
                    Box::pin(async move {
                        let qd = crate::retrieval::qdrant::real_deps();
                        let ed = crate::retrieval::embed::real_deps();
                        index_page(&pg, &qd, &ed, source, cursor, counts, &signal).await
                    })
                },
            )
        },
    }
}

pub fn real_reindex_deps(state: AppState) -> ReindexDeps {
    ReindexDeps {
        embed_dim: Arc::new(|| {
            Box::pin(async move {
                // What the embedding service serves RIGHT NOW — never a
                // cache, since migration decisions hang on it.
                let ed = crate::retrieval::embed::real_deps();
                embed_info(&ed).await.map(|i| i.dim as i64)
            })
        }),
        collections: {
            let pg = state.pg.clone();
            Arc::new(move || {
                let pg = pg.clone();
                Box::pin(async move {
                    // Ordered so "the next one not yet rebuilt" is a stable
                    // choice across re-entries rather than whatever the
                    // planner returned this time.
                    let rows = sqlx::query_as::<_, (String, String)>(
                        "select id::text, qdrant_name from rag_collections order by id asc",
                    )
                    .fetch_all(&pg)
                    .await
                    .map_err(|e| e.to_string())?;
                    Ok(rows
                        .into_iter()
                        .map(|(id, qdrant_name)| RegisteredCollection { id, qdrant_name })
                        .collect())
                })
            })
        },
        rebuild: {
            let pg = state.pg.clone();
            Arc::new(move |col: RegisteredCollection, dim: i64| {
                let pg = pg.clone();
                Box::pin(async move {
                    let qd = crate::retrieval::qdrant::real_deps();
                    // delete_collection swallows its own failure in both
                    // runtimes (a collection that never existed is fine to
                    // rebuild over); the ensure that follows is the one that
                    // can fail the step.
                    delete_collection(&qd, &col.qdrant_name).await;
                    ensure_hybrid_collection(&qd, &col.qdrant_name, dim).await?;
                    // Old point ids went with the collection; drop the
                    // bookkeeping so content hashes cannot skip the re-embed.
                    sqlx::query("delete from rag_points where collection_id = $1::uuid")
                        .bind(&col.id)
                        .execute(&pg)
                        .await
                        .map_err(|e| e.to_string())?;
                    sqlx::query(
                        "update rag_collections set embed_dim = $2, schema_version = 2 \
                         where id = $1::uuid",
                    )
                    .bind(&col.id)
                    .bind(dim)
                    .execute(&pg)
                    .await
                    .map_err(|e| e.to_string())?;
                    Ok(())
                })
            })
        },
        // The upgrade-status cache drop. The 60s cache crossed with the
        // admin rag route and lives in retrieval/migrate.rs now — one per
        // process, exactly like TS's module state — so this process's run
        // drops this process's cache after every collection it rebuilds: a
        // cached "needs reindex" surviving the rebuild that fixed it is an
        // alarm that trains people to ignore alarms.
        invalidate: Arc::new(crate::retrieval::migrate::invalidate_upgrade_status),
        backfill: real_backfill_deps(state.clone()),
    }
}

/// The registered backfill definition, exactly once per process — TS
/// registers at module load; the Rust equivalent is the first call. The
/// callers are `jobs.rs`'s `try_arm` (the boot list) and the admin rag route
/// (a process that can start a backfill can also be the process a reclaim
/// sweep asks to resume one).
pub fn backfill_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: BACKFILL_KIND.into(),
            label: "Re-index the workspace".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_BACKFILL.get().cloned() else {
                        return Err(
                            "rag-backfill steps are armed with the retrieval plane; this Rust \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    step_backfill_run(ctx, &deps).await
                })
            }),
            // ORG-WIDE WORK WITH NO OWNER AND NO SUBJECT. Only an admin can
            // start it and only an admin should be able to see it stall.
            audience: Arc::new(|_| Authority::Admin { on_board: None }),
            max_step_ms: MAX_STEP_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

pub fn reindex_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: REINDEX_KIND.into(),
            label: "Rebuild and refill the retrieval index".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_REINDEX.get().cloned() else {
                        return Err(
                            "rag-reindex steps are armed with the retrieval plane; this Rust \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    step_reindex(ctx, &deps).await
                })
            }),
            audience: Arc::new(|_| Authority::Admin { on_board: None }),
            max_step_ms: MAX_STEP_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

// ── Starting one ─────────────────────────────────────────────────────────────

/// ONE RUN PER THING, which is rule 6 of the at-least-once checklist:
/// `enqueue` deduplicates nothing above the row, so a caller that retries its
/// own POST — or an admin who presses the button twice — would otherwise
/// start a SECOND backfill doing identical work against the same collections.
///
/// The check and the insert are two statements, so two presses landing in the
/// same millisecond on two instances can still both pass. Named rather than
/// hidden: the residual window is one round trip wide, both runs index the
/// same content-hash-idempotent documents, and closing it properly wants a
/// unique partial index on (kind) for the non-terminal states — a migration,
/// which is not this workflow's to write.
async fn start_once(
    state: &AppState,
    kind: &str,
    def: &'static Arc<RunDefinition>,
) -> Result<(), String> {
    match active_run_of_kind(&state.pg, kind).await {
        Ok(Some(active)) => {
            tracing::info!(
                "{LOG} {kind} is already running as {} (\"{}\") — showing that one rather than \
                 starting a second",
                active.id,
                active.phase
            );
            Ok(())
        }
        Ok(None) => {
            // The enqueue bridge every crossed start verb uses: insert and
            // publish while TS owns the sweep (so its sweep drives the row),
            // drive inline once Rust does.
            let redis = state.redis().await.map_err(|e| {
                format!("the run could not be enqueued: redis is unavailable ({e})")
            })?;
            let realtime = crate::realtime::RealtimeDeps::publish_only(Some(redis.clone()));
            let deps = crate::runs::real_run_deps(state.pg.clone(), redis, realtime);
            enqueue(
                def,
                json!({}),
                EnqueueOptions {
                    owner_user_id: None,
                    subject_type: None,
                    subject_id: None,
                    phase: Some("queued".into()),
                    id: None,
                    start: Some(crate::scheduler::rust_owns_schedule()),
                },
                &deps,
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The admin verbs the admin rag route calls (that route crosses next; the
/// verbs live here beside their definitions, as they do in TS).
pub async fn start_backfill(state: &AppState) -> Result<(), String> {
    start_once(state, BACKFILL_KIND, backfill_run()).await
}

pub async fn start_reindex(state: &AppState) -> Result<(), String> {
    start_once(state, REINDEX_KIND, reindex_run()).await
}

#[cfg(test)]
mod tests {
    // The paging and rebuild state machines, driven with fake deps the way
    // reindex's own TS suite drives them — no Postgres, no Qdrant, no
    // embedding service. What is under test is the CHECKPOINT discipline:
    // what each step drops, what it records, and where a reclaim re-enters.
    //
    // NOTE: like agent_hire's suite, these drive the step fns directly and
    // never call the getters — the getters register kinds in the process-wide
    // registry, and jobs.rs's flip test owns that state.
    use super::*;
    use crate::runs::define::{RunRow, RunState};
    use std::sync::Mutex;

    // ── the harness ───────────────────────────────────────────────────────

    fn minimal_row() -> RunRow {
        RunRow {
            id: "run-1".into(),
            kind: BACKFILL_KIND.into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            state: RunState::Running,
            phase: String::new(),
            checkpoint: Value::Null,
            input: Value::Null,
            result: Value::Null,
            error: None,
            attempt: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: String::new(),
            updated_at: String::new(),
            started_at: None,
            finished_at: None,
        }
    }

    /// A dropped watch sender leaves the channel at its last value — `false`,
    /// never aborted — which is the shape an uncontended run has.
    fn quiet_signal() -> StepSignal {
        let (tx, signal) = StepSignal::channel();
        drop(tx);
        signal
    }

    /// `attempt` is the one field the reclaim tests vary.
    fn ctx(checkpoint: Value, attempt: i32) -> RunStepContext {
        RunStepContext {
            run: minimal_row(),
            input: json!({}),
            checkpoint,
            decision: None,
            signal: quiet_signal(),
            log: Arc::new(|_| {}),
            attempt,
        }
    }

    fn reindex_of(v: &Value) -> ReindexCheckpoint {
        serde_json::from_value(v.clone()).expect("the reindex checkpoint parses back")
    }

    fn source_str(s: BackfillSource) -> String {
        serde_json::to_value(s)
            .expect("the source serializes")
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The corpus: every source has the given number of rows, handed back in
    /// pages of `per_page`. `seen` records each page call as
    /// "source:cursor-or-start" so both the walk order and the resume points
    /// are observable; the tally is keyed by source name (the REAL page keys
    /// by what it indexed — the machine under test only sums it).
    struct Corpus {
        rows: BTreeMap<BackfillSource, usize>,
        per_page: usize,
        seen: Mutex<Vec<String>>,
        health_calls: Mutex<usize>,
        health: RagHealth,
    }

    impl Corpus {
        fn new(rows: BTreeMap<BackfillSource, usize>, per_page: usize) -> Arc<Self> {
            Arc::new(Corpus {
                rows,
                per_page,
                seen: Mutex::new(Vec::new()),
                health_calls: Mutex::new(0),
                health: RagHealth {
                    qdrant: true,
                    embeddings: true,
                },
            })
        }

        fn deps(self: &Arc<Self>) -> BackfillDeps {
            BackfillDeps {
                health: {
                    let this = self.clone();
                    Arc::new(move || {
                        let this = this.clone();
                        Box::pin(async move {
                            *this.health_calls.lock().unwrap() += 1;
                            this.health
                        })
                    })
                },
                page: {
                    let this = self.clone();
                    Arc::new(
                        move |source: BackfillSource,
                              cursor: Option<String>,
                              mut counts: Counts,
                              _signal: StepSignal| {
                            let this = this.clone();
                            Box::pin(async move {
                                this.seen.lock().unwrap().push(format!(
                                    "{}:{}",
                                    source_str(source),
                                    cursor.clone().unwrap_or_else(|| "start".into())
                                ));
                                // The corpus ids are "<source>-<index>"; the
                                // cursor names the last one DONE.
                                let total = this.rows.get(&source).copied().unwrap_or(0);
                                let already = match &cursor {
                                    Some(c) => c
                                        .rsplit_once('-')
                                        .and_then(|(_, n)| n.parse::<usize>().ok())
                                        .map(|n| n + 1)
                                        .unwrap_or(0),
                                    None => 0,
                                };
                                let take = total.saturating_sub(already).min(this.per_page);
                                for _ in already..already + take {
                                    bump(&mut counts, &source_str(source));
                                }
                                let cursor = (take > 0).then(|| {
                                    format!("{}-{}", source_str(source), already + take - 1)
                                });
                                Ok(Page {
                                    done: already + take >= total,
                                    cursor,
                                    counts,
                                })
                            })
                        },
                    )
                },
            }
        }
    }

    // ── backfill ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn walks_every_source_in_order_and_finishes() {
        let mut rows = BTreeMap::new();
        rows.insert(BackfillSource::Collections, 3);
        rows.insert(BackfillSource::KbDocs, 5);
        rows.insert(BackfillSource::ChannelMessages, 0);
        rows.insert(BackfillSource::Tickets, 1);
        rows.insert(BackfillSource::Comments, 0);
        rows.insert(BackfillSource::PlanTurns, 4);
        rows.insert(BackfillSource::Artifacts, 2);
        let corpus = Corpus::new(rows, 2);
        let deps = corpus.deps();

        let signal = quiet_signal();
        let mut cp: Option<BackfillCheckpoint> = None;
        loop {
            match step_backfill(cp.clone(), &signal, &deps).await.unwrap() {
                BackfillProgress::Next { checkpoint, phase } => {
                    // A source boundary announces the NEXT source's label; a
                    // mid-source page announces the running total.
                    assert!(!phase.is_empty());
                    cp = Some(checkpoint);
                }
                BackfillProgress::Done { counts } => {
                    assert_eq!(
                        counts,
                        Counts::from([
                            ("artifacts".to_string(), 2),
                            ("collections".to_string(), 3),
                            ("kb-docs".to_string(), 5),
                            ("plan-turns".to_string(), 4),
                            ("tickets".to_string(), 1),
                        ])
                    );
                    break;
                }
                BackfillProgress::Retry { reason, .. } => {
                    panic!("healthy services never retry: {reason}")
                }
            }
        }

        // The walk order is the source order: the first page of each source
        // appears in BACKFILL_SOURCES order (empty sources included).
        let starts: Vec<String> = corpus
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.ends_with(":start"))
            .cloned()
            .collect();
        let expected: Vec<String> = BACKFILL_SOURCES
            .iter()
            .map(|s| format!("{}:start", source_str(*s)))
            .collect();
        assert_eq!(starts, expected);
    }

    #[tokio::test]
    async fn resumes_at_the_next_unit_and_carries_the_tally() {
        // Two kb-docs pages persisted (2 rows each): the reclaim's first page
        // call starts AFTER the persisted cursor, and the counts it is handed
        // are the ones the checkpoint carried.
        let mut rows = BTreeMap::new();
        rows.insert(BackfillSource::KbDocs, 5);
        let corpus = Corpus::new(rows, 2);
        let deps = corpus.deps();

        let prior = BackfillCheckpoint {
            source: BackfillSource::KbDocs,
            cursor: Some("kb-docs-1".into()),
            counts: Counts::from([("kb-docs".to_string(), 2)]),
        };
        let signal = quiet_signal();
        let res = step_backfill(Some(prior), &signal, &deps).await.unwrap();
        let BackfillProgress::Next { checkpoint, .. } = res else {
            panic!("expected next after the resumed page")
        };
        assert_eq!(checkpoint.cursor.as_deref(), Some("kb-docs-3"));
        assert_eq!(checkpoint.counts.get("kb-docs"), Some(&4));
        // The reclaim's first call is the page AFTER the persisted one.
        assert_eq!(corpus.seen.lock().unwrap()[0], "kb-docs:kb-docs-1");
        // And no health probe: the cursor says the source already started.
        assert_eq!(*corpus.health_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn waits_for_a_dead_qdrant_rather_than_failing() {
        let mut rows = BTreeMap::new();
        rows.insert(BackfillSource::KbDocs, 1);
        let corpus = Corpus::new(rows, 2);
        let mut deps = corpus.deps();
        deps.health = {
            let this = corpus.clone();
            Arc::new(move || {
                let this = this.clone();
                Box::pin(async move {
                    *this.health_calls.lock().unwrap() += 1;
                    RagHealth {
                        qdrant: false,
                        embeddings: true,
                    }
                })
            })
        };

        let signal = quiet_signal();
        let res = step_backfill(None, &signal, &deps).await.unwrap();
        let BackfillProgress::Retry { after_ms, reason } = res else {
            panic!("expected retry")
        };
        assert_eq!(after_ms, SERVICES_DOWN_RETRY_MS);
        assert!(
            reason.contains("qdrant: down") && reason.contains("embeddings: up"),
            "the reason names both services: {reason}"
        );
        assert!(reason.contains("resumes at preparing the collections"));
    }

    #[tokio::test]
    async fn probes_health_at_a_source_boundary_only() {
        // Every source one page: the probes are exactly one per source,
        // never one per page — and the empty sources probe too, because the
        // boundary is the START of a source, not the first row.
        let rows: BTreeMap<BackfillSource, usize> =
            BACKFILL_SOURCES.iter().map(|s| (*s, 1)).collect();
        let corpus = Corpus::new(rows, 100);
        let deps = corpus.deps();

        let signal = quiet_signal();
        let mut cp: Option<BackfillCheckpoint> = None;
        loop {
            match step_backfill(cp.clone(), &signal, &deps).await.unwrap() {
                BackfillProgress::Next { checkpoint, .. } => cp = Some(checkpoint),
                BackfillProgress::Done { .. } => break,
                BackfillProgress::Retry { .. } => panic!("healthy services never retry"),
            }
        }
        assert_eq!(*corpus.health_calls.lock().unwrap(), BACKFILL_SOURCES.len());
    }

    #[test]
    fn the_checkpoints_read_and_write_the_ts_wire_shape() {
        // Rows TS drove before the flip carry these exact bytes; they must
        // parse here, and the checkpoints this side writes must parse there.
        let backfill_ts = r#"{"source":"kb-docs","cursor":"00000001-0000-0000-0000-000000000001","counts":{"kbDocs":100}}"#;
        let cp: BackfillCheckpoint = serde_json::from_str(backfill_ts).unwrap();
        assert_eq!(cp.source, BackfillSource::KbDocs);
        assert_eq!(cp.counts.get("kbDocs"), Some(&100));

        let reindex_ts =
            r#"{"phase":"rebuilding","rebuilt":["c1"],"embedDim":384,"backfill":null}"#;
        let rc: ReindexCheckpoint = serde_json::from_str(reindex_ts).unwrap();
        assert_eq!(rc.phase, ReindexPhase::Rebuilding);
        assert_eq!(rc.embed_dim, Some(384));
        assert_eq!(
            serde_json::to_value(&rc).unwrap(),
            json!({"phase":"rebuilding","rebuilt":["c1"],"embedDim":384,"backfill":null})
        );

        // The phase words the admin panel prints.
        assert_eq!(
            serde_json::to_value(ReindexPhase::Backfilling).unwrap(),
            json!("backfilling")
        );
        // And a fresh reindex checkpoint is the null-checkpoint TS started
        // from, spelled the same.
        assert_eq!(
            serde_json::to_value(fresh_reindex()).unwrap(),
            json!({"phase":"rebuilding","rebuilt":[],"embedDim":null,"backfill":null})
        );
    }

    // ── reindex ───────────────────────────────────────────────────────────

    struct Rebuild {
        cols: Vec<RegisteredCollection>,
        dim: Option<i64>,
        dropped: Mutex<Vec<String>>,
    }

    fn rebuild_fixture(dim: Option<i64>) -> Arc<Rebuild> {
        Arc::new(Rebuild {
            cols: ["c1", "c2", "c3"]
                .into_iter()
                .map(|id| RegisteredCollection {
                    id: id.into(),
                    qdrant_name: format!("talaria_{id}"),
                })
                .collect(),
            dim,
            dropped: Mutex::new(Vec::new()),
        })
    }

    impl Rebuild {
        fn deps(self: &Arc<Self>) -> ReindexDeps {
            ReindexDeps {
                embed_dim: {
                    let this = self.clone();
                    Arc::new(move || {
                        let this = this.clone();
                        Box::pin(async move { this.dim })
                    })
                },
                collections: {
                    let this = self.clone();
                    Arc::new(move || {
                        let this = this.clone();
                        Box::pin(async move { Ok(this.cols.clone()) })
                    })
                },
                rebuild: {
                    let this = self.clone();
                    Arc::new(move |col: RegisteredCollection, _dim: i64| {
                        let this = this.clone();
                        Box::pin(async move {
                            this.dropped.lock().unwrap().push(col.id);
                            Ok(())
                        })
                    })
                },
                invalidate: Arc::new(|| {}),
                backfill: Corpus::new(BTreeMap::new(), 100).deps(),
            }
        }
    }

    #[tokio::test]
    async fn rebuilds_one_per_step_then_flips_then_refills() {
        let rb = rebuild_fixture(Some(1024));
        let deps = rb.deps();
        let mut checkpoint = Value::Null;

        // Three collections: three rebuild steps.
        for i in 1..=3 {
            let res = step_reindex(ctx(checkpoint, 0), &deps).await.unwrap();
            let StepResult::Next {
                checkpoint: cp,
                phase,
            } = res
            else {
                panic!("expected next on rebuild step {i}")
            };
            let rc = reindex_of(&cp);
            assert_eq!(rc.phase, ReindexPhase::Rebuilding);
            assert_eq!(rc.rebuilt.len(), i);
            assert_eq!(rc.embed_dim, Some(1024));
            assert_eq!(
                phase.as_deref(),
                Some(format!("rebuilt talaria_c{i} ({i} of 3)").as_str())
            );
            checkpoint = cp;
        }

        // THE PHASE FLIP IS A STEP OF ITS OWN: it drops nothing and its only
        // effect is the checkpoint's phase.
        let before = rb.dropped.lock().unwrap().len();
        let res = step_reindex(ctx(checkpoint, 0), &deps).await.unwrap();
        let StepResult::Next {
            checkpoint: cp,
            phase,
        } = res
        else {
            panic!("expected the transition step")
        };
        let rc = reindex_of(&cp);
        assert_eq!(rc.phase, ReindexPhase::Backfilling);
        assert_eq!(
            phase.as_deref(),
            Some("refilling from the systems of record")
        );
        assert_eq!(
            rb.dropped.lock().unwrap().len(),
            before,
            "the flip drops nothing"
        );
        checkpoint = cp;

        // The backfill phase finishes over the empty corpus — one step per
        // source boundary (seven of them), then done.
        let mut result = None;
        for _ in 0..20 {
            match step_reindex(ctx(checkpoint, 0), &deps).await.unwrap() {
                StepResult::Done { result: r } => {
                    result = Some(r);
                    break;
                }
                StepResult::Next { checkpoint: cp, .. } => checkpoint = cp,
                other => panic!("the empty corpus never retries: {other:?}"),
            }
        }
        let result = result.expect("expected done after the refill");
        assert_eq!(result["rebuilt"], json!(3));
        assert_eq!(
            rb.dropped
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["c1", "c2", "c3"]
        );
    }

    #[tokio::test]
    async fn a_reclaim_does_not_restart_the_rebuild() {
        let rb = rebuild_fixture(Some(1024));
        let deps = rb.deps();

        // Two steps of progress, then the driver dies. The reclaim
        // (attempt 1) re-enters with the persisted checkpoint and drops
        // ONLY c3.
        let mut checkpoint = Value::Null;
        for _ in 0..2 {
            let res = step_reindex(ctx(checkpoint, 0), &deps).await.unwrap();
            let StepResult::Next { checkpoint: cp, .. } = res else {
                panic!("expected next")
            };
            checkpoint = cp;
        }
        assert_eq!(
            rb.dropped
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["c1", "c2"]
        );

        let res = step_reindex(ctx(checkpoint, 1), &deps).await.unwrap();
        let StepResult::Next {
            checkpoint: cp,
            phase,
        } = res
        else {
            panic!("expected the reclaim to continue the rebuild")
        };
        assert_eq!(phase.as_deref(), Some("rebuilt talaria_c3 (3 of 3)"));
        assert_eq!(reindex_of(&cp).rebuilt, vec!["c1", "c2", "c3"]);
        assert_eq!(
            rb.dropped
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["c1", "c2", "c3"],
            "the reclaim drops only the collection the checkpoint does not name"
        );
    }

    #[tokio::test]
    async fn a_reclaim_during_the_backfill_phase_never_touches_the_rebuild() {
        let rb = rebuild_fixture(Some(1024));
        let deps = rb.deps();
        let checkpoint = serde_json::to_value(ReindexCheckpoint {
            phase: ReindexPhase::Backfilling,
            rebuilt: vec!["c1".into(), "c2".into(), "c3".into()],
            embed_dim: Some(1024),
            backfill: Some(fresh_backfill()),
        })
        .unwrap();

        let res = step_reindex(ctx(checkpoint, 2), &deps).await.unwrap();
        match res {
            StepResult::Next { .. } | StepResult::Done { .. } => {}
            StepResult::Retry { .. } | StepResult::Decide { .. } => {
                panic!("an empty corpus finishes")
            }
        }
        assert!(
            rb.dropped.lock().unwrap().is_empty(),
            "the backfill phase drops nothing"
        );
    }

    #[tokio::test]
    async fn an_embedding_dimension_change_restarts_the_rebuild_as_a_pure_step() {
        let rb = rebuild_fixture(Some(1024));
        let deps = rb.deps();
        let checkpoint = serde_json::to_value(ReindexCheckpoint {
            phase: ReindexPhase::Rebuilding,
            rebuilt: vec!["c1".into(), "c2".into()],
            embed_dim: Some(384),
            backfill: None,
        })
        .unwrap();

        let res = step_reindex(ctx(checkpoint, 0), &deps).await.unwrap();
        let StepResult::Next {
            checkpoint: cp,
            phase,
        } = res
        else {
            panic!("expected the restart step")
        };
        let rc = reindex_of(&cp);
        assert_eq!(rc.rebuilt, Vec::<String>::new());
        assert_eq!(rc.embed_dim, Some(1024));
        assert_eq!(
            phase.as_deref(),
            Some("the embedding dimension changed to 1024; starting the rebuild again")
        );
        assert!(
            rb.dropped.lock().unwrap().is_empty(),
            "the restart drops nothing"
        );
    }

    #[tokio::test]
    async fn waits_for_the_embedding_service_before_rebuilding() {
        let rb = rebuild_fixture(None);
        let deps = rb.deps();

        let res = step_reindex(ctx(Value::Null, 0), &deps).await.unwrap();
        let StepResult::Retry { after, reason } = res else {
            panic!("expected retry")
        };
        assert_eq!(after, Duration::from_millis(SERVICES_DOWN_RETRY_MS));
        assert_eq!(
            reason,
            "waiting for the embedding service before rebuilding"
        );
        assert!(rb.dropped.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn does_not_start_a_rebuild_the_driver_has_given_up_on() {
        let rb = rebuild_fixture(Some(1024));
        let deps = rb.deps();
        let (tx, signal) = StepSignal::channel();
        tx.send(true).unwrap();
        let mut context = ctx(Value::Null, 0);
        context.signal = signal;

        let res = step_reindex(context, &deps).await.unwrap();
        let StepResult::Retry { reason, .. } = res else {
            panic!("expected retry")
        };
        assert_eq!(
            reason,
            "the driver gave this step up before the rebuild started"
        );
        assert!(rb.dropped.lock().unwrap().is_empty());
    }
}
