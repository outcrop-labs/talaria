// The daily board digest: for every board a person set to a daily digest
// (talaria_notify::boards, cadence `digest`), one row a day rolling up what
// happened on it, and one "Your boards" mail for all of them
// (docs/notifications/producers.md "Each board").
//
// WHERE IT RUNS. A board heard as a digest files nothing at fan-out: the
// resolver answers `Delivery::Digest` and the ticket producers drop it. This
// job is the other half. It runs in the daily digest's slot — the
// `digest_config` hour in the person's own zone, a 15-minute tick, a two-hour
// window that never wraps (`in_send_window`) — and reads the board's own
// records at that moment (read time, no per-event queue): `task_activity` for
// what happened, `tasks` for what is due. A change of setting therefore
// applies to the very next digest.
//
// WHAT ONE BOARD'S ROW COUNTS, over the window since the person's last
// digest (the first covers a day; none covers more than two), asking the
// questions the resolver asks of a live event:
//   new        tickets created on the board or moved onto it — a board-wide
//              event (nobody is in a ticket before it exists), so only for a
//              setting over the whole board;
//   moved      tickets that changed column, but not into a done column;
//   completed  tickets moved into one of the board's done columns;
//   comments   comments in the tickets' rooms;
//   due soon / overdue   tickets due within a day of the digest, or past due,
//              and not done — only once due-date reminders have a producer
//              (`BoardEvent::available`), so a digest never carries news
//              nobody could choose.
// Each is counted only when the setting wants that event, over the tickets
// it covers: every ticket for scope `board`, the person's own for `mine`
// (watching, assigned, created — and for comments, having commented
// before). Never counted: the person's own actions (a producer never tells
// the actor), a tracker sync's writes (`MIRROR_ACTORS`: the sync is quiet,
// its tracker already told its people), archived tickets, and the moves,
// completions and comments of a ticket they watch themselves — a deliberate
// watch already carried those as they happened. Assignments and @mentions
// are never digested: what points at a person reaches them at once.
//
// IT DISCLOSES NOTHING THE APP WOULD NOT. Only the boards the person's board
// list shows today (`list_boards`: both access legs, not archived) — a board
// they left since they chose a digest is dormant, as everywhere — and only
// counts and the board's name, the same line its board page shows them.
//
// WHAT IS SENT. Nothing to report, nothing sent. Otherwise:
//   in-app  one `board-digest` row per board (class `board_activity`,
//           `group_key::board_digest`, Realert — a newer digest replaces an
//           unread one rather than stacking). Filed QUIET: a digest never
//           rings, and its email is the mail below, not the missed-activity
//           batcher's (which would mail the same counts a second time).
//   email   one "Your boards" mail listing every board with something to
//           report, when the person's Board activity Email is on and their
//           email frequency is not Never — the matrix row that already
//           governs board mail — through `send_gated_mail`, so the instance
//           switch holds it like every notification mail.
//
// THE LEDGER (`board_digest_state`, one row per person) is written as the
// pass goes, and every write that decides something is a compare-and-set, so
// two passes that overlap (a lease lost mid-run) cannot both file a day or
// both send its mail. The day is claimed before its rows are filed; a row
// that fails to land gives the claim back, and the next tick files the day
// again. The mail is claimed before it is sent; a mail that fails, or is held
// by the instance switch, gives its claim back, and the next tick inside the
// window offers it again without filing the rows twice — past the window, the
// rows were the digest. A pass that dies between claiming a mail and sending
// it loses that one mail, never sends it twice.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::now_ms as wall_ms;
use talaria_boards::list_boards;
use talaria_email::{email_button, email_escape, email_shell};
use talaria_notify::boards::{self as board_prefs, BoardEvent, BoardPref, Scope};
use talaria_notify::{
    ActorKind, Coalesce, GatedMail, GatedSendResult, NOTIFY_SETTINGS_PATH, NotificationInput,
    NotifyActor, NotifyDeps, NotifyEntity, NotifyMeta, add_notification_with, channels_of,
    email_frequency, entity, group_key, instance_base_url, send_gated_mail,
};
use talaria_realtime::RealtimeDeps;
use talaria_state::AppState;
use talaria_statuses::{OFF_BOARD_STATUSES, status_meta};
use talaria_tasks_types::MIRROR_ACTORS;
use talaria_tz::local_moment;

use crate::{
    ListRow, Recipient, RenderedDigest, digest_config, in_send_window, list_html, recipient_zone,
};

/// The kind of the daily row (`KIND_CLASS`: class `board_activity`).
pub const BOARD_DIGEST_KIND: &str = "board-digest";

/// The class whose Email column decides the "Your boards" mail: the matrix
/// row every board mail answers to.
const BOARD_CLASS: &str = "board_activity";

/// The first digest covers a day; a person back after longer than two days
/// gets the last two, never a week of history in one line.
const FIRST_WINDOW_MS: i64 = 24 * 3_600_000;
const LONGEST_WINDOW_MS: i64 = 48 * 3_600_000;
/// "Due soon" is due within this long of the digest.
const DUE_SOON_MS: i64 = 24 * 3_600_000;

// ── What one board's row says ────────────────────────────────────────────────

/// One event's count over the whole board and over the person's own tickets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub board: i64,
    pub mine: i64,
}

impl Tally {
    fn of(self, scope: Scope) -> i64 {
        match scope {
            Scope::Board => self.board,
            Scope::Mine => self.mine,
        }
    }
}

/// What a board's records hold for one person over one window, before their
/// setting picks (`raw_counts`). `created` is board-wide by nature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    pub created: i64,
    pub status: Tally,
    pub completed: Tally,
    pub comment: Tally,
    pub due_soon: Tally,
    pub overdue: Tally,
}

/// One board's line, as the person's setting reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub created: i64,
    pub moved: i64,
    pub completed: i64,
    pub comments: i64,
    pub due_soon: i64,
    pub overdue: i64,
}

impl Counts {
    pub fn is_empty(&self) -> bool {
        *self == Counts::default()
    }

    fn add(&mut self, o: &Counts) {
        self.created += o.created;
        self.moved += o.moved;
        self.completed += o.completed;
        self.comments += o.comments;
        self.due_soon += o.due_soon;
        self.overdue += o.overdue;
    }

    /// "3 new · 8 moved · 2 completed · 11 comments · 1 due soon · 2
    /// overdue" — the parts that are not zero, in that order. Empty for
    /// nothing (which is never sent).
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut push = |n: i64, what: &str| {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        };
        push(self.created, "new");
        push(self.moved, "moved");
        push(self.completed, "completed");
        push(
            self.comments,
            if self.comments == 1 {
                "comment"
            } else {
                "comments"
            },
        );
        push(self.due_soon, "due soon");
        push(self.overdue, "overdue");
        parts.join(" \u{b7} ")
    }
}

/// The person's setting, applied to what the board holds — the resolver's
/// questions, in counts: an event counts only when the setting wants it and
/// its producer exists, over the tickets the setting covers; a new ticket only
/// for a setting over the whole board. Pure.
pub fn pick(p: &BoardPref, raw: &Raw) -> Counts {
    pick_with(p, raw, BoardEvent::available)
}

/// `pick`, with "does this event have a producer" as a parameter, so the
/// tests reach the due-date half before its producer ships.
fn pick_with(p: &BoardPref, raw: &Raw, available: impl Fn(BoardEvent) -> bool) -> Counts {
    let scope = p.scope();
    let heard = |ev: BoardEvent| p.wants(ev) && available(ev);
    let take = |ev: BoardEvent, t: Tally| if heard(ev) { t.of(scope) } else { 0 };
    Counts {
        created: if heard(BoardEvent::Created) && scope == Scope::Board {
            raw.created
        } else {
            0
        },
        moved: take(BoardEvent::Status, raw.status),
        completed: take(BoardEvent::Completed, raw.completed),
        comments: take(BoardEvent::Comment, raw.comment),
        due_soon: take(BoardEvent::Due, raw.due_soon),
        overdue: take(BoardEvent::Due, raw.overdue),
    }
}

/// Where the next digest's window starts: where the last one ended, but
/// never more than two days back; a day back for a person's first. Pure.
pub fn window_from(last_to: Option<i64>, now: i64) -> i64 {
    match last_to {
        Some(t) => t.clamp(now - LONGEST_WINDOW_MS, now),
        None => now - FIRST_WINDOW_MS,
    }
}

// ── The ledger ───────────────────────────────────────────────────────────────

/// One person's row of `board_digest_state`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    /// The local date the in-app rows were filed for.
    filed_on: String,
    /// The window they counted, epoch ms: [from, to).
    from_ms: i64,
    to_ms: i64,
    /// The local date the mail went (or there was none to send); behind
    /// `filed_on` while a mail is owed.
    mailed_on: Option<String>,
}

/// What a tick owes one person today. Pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Today's rows are filed and its mail settled.
    Done,
    /// Today's rows are filed; the mail for their window is still owed.
    Mail { from: i64, to: i64 },
    /// Today's digest: count [from, to), file, then mail.
    File { from: i64, to: i64 },
}

fn step(mark: Option<&Mark>, today: &str, now: i64) -> Step {
    match mark {
        Some(m) if m.filed_on == today => {
            if m.mailed_on.as_deref() == Some(today) {
                Step::Done
            } else {
                Step::Mail {
                    from: m.from_ms,
                    to: m.to_ms,
                }
            }
        }
        _ => Step::File {
            from: window_from(mark.map(|m| m.to_ms), now),
            to: now,
        },
    }
}

async fn read_mark(pg: &PgPool, user_id: &str) -> Result<Option<Mark>, sqlx::Error> {
    let row: Option<(String, i64, i64, Option<String>)> = sqlx::query_as(
        "select filed_on, (trunc(extract(epoch from window_from) * 1000))::bigint, \
                (trunc(extract(epoch from window_to) * 1000))::bigint, mailed_on \
         from board_digest_state where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(filed_on, from_ms, to_ms, mailed_on)| Mark {
        filed_on,
        from_ms,
        to_ms,
        mailed_on,
    }))
}

/// Claim today for this person: the mark is written only when it is not
/// already today's. False when another pass filed today first.
async fn claim_day(pg: &PgPool, user_id: &str, m: &Mark) -> Result<bool, sqlx::Error> {
    let won: Option<String> = sqlx::query_scalar(
        "insert into board_digest_state (user_id, filed_on, window_from, window_to, mailed_on) \
         values ($1::uuid, $2, to_timestamp($3::bigint / 1000.0), \
                 to_timestamp($4::bigint / 1000.0), $5) \
         on conflict (user_id) do update set filed_on = excluded.filed_on, \
           window_from = excluded.window_from, window_to = excluded.window_to, \
           mailed_on = excluded.mailed_on, updated_at = now() \
         where board_digest_state.filed_on is distinct from excluded.filed_on \
         returning user_id::text",
    )
    .bind(user_id)
    .bind(&m.filed_on)
    .bind(m.from_ms)
    .bind(m.to_ms)
    .bind(&m.mailed_on)
    .fetch_optional(pg)
    .await?;
    Ok(won.is_some())
}

/// Give a day's claim back: the mark as it was before (none at all for a
/// person's first), so the next tick files the day again.
async fn unclaim_day(pg: &PgPool, user_id: &str, before: Option<&Mark>) -> Result<(), sqlx::Error> {
    match before {
        Some(m) => write_mark(pg, user_id, m).await,
        None => sqlx::query("delete from board_digest_state where user_id = $1::uuid")
            .bind(user_id)
            .execute(pg)
            .await
            .map(|_| ()),
    }
}

/// Claim the day's mail: settled only by the pass that flips it. False when
/// another pass settled it first, or the day is no longer `day`.
async fn claim_mail(pg: &PgPool, user_id: &str, day: &str) -> Result<bool, sqlx::Error> {
    let won: Option<String> = sqlx::query_scalar(
        "update board_digest_state set mailed_on = filed_on, updated_at = now() \
         where user_id = $1::uuid and filed_on = $2 and mailed_on is distinct from $2 \
         returning user_id::text",
    )
    .bind(user_id)
    .bind(day)
    .fetch_optional(pg)
    .await?;
    Ok(won.is_some())
}

/// Give the mail's claim back: it is owed again.
async fn unclaim_mail(pg: &PgPool, user_id: &str, day: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update board_digest_state set mailed_on = null, updated_at = now() \
         where user_id = $1::uuid and filed_on = $2",
    )
    .bind(user_id)
    .bind(day)
    .execute(pg)
    .await?;
    Ok(())
}

/// The mark as given, unconditionally: only `unclaim_day`, putting back what
/// a pass read before it claimed.
async fn write_mark(pg: &PgPool, user_id: &str, m: &Mark) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into board_digest_state (user_id, filed_on, window_from, window_to, mailed_on) \
         values ($1::uuid, $2, to_timestamp($3::bigint / 1000.0), \
                 to_timestamp($4::bigint / 1000.0), $5) \
         on conflict (user_id) do update set filed_on = excluded.filed_on, \
           window_from = excluded.window_from, window_to = excluded.window_to, \
           mailed_on = excluded.mailed_on, updated_at = now()",
    )
    .bind(user_id)
    .bind(&m.filed_on)
    .bind(m.from_ms)
    .bind(m.to_ms)
    .bind(&m.mailed_on)
    .execute(pg)
    .await?;
    Ok(())
}

// ── Reading a board ──────────────────────────────────────────────────────────

/// One board's raw counts for one person over [from, to), from the board's
/// records. `done_keys` are the board's done columns (`status_meta`, the ones
/// the producers' move event reads); a status line names its column the way
/// every move writes it ("moved to <key>" or "moved to <key> (<why>)", the
/// shape the approvals census reads too).
async fn raw_counts(
    pg: &PgPool,
    board_id: &str,
    person: &Recipient,
    from: i64,
    to: i64,
    done_keys: &[String],
) -> Result<Raw, sqlx::Error> {
    let terminal: Vec<String> = done_keys
        .iter()
        .cloned()
        .chain(OFF_BOARD_STATUSES.iter().map(|s| s.to_string()))
        .collect();
    let mirror: Vec<String> = MIRROR_ACTORS.iter().map(|s| s.to_string()).collect();
    type Row = (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64);
    let r: Row = sqlx::query_as(
        "with t as ( \
           select t.id, t.status, t.due_date, \
                  (t.assignees @> jsonb_build_array('user:' || $2::text) \
                   or lower(t.created_by) = lower($3) \
                   or exists (select 1 from task_watchers w \
                              where w.task_id = t.id and lower(w.watcher) = lower($3))) as mine, \
                  exists (select 1 from task_watchers w \
                          where w.task_id = t.id and lower(w.watcher) = lower($3) \
                            and (w.added_by is null or w.added_by = $2)) as watched \
           from tasks t where t.board_id = $1::uuid and t.archived_at is null \
         ), a as ( \
           select a.task_id, a.type, t.mine, t.watched, \
                  a.type = 'status' and exists (select 1 from unnest($6::text[]) k \
                     where a.description = 'moved to ' || k \
                        or starts_with(a.description, 'moved to ' || k || ' (')) as done, \
                  case when a.type = 'comment' then exists ( \
                     select 1 from channels c join channel_messages m on m.channel_id = c.id \
                     where c.task_id = a.task_id and m.author_type = 'user' \
                       and lower(m.author) = lower($3) and m.created_at < a.created_at) \
                  else false end as commented \
           from task_activity a join t on t.id = a.task_id \
           where a.created_at >= to_timestamp($4::bigint / 1000.0) \
             and a.created_at < to_timestamp($5::bigint / 1000.0) \
             and a.type in ('created', 'moved', 'status', 'comment') \
             and lower(a.actor) <> lower($3) \
             and a.actor <> all($7::text[]) \
         ), d as ( \
           select mine, \
                  due_date >= to_timestamp($5::bigint / 1000.0) as soon \
           from t where due_date is not null and not (status = any($8::text[])) \
             and due_date < to_timestamp(($5::bigint + $9::bigint) / 1000.0) \
         ) \
         select \
           (select count(distinct task_id) from a where type in ('created', 'moved')), \
           (select count(distinct task_id) from a \
             where type = 'status' and not done and not watched), \
           (select count(distinct task_id) from a \
             where type = 'status' and not done and not watched and mine), \
           (select count(distinct task_id) from a \
             where type = 'status' and done and not watched), \
           (select count(distinct task_id) from a \
             where type = 'status' and done and not watched and mine), \
           (select count(*) from a where type = 'comment' and not watched), \
           (select count(*) from a where type = 'comment' and not watched and (mine or commented)), \
           (select count(*) from d where soon), \
           (select count(*) from d where soon and mine), \
           (select count(*) from d where not soon), \
           (select count(*) from d where not soon and mine)",
    )
    .bind(board_id)
    .bind(&person.id)
    .bind(&person.email)
    .bind(from)
    .bind(to)
    .bind(done_keys)
    .bind(&mirror)
    .bind(&terminal)
    .bind(DUE_SOON_MS)
    .fetch_one(pg)
    .await?;
    let (created, sb, sm, cb, cm, mb, mm, db, dm, ob, om) = r;
    Ok(Raw {
        created,
        status: Tally {
            board: sb,
            mine: sm,
        },
        completed: Tally {
            board: cb,
            mine: cm,
        },
        comment: Tally {
            board: mb,
            mine: mm,
        },
        due_soon: Tally {
            board: db,
            mine: dm,
        },
        overdue: Tally {
            board: ob,
            mine: om,
        },
    })
}

/// One board's digest for one person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardDigest {
    pub board_id: String,
    pub name: String,
    pub counts: Counts,
}

/// Every board the person hears as a digest that has something to report
/// over [from, to), in their board list's order. Only the boards that list
/// shows today: access on either leg, not archived.
async fn build(
    pg: &PgPool,
    person: &Recipient,
    from: i64,
    to: i64,
) -> Result<Vec<BoardDigest>, sqlx::Error> {
    let boards = list_boards(pg, &person.id, false).await?;
    let ids: Vec<String> = boards.iter().map(|b| b.id.clone()).collect();
    let heard = board_prefs::digest_boards(pg, &person.id, &ids).await?;
    let mut out = Vec::with_capacity(heard.len());
    for (board_id, pref) in heard {
        let Some(board) = boards.iter().find(|b| b.id == board_id) else {
            continue;
        };
        let meta = status_meta(pg, &board_id).await?;
        let raw = raw_counts(pg, &board_id, person, from, to, &meta.done_keys).await?;
        let counts = pick(&pref, &raw);
        if !counts.is_empty() {
            out.push(BoardDigest {
                board_id,
                name: board.name.clone(),
                counts,
            });
        }
    }
    Ok(out)
}

// ── What is sent ─────────────────────────────────────────────────────────────

/// How the daily row is filed: one open row per board, the newest day's
/// counts replacing an unread one; quiet (no banner, push or batcher mail —
/// the "Your boards" mail is its email). Pure.
pub fn row_meta(board_id: &str) -> NotifyMeta {
    NotifyMeta {
        quiet: true,
        skip_push: true,
        group_key: Some(group_key::board_digest(board_id)),
        coalesce: Coalesce::Realert,
        entity: Some(NotifyEntity::new(entity::BOARD, board_id)),
        actor: Some(NotifyActor {
            kind: ActorKind::System,
            id: BOARD_DIGEST_KIND.into(),
            label: "Daily digest".into(),
        }),
        ..Default::default()
    }
}

/// The row's title. Pure.
fn row_title(name: &str) -> String {
    format!("Daily digest: {name}")
}

/// The in-app half: one row per board. Answers how many landed, or the first
/// failure once every board has been tried — a day with a row missing is not
/// settled (`digest_one` gives its claim back, and the next tick files it
/// again: the rows that did land take it as a re-alert of the same counts).
async fn file_rows(
    notify: &NotifyDeps,
    user_id: &str,
    digests: &[BoardDigest],
) -> Result<usize, sqlx::Error> {
    let mut filed = 0;
    let mut failed: Option<sqlx::Error> = None;
    for d in digests {
        let title = row_title(&d.name);
        let line = d.counts.line();
        let href = format!("/boards/{}", d.board_id);
        let input = NotificationInput {
            kind: BOARD_DIGEST_KIND,
            title: &title,
            body: Some(&line),
            href: Some(&href),
        };
        match add_notification_with(notify, user_id, &input, &row_meta(&d.board_id)).await {
            Ok(_) => filed += 1,
            Err(e) => {
                tracing::error!(
                    "[board-digest] the row for board {} to {user_id} failed: {e}",
                    d.board_id
                );
                failed.get_or_insert(e);
            }
        }
    }
    match failed {
        None => Ok(filed),
        Some(e) => Err(e),
    }
}

/// Whether the person takes the "Your boards" mail: Board activity's Email
/// on, and an email frequency that is not Never (Settings says nothing is
/// mailed while it is, whatever the Email column says). Pure.
pub fn mail_wanted(blob: &Value) -> bool {
    channels_of(blob, BOARD_CLASS).email && email_frequency(blob) != "off"
}

/// The subject carries the content (the digest's rule 2): one board's line,
/// or the boards' lines added up. Pure.
pub fn mail_subject(digests: &[BoardDigest]) -> String {
    match digests {
        [one] => format!("Talaria: {} \u{2014} {}", one.name, one.counts.line()),
        many => {
            let mut total = Counts::default();
            for d in many {
                total.add(&d.counts);
            }
            format!(
                "Talaria: your boards \u{2014} {} on {} boards",
                total.line(),
                many.len()
            )
        }
    }
}

/// The opt-out, spelled as an instruction, for a deployment with no verified
/// domain to link from.
const BOARD_DIGEST_OPT_OUT_STEPS: &str = "To stop this email, set the board back to instant notifications, or turn off Email for Board activity in Settings \u{2192} Notifications.";

/// The "Your boards" mail. `base` is the instance's verified origin, if any.
/// Pure.
pub fn render_board_mail(digests: &[BoardDigest], base: Option<&str>) -> RenderedDigest {
    let url = |path: &str| base.map(|b| format!("{b}{path}"));
    let subject = mail_subject(digests);
    let rows: Vec<ListRow> = digests
        .iter()
        .map(|d| ListRow {
            label: d.name.clone(),
            sub: d.counts.line(),
            href: url(&format!("/boards/{}", d.board_id)),
        })
        .collect();
    let mut body = String::from(
        "<p style=\"margin:0\">What happened on the boards you hear as a daily digest, since your last one.</p>",
    );
    body += &list_html(&rows);
    if let Some(b) = base {
        body += &email_button(&format!("{b}/"), "Open Talaria");
    }
    let settings = url(NOTIFY_SETTINGS_PATH);
    let why = "Your daily board digest \u{2014} for the boards you set to a daily digest, sent only when one of them changed";
    let footer = match &settings {
        Some(s) => format!(
            "{why} \u{b7} <a href=\"{}\" style=\"color:#8a8a84\">change your notification settings</a>",
            email_escape(s)
        ),
        None => format!("{why} \u{b7} {}", email_escape(BOARD_DIGEST_OPT_OUT_STEPS)),
    };
    let mut text: Vec<String> = vec![subject.clone(), String::new()];
    for d in digests {
        text.push(format!("- {}: {}", d.name, d.counts.line()));
        if let Some(u) = url(&format!("/boards/{}", d.board_id)) {
            text.push(format!("  {u}"));
        }
    }
    text.push(String::new());
    text.push("--".into());
    text.push(format!("{why}."));
    text.push(match &settings {
        Some(s) => format!("Change your notification settings: {s}"),
        None => BOARD_DIGEST_OPT_OUT_STEPS.into(),
    });
    RenderedDigest {
        html: email_shell(&email_escape(&subject), &body, &footer),
        text: text.join("\n"),
        subject,
        unsubscribe_url: settings,
    }
}

/// One mail offered to the door.
#[derive(Clone, Debug)]
pub struct BoardMail {
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub unsubscribe_url: Option<String>,
}

pub type SendFuture = Pin<Box<dyn Future<Output = GatedSendResult> + Send>>;
/// The mail door, as a parameter: `send_gated_mail` in production, a
/// recorder in the live tests.
pub type SendBoardMail = Arc<dyn Fn(BoardMail) -> SendFuture + Send + Sync>;

/// What the job touches: the notification edge its rows go through, and the
/// mail door. `now` is a parameter on the pass (the house rule).
#[derive(Clone)]
pub struct BoardDigestDeps {
    pub notify: NotifyDeps,
    pub send: SendBoardMail,
}

pub fn real_board_digest_deps(state: &AppState, realtime: RealtimeDeps) -> BoardDigestDeps {
    let st = state.clone();
    BoardDigestDeps {
        notify: NotifyDeps {
            pg: state.pg.clone(),
            realtime,
        },
        send: Arc::new(move |m: BoardMail| {
            let st = st.clone();
            Box::pin(async move {
                // The secretbox is read when a mail is about to go: keys that
                // failed to load are this person's failed send, retried on
                // the next tick, not a reason to skip the pass.
                match st.secretbox().await {
                    Ok(sb) => {
                        send_gated_mail(
                            &st.pg,
                            &sb,
                            &GatedMail {
                                to: &m.to,
                                subject: &m.subject,
                                html: &m.html,
                                text: Some(&m.text),
                                unsubscribe_url: m.unsubscribe_url.as_deref(),
                                what: "daily board digest",
                            },
                        )
                        .await
                    }
                    Err(e) => GatedSendResult {
                        ok: false,
                        blocked: false,
                        error: Some(e),
                    },
                }
            })
        }),
    }
}

// ── The pass ─────────────────────────────────────────────────────────────────

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardDigestRun {
    /// People with a board that could be a digest.
    pub considered: usize,
    pub outside_window: usize,
    /// Today's digest already filed and mailed (or had no mail to send).
    pub already_done: usize,
    /// Counted today, nothing to report: nothing filed, nothing mailed.
    pub nothing_to_report: usize,
    /// People who got rows today, and the rows.
    pub people_filed: usize,
    pub rows: usize,
    pub mailed: usize,
    /// Built, and refused by the instance email switch: owed, not failed.
    pub mail_held: usize,
    /// The mail door failed: owed, retried on the next tick in the window.
    pub mail_failed: usize,
    /// A person whose boards could not be read or whose mark could not be
    /// written: nothing marked, retried on the next tick.
    pub failed: usize,
}

/// The people the pass considers, by id: the ones with an address.
async fn recipients_by_id(pg: &PgPool, ids: &[String]) -> Result<Vec<Recipient>, sqlx::Error> {
    let rows: Vec<(String, String, Option<Value>, Option<String>)> = sqlx::query_as(
        "select id::text, email, notify_prefs, timezone from users \
         where id = any($1::uuid[]) and email is not null and length(trim(email)) > 0 \
         order by created_at asc",
    )
    .bind(ids)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, prefs, timezone)| Recipient {
            id,
            email,
            prefs,
            timezone,
        })
        .collect())
}

/// One pass over everyone who hears a board as a digest. Err is a FAILED run
/// (who to consider could not be read), so the scheduler's error path says
/// so; one person's failure is counted and logged, and the rest go on.
pub async fn run_board_digest(
    deps: &BoardDigestDeps,
    now_ms: i64,
) -> Result<BoardDigestRun, String> {
    run_board_digest_for(deps, now_ms, None).await
}

/// `run_board_digest`, for these people only when `only` names them — how a
/// live test runs the pass beside other tests' people.
pub async fn run_board_digest_for(
    deps: &BoardDigestDeps,
    now_ms: i64,
    only: Option<&[String]>,
) -> Result<BoardDigestRun, String> {
    let pg = &deps.notify.pg;
    let mut ids = board_prefs::digest_people(pg).await.map_err(|e| {
        format!("board digest pass could not read who hears a board as a digest: {e}")
    })?;
    if let Some(only) = only {
        ids.retain(|id| only.contains(id));
    }
    let mut run = BoardDigestRun {
        considered: ids.len(),
        ..Default::default()
    };
    if ids.is_empty() {
        return Ok(run);
    }
    let people = recipients_by_id(pg, &ids)
        .await
        .map_err(|e| format!("board digest pass could not read its recipients: {e}"))?;
    let config = digest_config(pg).await;
    let base = instance_base_url(pg).await;
    for person in &people {
        let moment = local_moment(&recipient_zone(person, &config), now_ms);
        if !in_send_window(moment.hour as i64, config.hour) {
            run.outside_window += 1;
            continue;
        }
        if let Err(e) = digest_one(
            deps,
            person,
            &moment.date,
            now_ms,
            base.as_deref(),
            &mut run,
        )
        .await
        {
            run.failed += 1;
            tracing::error!("[board-digest] {}: {e}", person.email);
        }
    }
    Ok(run)
}

/// Today's digest for one person: claim the day, file its rows, then the
/// mail — or only the mail a failed send still owes.
async fn digest_one(
    deps: &BoardDigestDeps,
    person: &Recipient,
    today: &str,
    now_ms: i64,
    base: Option<&str>,
    run: &mut BoardDigestRun,
) -> Result<(), sqlx::Error> {
    let pg = &deps.notify.pg;
    let mark = read_mark(pg, &person.id).await?;
    match step(mark.as_ref(), today, now_ms) {
        Step::Done => {
            run.already_done += 1;
            Ok(())
        }
        Step::Mail { from, to } => {
            let digests = build(pg, person, from, to).await?;
            mail(deps, person, today, &digests, base, run).await
        }
        Step::File { from, to } => {
            let digests = build(pg, person, from, to).await?;
            let blob = person.prefs.clone().unwrap_or(Value::Null);
            let owes_mail = !digests.is_empty() && mail_wanted(&blob);
            let claim = Mark {
                filed_on: today.to_string(),
                from_ms: from,
                to_ms: to,
                mailed_on: (!owes_mail).then(|| today.to_string()),
            };
            // Claimed before the rows, so an overlapping pass that read the
            // same old mark files nothing and sends nothing.
            if !claim_day(pg, &person.id, &claim).await? {
                run.already_done += 1;
                return Ok(());
            }
            if digests.is_empty() {
                run.nothing_to_report += 1;
                return Ok(());
            }
            match file_rows(&deps.notify, &person.id, &digests).await {
                Ok(n) => {
                    run.people_filed += 1;
                    run.rows += n;
                }
                Err(e) => {
                    // A row is missing: the day is not done. Give the claim
                    // back (and send no mail about a row that is not there);
                    // the next tick files it again.
                    unclaim_day(pg, &person.id, mark.as_ref()).await?;
                    return Err(e);
                }
            }
            if owes_mail {
                mail(deps, person, today, &digests, base, run).await?;
            }
            Ok(())
        }
    }
}

/// The mail half, for a day whose rows are filed. Claimed before it is sent
/// (an overlapping pass sends nothing), and the claim stands when it went or
/// when there is nothing left to send — the boards changed, or the person
/// turned the mail off since. Given back when the door failed or the instance
/// switch held it: owed again, for the next tick.
async fn mail(
    deps: &BoardDigestDeps,
    person: &Recipient,
    today: &str,
    digests: &[BoardDigest],
    base: Option<&str>,
    run: &mut BoardDigestRun,
) -> Result<(), sqlx::Error> {
    let pg = &deps.notify.pg;
    if !claim_mail(pg, &person.id, today).await? {
        run.already_done += 1;
        return Ok(());
    }
    let blob = person.prefs.clone().unwrap_or(Value::Null);
    if digests.is_empty() || !mail_wanted(&blob) {
        return Ok(());
    }
    let m = render_board_mail(digests, base);
    let sent = (deps.send)(BoardMail {
        to: person.email.clone(),
        subject: m.subject,
        html: m.html,
        text: m.text,
        unsubscribe_url: m.unsubscribe_url,
    })
    .await;
    if sent.ok {
        run.mailed += 1;
        return Ok(());
    }
    if sent.blocked {
        // The instance switch is off: owed, so a switch turned back on inside
        // the window still sends it. Not a failure — nothing was attempted.
        run.mail_held += 1;
    } else {
        run.mail_failed += 1;
        tracing::error!(
            "[board-digest] could not mail {}: {}",
            person.email,
            sent.error.as_deref().unwrap_or("unknown error")
        );
    }
    unclaim_mail(pg, &person.id, today).await
}

pub fn board_digest_job_spec(deps: BoardDigestDeps) -> talaria_scheduler::JobSpec {
    use talaria_scheduler::{JobFn, JobName, JobSpec};
    let run: JobFn = Arc::new(move || {
        let deps = deps.clone();
        Box::pin(async move {
            let r = run_board_digest(&deps, wall_ms()).await?;
            if r.rows == 0
                && r.mailed == 0
                && r.mail_held == 0
                && r.mail_failed == 0
                && r.failed == 0
            {
                return Ok(None);
            }
            let mut line = format!(
                "filed {} board digest row(s) for {} person(s), mailed {}",
                r.rows, r.people_filed, r.mailed
            );
            if r.mail_held > 0 {
                line += &format!(
                    ", {} held back because email delivery is off for this instance",
                    r.mail_held
                );
            }
            if r.mail_failed > 0 {
                line += &format!(
                    ", {} mail(s) failed and wait for the next tick",
                    r.mail_failed
                );
            }
            if r.failed > 0 {
                line += &format!(
                    ", {} person(s) could not be read or recorded and wait for the next tick",
                    r.failed
                );
            }
            Ok(Some(line))
        })
    });
    JobSpec {
        name: JobName::BoardDigest,
        // The daily digest's tick: the window is anchored to a local hour.
        every_ms: 15 * 60_000,
        // After the daily digest's first run, and well past a deploy's
        // settling: a crash-looping instance never reaches a job that mails.
        first_run_delay_ms: Some(90_000),
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run,
    }
}

pub fn register_board_digest_job(deps: BoardDigestDeps) {
    talaria_scheduler::register_job(board_digest_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talaria_notify::boards::{Cadence, Preset};

    fn raw() -> Raw {
        Raw {
            created: 3,
            status: Tally { board: 8, mine: 2 },
            completed: Tally { board: 4, mine: 1 },
            comment: Tally { board: 11, mine: 5 },
            due_soon: Tally { board: 2, mine: 1 },
            overdue: Tally { board: 6, mine: 0 },
        }
    }

    // Every preset, both scopes of Custom, and the event filter: the counts
    // ask the resolver's questions — what the setting wants, over the tickets
    // it covers, a new ticket only over the whole board, and nothing that
    // points at the person (assigned, mention) ever.
    #[test]
    fn the_setting_picks_what_counts() {
        let digest = |p: Preset| BoardPref::preset(p, Cadence::Digest);
        let all = |_: BoardEvent| true;
        assert_eq!(
            pick_with(&digest(Preset::All), &raw(), all),
            Counts {
                created: 3,
                moved: 8,
                completed: 4,
                comments: 11,
                due_soon: 2,
                overdue: 6
            }
        );
        // Participating: the person's own tickets; no new tickets, no due.
        assert_eq!(
            pick_with(&digest(Preset::Participating), &raw(), all),
            Counts {
                moved: 2,
                completed: 1,
                comments: 5,
                ..Counts::default()
            }
        );
        // Mentions & assignments, and Off: nothing is ever digested.
        assert!(pick_with(&digest(Preset::Mentions), &raw(), all).is_empty());
        assert!(pick_with(&digest(Preset::Off), &raw(), all).is_empty());
        // Custom over mine: only its events, over the person's tickets.
        let mine = BoardPref::custom(
            &[BoardEvent::Comment, BoardEvent::Due],
            Scope::Mine,
            Cadence::Digest,
        );
        assert_eq!(
            pick_with(&mine, &raw(), all),
            Counts {
                comments: 5,
                due_soon: 1,
                ..Counts::default()
            }
        );
        // Custom over the board: new tickets count.
        let board = BoardPref::custom(
            &[BoardEvent::Created, BoardEvent::Completed],
            Scope::Board,
            Cadence::Digest,
        );
        assert_eq!(
            pick_with(&board, &raw(), all),
            Counts {
                created: 3,
                completed: 4,
                ..Counts::default()
            }
        );
    }

    // An event with no producer is never counted, so a digest carries only
    // what a person could have chosen: due dates join with their producer.
    #[test]
    fn due_dates_wait_for_their_producer() {
        let all = BoardPref::preset(Preset::All, Cadence::Digest);
        let without_due = pick_with(&all, &raw(), |e| e != BoardEvent::Due);
        assert_eq!((without_due.due_soon, without_due.overdue), (0, 0));
        assert_eq!(without_due.moved, 8);
        let live = pick(&all, &raw());
        let due_live = BoardEvent::Due.available();
        assert_eq!(live.due_soon > 0, due_live);
        assert_eq!(live.overdue > 0, due_live);
    }

    #[test]
    fn the_line_says_only_what_happened() {
        assert_eq!(Counts::default().line(), "");
        assert!(Counts::default().is_empty());
        let c = Counts {
            created: 3,
            moved: 8,
            completed: 2,
            comments: 11,
            due_soon: 1,
            overdue: 2,
        };
        assert_eq!(
            c.line(),
            "3 new \u{b7} 8 moved \u{b7} 2 completed \u{b7} 11 comments \u{b7} 1 due soon \u{b7} 2 overdue"
        );
        let one = Counts {
            comments: 1,
            ..Counts::default()
        };
        assert_eq!(one.line(), "1 comment");
        assert!(!one.is_empty());
    }

    #[test]
    fn the_window_runs_from_the_last_digest_within_two_days() {
        let now = 10 * LONGEST_WINDOW_MS;
        assert_eq!(window_from(None, now), now - FIRST_WINDOW_MS);
        let yesterday = now - 23 * 3_600_000;
        assert_eq!(window_from(Some(yesterday), now), yesterday);
        // A week away is two days, not seven.
        assert_eq!(
            window_from(Some(now - 7 * FIRST_WINDOW_MS), now),
            now - LONGEST_WINDOW_MS
        );
        // A mark from the future (a clock that stepped back) is an empty
        // window, never a negative one.
        assert_eq!(window_from(Some(now + 60_000), now), now);
    }

    // The ledger's day: file once, then only the owed mail, then nothing —
    // and the next day starts where the filed window ended.
    #[test]
    fn a_day_is_filed_once_and_mailed_once() {
        let now = 1_000 * FIRST_WINDOW_MS;
        assert_eq!(
            step(None, "2026-11-20", now),
            Step::File {
                from: now - FIRST_WINDOW_MS,
                to: now
            }
        );
        let owing = Mark {
            filed_on: "2026-11-20".into(),
            from_ms: now - FIRST_WINDOW_MS,
            to_ms: now,
            mailed_on: None,
        };
        // A later tick the same day owes only the mail, for the same window.
        assert_eq!(
            step(Some(&owing), "2026-11-20", now + 900_000),
            Step::Mail {
                from: now - FIRST_WINDOW_MS,
                to: now
            }
        );
        let settled = Mark {
            mailed_on: Some("2026-11-20".into()),
            ..owing.clone()
        };
        assert_eq!(
            step(Some(&settled), "2026-11-20", now + 900_000),
            Step::Done
        );
        // Tomorrow — mailed or not — counts from where today's rows ended.
        let tomorrow = now + FIRST_WINDOW_MS;
        for m in [&owing, &settled] {
            assert_eq!(
                step(Some(m), "2026-11-21", tomorrow),
                Step::File {
                    from: now,
                    to: tomorrow
                }
            );
        }
    }

    // Board activity's Email decides the mail, and Never means never — the
    // promise Settings makes under the Email column.
    #[test]
    fn the_mail_follows_board_activity_email() {
        assert!(!mail_wanted(&Value::Null), "quiet by default");
        let on = json!({ "channels": { "board_activity": { "email": true } } });
        assert!(mail_wanted(&on));
        let never = json!({
            "channels": { "board_activity": { "email": true } },
            "email": { "frequency": "off" },
        });
        assert!(!mail_wanted(&never));
        let other = json!({ "channels": { "work_complete": { "email": true } } });
        assert!(!mail_wanted(&other));
    }

    #[test]
    fn the_row_is_quiet_one_per_board_and_replaces_an_unread_one() {
        let m = row_meta("b1");
        assert!(m.quiet && m.skip_push);
        assert_eq!(m.group_key.as_deref(), Some("board:b1:digest"));
        assert_eq!(m.coalesce, Coalesce::Realert);
        assert_eq!(m.entity, Some(NotifyEntity::new(entity::BOARD, "b1")));
        assert_eq!(m.actor.map(|a| a.kind), Some(ActorKind::System));
        assert_eq!(row_title("Ops"), "Daily digest: Ops");
        // The kind is registered, in the class whose Email decides the mail.
        assert_eq!(
            talaria_notify::notify_class_of(BOARD_DIGEST_KIND),
            BOARD_CLASS
        );
    }

    fn digests() -> Vec<BoardDigest> {
        vec![
            BoardDigest {
                board_id: "b1".into(),
                name: "Ops <core>".into(),
                counts: Counts {
                    created: 2,
                    moved: 3,
                    ..Counts::default()
                },
            },
            BoardDigest {
                board_id: "b2".into(),
                name: "Web".into(),
                counts: Counts {
                    moved: 1,
                    comments: 4,
                    ..Counts::default()
                },
            },
        ]
    }

    #[test]
    fn the_subject_carries_the_counts() {
        let d = digests();
        assert_eq!(
            mail_subject(&d[..1]),
            "Talaria: Ops <core> \u{2014} 2 new \u{b7} 3 moved"
        );
        assert_eq!(
            mail_subject(&d),
            "Talaria: your boards \u{2014} 2 new \u{b7} 4 moved \u{b7} 4 comments on 2 boards"
        );
    }

    #[test]
    fn the_mail_links_each_board_and_its_exit() {
        let m = render_board_mail(&digests(), Some("https://t.example"));
        assert!(m.html.contains("https://t.example/boards/b1"));
        assert!(m.html.contains("https://t.example/boards/b2"));
        // Names are content: escaped in the html, plain in the text.
        assert!(m.html.contains("Ops &lt;core&gt;"));
        assert!(!m.html.contains("Ops <core>"));
        assert!(m.text.contains("- Ops <core>: 2 new \u{b7} 3 moved"));
        assert!(m.text.contains("  https://t.example/boards/b2"));
        assert_eq!(
            m.unsubscribe_url.as_deref(),
            Some("https://t.example/settings/notifications")
        );
        assert!(m.text.contains("https://t.example/settings/notifications"));
        // No verified domain: no links, and the exit spelled as steps in both
        // parts.
        let bare = render_board_mail(&digests(), None);
        assert!(!bare.html.contains("href=\"/boards"));
        assert!(bare.unsubscribe_url.is_none());
        assert!(
            bare.html
                .contains("set the board back to instant notifications")
        );
        assert!(bare.text.contains(BOARD_DIGEST_OPT_OUT_STEPS));
    }
}
