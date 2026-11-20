// Per-board notification settings: what one person hears about one board's
// tickets (docs/notifications/producers.md "Each board"). A person picks a
// preset for a board (All, Participating, Mentions, Off, or Custom events
// over a scope); a board with no setting of theirs follows their own default
// (the reserved `boards` key of `users.notify_prefs`), and a person with no
// default gets the built-in — Participating, which is exactly the audiences
// every ticket producer had before this module existed. So nobody hears
// anything new, and nobody hears less, until they choose.
//
// WHERE IT RUNS. Who gets a row is decided once, at fan-out, by the producers
// (talaria-tasks' ticket fan-out and the four ticket @mention doors through
// talaria-mentions) — the place channel levels are decided today. Nothing at
// send time re-reads it: the deferred push and the mail batcher keep reading
// the class matrix, as they do for a channel level. The row, once filed, is
// the record.
//
// The precedence, in order (`resolve`):
//   1. Needs-you rows never come here: approvals (ticket review included),
//      judge escalations, workchain turns and blocked agents are filed by
//      their own producers, whatever a board says.
//   2. A deliberate watch on the ticket (the watcher added it themselves, or a
//      watch from before anyone recorded who added it) — its ticket's events
//      reach them even on a board set to Off.
//   3. The person's setting for this board.
//   4. Their default for every board.
//   5. The built-in: Participating.
//   Then the class matrix, DND, working hours, the instance switches and the
//   email frequency apply to the row exactly as to any other.
//
// CONFINED. A personal setting must never change what anyone else receives,
// so only the per-recipient fan-outs may ask it: scripts/check-invariants.mjs
// allows `talaria_notify::boards` and the `board_notify_prefs` table in an
// allow-listed set of files and nowhere else (no broadcast — a board event to
// apps, a channel or room post, a realtime frame — can read it). That is why
// lib.rs does not glob-re-export this module.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use sqlx::PgPool;
use talaria_body::{enum_member, enum_msg, object_msg, zod_type_name};

/// The reserved key of `users.notify_prefs` that holds a person's default
/// for every board (`prefs::merge_prefs` writes it, `default_of` reads it).
pub const BOARDS_KEY: &str = "boards";

/// The features this module serves in `GET /api/notifications` (I-23):
/// `board-prefs` (the settings and the ticket producers that obey them) and
/// `task-due` (the due-date reminders, talaria-tasks `due`). `board-digest`
/// joins when its producer ships.
pub const FEATURES: &[&str] = &["board-prefs", "task-due"];

/// The daily board digest has a producer (`board-digest`). Until then the
/// cadence is listed but refused.
const DIGEST_SERVED: bool = false;

// ── The vocabulary ───────────────────────────────────────────────────────────

/// What happened on a board, as a person chooses to hear it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoardEvent {
    /// A ticket appeared on the board (created there, or moved onto it).
    Created,
    /// The person was assigned a ticket.
    Assigned,
    /// A ticket changed column (any move but one into a done column).
    Status,
    /// A comment or reply on a ticket.
    Comment,
    /// The person was @mentioned on a ticket.
    Mention,
    /// A ticket is due within a day, or overdue (the `task-due` job,
    /// talaria-tasks `due`). Opt-in: only All and a Custom set name it.
    Due,
    /// A ticket moved into one of the board's done columns.
    Completed,
}

impl BoardEvent {
    /// Every event, in registry (wire) order.
    pub const ALL: [BoardEvent; 7] = [
        BoardEvent::Created,
        BoardEvent::Assigned,
        BoardEvent::Status,
        BoardEvent::Comment,
        BoardEvent::Mention,
        BoardEvent::Due,
        BoardEvent::Completed,
    ];

    /// The stored and wire spelling.
    pub fn id(self) -> &'static str {
        match self {
            BoardEvent::Created => "created",
            BoardEvent::Assigned => "assigned",
            BoardEvent::Status => "status",
            BoardEvent::Comment => "comment",
            BoardEvent::Mention => "mention",
            BoardEvent::Due => "due",
            BoardEvent::Completed => "completed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BoardEvent::Created => "New ticket on the board",
            BoardEvent::Assigned => "Assigned to me",
            BoardEvent::Status => "Status change",
            BoardEvent::Comment => "Comment or reply",
            BoardEvent::Mention => "Mention",
            BoardEvent::Due => "Due soon or overdue",
            BoardEvent::Completed => "Ticket completed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id() == s)
    }

    /// Board-wide by nature: nobody is in a ticket before it exists, so
    /// "new ticket" only ever reaches a person who hears the whole board.
    pub fn board_wide_only(self) -> bool {
        self == BoardEvent::Created
    }

    /// Whether a person may choose it yet (its producer exists). Every event
    /// has one since the due-date reminders shipped; the wire keeps the
    /// member so a later event can be listed before its producer lands.
    pub fn available(self) -> bool {
        true
    }
}

/// Which tickets on the board a setting covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Tickets the person is in: each producer's own audience — watchers,
    /// assigned people, the creator, and earlier commenters for comments.
    Mine,
    /// Every ticket on the board.
    Board,
}

impl Scope {
    pub fn id(self) -> &'static str {
        match self {
            Scope::Mine => "mine",
            Scope::Board => "board",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "mine" => Some(Scope::Mine),
            "board" => Some(Scope::Board),
            _ => None,
        }
    }
}

const SCOPE_IDS: &[&str] = &["mine", "board"];

/// When the rows come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// As it happens.
    Instant,
    /// Rolled up once a day (no producer yet: `DIGEST_SERVED`).
    Digest,
}

impl Cadence {
    pub fn id(self) -> &'static str {
        match self {
            Cadence::Instant => "instant",
            Cadence::Digest => "digest",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Cadence::Instant => "Instant",
            Cadence::Digest => "Daily digest",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "instant" => Some(Cadence::Instant),
            "digest" => Some(Cadence::Digest),
            _ => None,
        }
    }
    fn available(self) -> bool {
        self == Cadence::Instant || DIGEST_SERVED
    }
}

const CADENCE_IDS: &[&str] = &["instant", "digest"];

/// A named set of events over a scope. Stored as its name, expanded when
/// read — never stored expanded, so a preset means the same thing for
/// everyone who chose it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Every event, every ticket on the board.
    All,
    /// Tickets you're in: assigned, status, comment, mention, completed. The
    /// built-in, and exactly what the producers filed before boards had a
    /// setting.
    Participating,
    /// Only what points at you: assignments and @mentions.
    Mentions,
    /// Nothing from this board — assignments and @mentions included (a
    /// channel set to Nothing silences @mentions too). Needs-you rows still
    /// come; a ticket you watch yourself still reaches you.
    Off,
    /// The stored events and scope.
    Custom,
}

impl Preset {
    /// Every preset, in wire order.
    pub const ALL: [Preset; 5] = [
        Preset::All,
        Preset::Participating,
        Preset::Mentions,
        Preset::Off,
        Preset::Custom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Preset::All => "all",
            Preset::Participating => "participating",
            Preset::Mentions => "mentions",
            Preset::Off => "off",
            Preset::Custom => "custom",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::All => "All activity",
            Preset::Participating => "Participating",
            Preset::Mentions => "Mentions & assignments",
            Preset::Off => "Off",
            Preset::Custom => "Custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == s)
    }

    /// What a named preset expands to; None for Custom (its own events).
    fn expansion(self) -> Option<(&'static [BoardEvent], Scope)> {
        match self {
            Preset::All => Some((EVERY_EVENT, Scope::Board)),
            Preset::Participating => Some((PARTICIPATING, Scope::Mine)),
            Preset::Mentions => Some((MENTIONS, Scope::Mine)),
            Preset::Off => Some((NO_EVENTS, Scope::Mine)),
            Preset::Custom => None,
        }
    }
}

const EVERY_EVENT: &[BoardEvent] = &BoardEvent::ALL;
const PARTICIPATING: &[BoardEvent] = &[
    BoardEvent::Assigned,
    BoardEvent::Status,
    BoardEvent::Comment,
    BoardEvent::Mention,
    BoardEvent::Completed,
];
const MENTIONS: &[BoardEvent] = &[BoardEvent::Assigned, BoardEvent::Mention];
const NO_EVENTS: &[BoardEvent] = &[];

const PRESET_IDS: &[&str] = &["all", "participating", "mentions", "off", "custom"];

/// One person's setting for one board (or their default for every board).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardPref {
    pub preset: Preset,
    /// Custom only: the chosen events, in registry order.
    custom_events: Vec<BoardEvent>,
    /// Custom only: the chosen scope.
    custom_scope: Scope,
    pub cadence: Cadence,
}

/// Where a person's effective setting for a board came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Their own setting for this board.
    Board,
    /// Their default for every board.
    Default,
    /// Neither: the built-in.
    Builtin,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::Board => "board",
            Source::Default => "default",
            Source::Builtin => "builtin",
        }
    }
}

impl BoardPref {
    /// The built-in: Participating, as it happens. Today's audiences.
    pub fn builtin() -> Self {
        Self::preset(Preset::Participating, Cadence::Instant)
    }

    /// A named preset (not Custom).
    pub fn preset(preset: Preset, cadence: Cadence) -> Self {
        Self {
            preset,
            custom_events: Vec::new(),
            custom_scope: Scope::Mine,
            cadence,
        }
    }

    /// A custom setting: these events (registry order, no repeats) over this
    /// scope.
    pub fn custom(events: &[BoardEvent], scope: Scope, cadence: Cadence) -> Self {
        Self {
            preset: Preset::Custom,
            custom_events: BoardEvent::ALL
                .into_iter()
                .filter(|e| events.contains(e))
                .collect(),
            custom_scope: scope,
            cadence,
        }
    }

    /// The events it covers, expanded.
    pub fn events(&self) -> Vec<BoardEvent> {
        match self.preset.expansion() {
            Some((events, _)) => events.to_vec(),
            None => self.custom_events.clone(),
        }
    }

    pub fn scope(&self) -> Scope {
        match self.preset.expansion() {
            Some((_, scope)) => scope,
            None => self.custom_scope,
        }
    }

    pub fn wants(&self, ev: BoardEvent) -> bool {
        self.preset != Preset::Off && self.events().contains(&ev)
    }

    /// The stored shape (the `boards` default; a table row holds the same
    /// fields): the preset, the cadence, and — Custom only — the events and
    /// the scope.
    pub fn to_stored(&self) -> Value {
        let mut m = Map::new();
        m.insert("preset".into(), json!(self.preset.id()));
        if self.preset == Preset::Custom {
            m.insert(
                "events".into(),
                json!(
                    self.custom_events
                        .iter()
                        .map(|e| e.id())
                        .collect::<Vec<_>>()
                ),
            );
            m.insert("scope".into(), json!(self.custom_scope.id()));
        }
        m.insert("cadence".into(), json!(self.cadence.id()));
        Value::Object(m)
    }

    /// The effective shape the wire serves: always expanded.
    pub fn to_wire(&self) -> Value {
        json!({
            "preset": self.preset.id(),
            "events": self.events().iter().map(|e| e.id()).collect::<Vec<_>>(),
            "scope": self.scope().id(),
            "cadence": self.cadence.id(),
        })
    }

    /// A stored value back (the `boards` default, or a row's columns). A
    /// shape this binary does not understand — a hand edit, a value a later
    /// release wrote — reads as None, which falls through to the next layer;
    /// it never reads as silence.
    fn from_parts(
        preset: &str,
        events: Option<&[String]>,
        scope: Option<&str>,
        cadence: Option<&str>,
    ) -> Option<Self> {
        let preset = Preset::parse(preset)?;
        let cadence = match cadence {
            None => Cadence::Instant,
            Some(c) => Cadence::parse(c)?,
        };
        if preset != Preset::Custom {
            return Some(Self::preset(preset, cadence));
        }
        let mut chosen = Vec::new();
        for e in events? {
            chosen.push(BoardEvent::parse(e)?);
        }
        let scope = Scope::parse(scope.unwrap_or("mine"))?;
        Some(Self::custom(&chosen, scope, cadence))
    }

    /// The stored JSON shape back (`to_stored`'s inverse).
    pub fn from_stored(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let events: Option<Vec<String>> = match o.get("events") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|e| e.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>(),
            _ => None,
        };
        Self::from_parts(
            o.get("preset")?.as_str()?,
            events.as_deref(),
            o.get("scope").and_then(Value::as_str),
            o.get("cadence").and_then(Value::as_str),
        )
    }
}

/// A person's default for every board, from their prefs blob.
pub fn default_of(blob: &Value) -> Option<BoardPref> {
    blob.get(BOARDS_KEY).and_then(BoardPref::from_stored)
}

/// The effective setting and where it came from: the board's own row, else
/// the person's default, else the built-in.
pub fn effective(row: Option<&BoardPref>, default: Option<&BoardPref>) -> (BoardPref, Source) {
    match (row, default) {
        (Some(r), _) => (r.clone(), Source::Board),
        (None, Some(d)) => (d.clone(), Source::Default),
        (None, None) => (BoardPref::builtin(), Source::Builtin),
    }
}

// ── The resolver ─────────────────────────────────────────────────────────────

/// What one person gets for one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Nothing.
    Skip,
    /// The producer's own per-ticket row, as before boards had a setting.
    Ticket,
    /// One `board-activity` row per board, counting quietly (`board_activity`
    /// class): an event on a ticket they are not in, on a board they hear in
    /// full.
    BoardActivity,
    /// Held for the daily board digest (no producer yet; nothing is filed).
    Digest,
}

/// One person a ticket event could reach, and how they relate to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub user_id: String,
    /// They watch the ticket, and the watch is their own (or predates the
    /// record of who added it).
    pub deliberate_watch: bool,
    /// In the producer's own audience for this event: a watcher, an assigned
    /// person, the creator, an earlier commenter.
    pub involved: bool,
    /// The event points at them: they were assigned, or @mentioned.
    pub addressed: bool,
}

impl Candidate {
    /// In the event's own audience.
    pub fn involved(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            deliberate_watch: false,
            involved: true,
            addressed: false,
        }
    }

    /// The event points at them.
    pub fn addressed(user_id: impl Into<String>) -> Self {
        Self {
            addressed: true,
            ..Self::involved(user_id)
        }
    }

    /// On the board, not in this ticket.
    pub fn bystander(user_id: impl Into<String>) -> Self {
        Self {
            involved: false,
            ..Self::involved(user_id)
        }
    }

    /// The same person seen twice (a watcher who is also assigned): every
    /// relationship counts.
    pub fn merge(&mut self, other: &Candidate) {
        self.deliberate_watch |= other.deliberate_watch;
        self.involved |= other.involved;
        self.addressed |= other.addressed;
    }
}

/// THE decision, for one person and one event. Pure; the written order is
/// the module comment's.
pub fn resolve(p: &BoardPref, ev: BoardEvent, c: &Candidate) -> Delivery {
    // A ticket they watch on purpose reaches them, Off or not — the events a
    // watch always carried. Due is not one: a watch turns on nothing new.
    if c.deliberate_watch
        && matches!(
            ev,
            BoardEvent::Status
                | BoardEvent::Comment
                | BoardEvent::Completed
                | BoardEvent::Mention
                | BoardEvent::Assigned
        )
    {
        return Delivery::Ticket;
    }
    if !p.wants(ev) {
        return Delivery::Skip;
    }
    // What points at them never waits for a digest.
    if c.addressed {
        return Delivery::Ticket;
    }
    let digest = p.cadence == Cadence::Digest;
    if c.involved {
        return if digest {
            Delivery::Digest
        } else {
            Delivery::Ticket
        };
    }
    if p.scope() == Scope::Board {
        return if digest {
            Delivery::Digest
        } else {
            Delivery::BoardActivity
        };
    }
    Delivery::Skip
}

/// Candidates folded by person (flags merged), first appearance kept. An
/// org-wide board's whole membership can be a candidate list, so by index.
fn fold(candidates: &[Candidate]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::with_capacity(candidates.len());
    let mut at: HashMap<&str, usize> = HashMap::with_capacity(candidates.len());
    for c in candidates {
        match at.get(c.user_id.as_str()) {
            Some(&i) => out[i].merge(c),
            None => {
                at.insert(c.user_id.as_str(), out.len());
                out.push(c.clone());
            }
        }
    }
    out
}

/// Each candidate's delivery for one event on one board, from ONE read of
/// their settings for it and their defaults. A read that fails is logged
/// and answers the built-in for everyone — the audiences the producers had
/// before boards had a setting — never silence.
///
/// Not for an event whose built-in IS silence: no built-in names `Due`, so
/// for due dates that fallback would tell nobody. The `task-due` scan asks
/// `try_listeners` and keeps the ticket for its next pass instead.
pub async fn listeners(
    pg: &PgPool,
    board_id: &str,
    ev: BoardEvent,
    candidates: &[Candidate],
) -> Vec<(Candidate, Delivery)> {
    match try_listeners(pg, board_id, ev, candidates).await {
        Ok(decided) => decided,
        Err(e) => {
            tracing::error!("[notifications] board settings read failed: {e}");
            decide(fold(candidates), &HashMap::new(), ev)
        }
    }
}

/// `listeners`, with a read that fails answered as the caller's error rather
/// than the built-in.
pub async fn try_listeners(
    pg: &PgPool,
    board_id: &str,
    ev: BoardEvent,
    candidates: &[Candidate],
) -> Result<Vec<(Candidate, Delivery)>, sqlx::Error> {
    let folded = fold(candidates);
    if folded.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = folded.iter().map(|c| c.user_id.clone()).collect();
    let prefs = read_prefs(pg, board_id, &ids).await?;
    Ok(decide(folded, &prefs, ev))
}

/// Each folded candidate's delivery from the settings read for them (a
/// person missing from `prefs` has the built-in).
fn decide(
    folded: Vec<Candidate>,
    prefs: &HashMap<String, (Option<BoardPref>, Option<BoardPref>)>,
    ev: BoardEvent,
) -> Vec<(Candidate, Delivery)> {
    folded
        .into_iter()
        .map(|c| {
            let (row, default) = prefs.get(&c.user_id).cloned().unwrap_or_default();
            let (p, _) = effective(row.as_ref(), default.as_ref());
            let d = resolve(&p, ev, &c);
            (c, d)
        })
        .collect()
}

type PrefRow = (
    String,
    Option<String>,
    Option<Vec<String>>,
    Option<String>,
    Option<String>,
    Option<Value>,
);

/// user id → (their row for this board, their default), for these people.
async fn read_prefs(
    pg: &PgPool,
    board_id: &str,
    user_ids: &[String],
) -> Result<HashMap<String, (Option<BoardPref>, Option<BoardPref>)>, sqlx::Error> {
    let rows: Vec<PrefRow> = sqlx::query_as(
        "select u.id::text, p.preset, p.events, p.scope, p.cadence, u.notify_prefs -> 'boards' \
         from users u \
         left join board_notify_prefs p on p.user_id = u.id and p.board_id = $1::uuid \
         where u.id = any($2::uuid[])",
    )
    .bind(board_id)
    .bind(user_ids)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, preset, events, scope, cadence, default)| {
            let row = preset.and_then(|p| {
                BoardPref::from_parts(&p, events.as_deref(), scope.as_deref(), cadence.as_deref())
            });
            let default = default.as_ref().and_then(BoardPref::from_stored);
            (id, (row, default))
        })
        .collect())
}

/// Could anybody hear this whole board? True when a setting for it, or
/// anybody's default for every board, covers scope `board` — All, or Custom
/// over the board. A producer asks before it reads a board's whole
/// membership, so a workspace where nobody chose one pays this one query and
/// no more. A read that fails answers true: the full read is the safe side.
///
/// Two proofs, because no one test can run both: `hears_board` below is the
/// same predicate over a stored setting, and its unit test pins it to
/// `BoardPref::scope` for every setting; the SQL itself is exercised leg by
/// leg live (`board_notify_live::the_board_wide_probe_hears_each_leg` — a
/// custom row over the board, then a default of All on a board where nobody
/// has a row).
pub async fn board_wide_possible(pg: &PgPool, board_id: &str) -> bool {
    match sqlx::query_scalar::<_, bool>(
        "select exists (select 1 from board_notify_prefs \
                         where board_id = $1::uuid and (preset = 'all' or scope = 'board')) \
             or exists (select 1 from users \
                         where notify_prefs -> 'boards' ->> 'preset' = 'all' \
                            or notify_prefs -> 'boards' ->> 'scope' = 'board')",
    )
    .bind(board_id)
    .fetch_one(pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] board-wide settings probe failed: {e}");
            true
        }
    }
}

/// The stored shape `board_wide_possible` matches: preset `all`, or a stored
/// scope of `board` (only Custom stores one). Pure; pinned against
/// `BoardPref::scope` for every preset.
#[cfg(test)]
fn hears_board(stored: &Value) -> bool {
    stored.get("preset").and_then(Value::as_str) == Some("all")
        || stored.get("scope").and_then(Value::as_str) == Some("board")
}

/// Could anybody hear a due-date reminder from this board? True when a
/// setting for it, or anybody's default for every board, names `due` — All,
/// or a Custom set holding it. Due reminders are opt-in (no preset but All
/// names them), so the `task-due` scan asks this before it reads a ticket's
/// audience, and a workspace where nobody opted in pays this one query per
/// board and no more. A read that fails answers true: the full read is the
/// safe side. `hears_due` below is the same predicate over a stored setting,
/// pinned to `BoardPref::wants(Due)` for every setting; the SQL is exercised
/// leg by leg live (`task_due_live`: a Custom row naming due throughout, and
/// in `a_default_or_all_opts_in_as_well` a default naming due on a board where
/// nobody has a row, then a board row of All).
pub async fn due_possible(pg: &PgPool, board_id: &str) -> bool {
    match sqlx::query_scalar::<_, bool>(
        "select exists (select 1 from board_notify_prefs \
                         where board_id = $1::uuid and (preset = 'all' or 'due' = any(events))) \
             or exists (select 1 from users \
                         where notify_prefs -> 'boards' ->> 'preset' = 'all' \
                            or notify_prefs -> 'boards' -> 'events' @> '[\"due\"]'::jsonb)",
    )
    .bind(board_id)
    .fetch_one(pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] due-reminder settings probe failed: {e}");
            true
        }
    }
}

/// The stored shape `due_possible` matches: preset `all`, or stored events
/// naming `due` (only Custom stores events). Pure; pinned against
/// `BoardPref::wants(Due)` for every setting.
#[cfg(test)]
fn hears_due(stored: &Value) -> bool {
    stored.get("preset").and_then(Value::as_str) == Some("all")
        || stored
            .get("events")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|e| e.as_str() == Some("due")))
}

/// Who among `user_ids` — the people a ticket's text @mentions — gets their
/// mention row, by their setting for the ticket's board. The deliberate-watch
/// flag is read for exactly these people: a watch can only keep a mention a
/// board would drop, never add a recipient the caller did not already name.
/// A read that fails keeps everyone (the built-in files every mention).
pub async fn mention_listeners(
    pg: &PgPool,
    board_id: &str,
    task_id: &str,
    user_ids: &[String],
) -> HashSet<String> {
    if user_ids.is_empty() {
        return HashSet::new();
    }
    let watching: HashSet<String> = match sqlx::query_scalar::<_, String>(
        "select u.id::text from task_watchers w \
         join users u on lower(u.email) = lower(w.watcher) \
         where w.task_id = $1::uuid and u.id = any($2::uuid[]) \
           and (w.added_by is null or w.added_by = u.id::text)",
    )
    .bind(task_id)
    .bind(user_ids)
    .fetch_all(pg)
    .await
    {
        Ok(v) => v.into_iter().collect(),
        Err(e) => {
            tracing::error!("[notifications] mention watch read failed: {e}");
            HashSet::new()
        }
    };
    let candidates: Vec<Candidate> = user_ids
        .iter()
        .map(|u| Candidate {
            deliberate_watch: watching.contains(u),
            ..Candidate::addressed(u.as_str())
        })
        .collect();
    listeners(pg, board_id, BoardEvent::Mention, &candidates)
        .await
        .into_iter()
        .filter(|(_, d)| *d == Delivery::Ticket)
        .map(|(c, _)| c.user_id)
        .collect()
}

// ── The store ────────────────────────────────────────────────────────────────

/// The caller's default for every board, if they set one.
pub async fn default_for(pg: &PgPool, user_id: &str) -> Result<Option<BoardPref>, sqlx::Error> {
    let v: Option<Option<Value>> = sqlx::query_scalar::<_, Option<Value>>(
        "select notify_prefs -> 'boards' from users where id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(v.flatten().as_ref().and_then(BoardPref::from_stored))
}

/// The caller's own setting for one board, if they made one.
pub async fn stored(
    pg: &PgPool,
    user_id: &str,
    board_id: &str,
) -> Result<Option<BoardPref>, sqlx::Error> {
    Ok(stored_for_boards(pg, user_id, &[board_id.to_string()])
        .await?
        .remove(board_id))
}

/// The caller's own settings for these boards: board id → setting.
pub async fn stored_for_boards(
    pg: &PgPool,
    user_id: &str,
    board_ids: &[String],
) -> Result<HashMap<String, BoardPref>, sqlx::Error> {
    if board_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, String, Option<Vec<String>>, Option<String>, String)> = sqlx::query_as(
        "select board_id::text, preset, events, scope, cadence from board_notify_prefs \
             where user_id = $1::uuid and board_id = any($2::uuid[])",
    )
    .bind(user_id)
    .bind(board_ids)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(board, preset, events, scope, cadence)| {
            BoardPref::from_parts(
                &preset,
                events.as_deref(),
                scope.as_deref(),
                Some(cadence.as_str()),
            )
            .map(|p| (board, p))
        })
        .collect())
}

/// Write the caller's own setting for one board (an upsert on their row —
/// never anyone else's).
pub async fn store(
    pg: &PgPool,
    user_id: &str,
    board_id: &str,
    p: &BoardPref,
) -> Result<(), sqlx::Error> {
    let custom = p.preset == Preset::Custom;
    let events: Option<Vec<&str>> =
        custom.then(|| p.custom_events.iter().map(|e| e.id()).collect());
    sqlx::query(
        "insert into board_notify_prefs (user_id, board_id, preset, events, scope, cadence) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6) \
         on conflict (user_id, board_id) do update set \
           preset = excluded.preset, events = excluded.events, scope = excluded.scope, \
           cadence = excluded.cadence, updated_at = now()",
    )
    .bind(user_id)
    .bind(board_id)
    .bind(p.preset.id())
    .bind(events)
    .bind(custom.then(|| p.custom_scope.id()))
    .bind(p.cadence.id())
    .execute(pg)
    .await?;
    Ok(())
}

/// Back to the default: delete the caller's own row for one board.
pub async fn clear(pg: &PgPool, user_id: &str, board_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from board_notify_prefs where user_id = $1::uuid and board_id = $2::uuid")
        .bind(user_id)
        .bind(board_id)
        .execute(pg)
        .await?;
    Ok(())
}

// ── The wire ─────────────────────────────────────────────────────────────────

/// The vocabulary the client renders — labels included, never a client-side
/// list: `{events: [{id, label, available, boardWideOnly}], presets: [{id,
/// label, events, scope}], cadences: [{id, label, available}]}`.
pub fn registry_wire() -> Value {
    json!({
        "events": BoardEvent::ALL.iter().map(|e| json!({
            "id": e.id(),
            "label": e.label(),
            "available": e.available(),
            "boardWideOnly": e.board_wide_only(),
        })).collect::<Vec<_>>(),
        "presets": Preset::ALL.iter().map(|p| {
            let (events, scope) = p.expansion().map_or((Value::Null, Value::Null), |(ev, sc)| {
                (json!(ev.iter().map(|e| e.id()).collect::<Vec<_>>()), json!(sc.id()))
            });
            json!({ "id": p.id(), "label": p.label(), "events": events, "scope": scope })
        }).collect::<Vec<_>>(),
        "cadences": CADENCES.iter().map(|c| json!({
            "id": c.id(),
            "label": c.label(),
            "available": c.available(),
        })).collect::<Vec<_>>(),
    })
}

/// Every cadence, in wire order.
const CADENCES: [Cadence; 2] = [Cadence::Instant, Cadence::Digest];

/// One effective setting as the wire serves it: `{effective, source,
/// stored}` — the expanded setting that applies, where it came from, and the
/// row it came from (null when it is not the person's own).
pub fn state_wire(effective: &BoardPref, source: Source, stored: Option<&BoardPref>) -> Value {
    json!({
        "effective": effective.to_wire(),
        "source": source.id(),
        "stored": stored.map(BoardPref::to_stored),
    })
}

/// A person's default as `GET /api/notifications` serves it (`boards`):
/// `{effective, source: "default"|"builtin", stored}`.
pub fn default_wire(blob: &Value) -> Value {
    let default = default_of(blob);
    let (p, source) = effective(None, default.as_ref());
    state_wire(&p, source, default.as_ref())
}

/// A written setting, checked: `{preset, events?, scope?, cadence?}`.
/// `events` and `scope` belong to Custom alone; Custom needs at least one
/// event; New ticket needs scope board; an event or a cadence with no
/// producer yet is refused. Unknown members are dropped. Every message a
/// write can reach is pinned in the tests below.
pub fn parse_write(v: &Value) -> Result<BoardPref, String> {
    let o = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
    let preset = Preset::parse(&enum_member(o, "preset", PRESET_IDS)?).expect("checked");
    let present = |k: &str| o.get(k).is_some_and(|v| !v.is_null());
    let cadence = match o.get("cadence").filter(|v| !v.is_null()) {
        None => Cadence::Instant,
        Some(_) => Cadence::parse(&enum_member(o, "cadence", CADENCE_IDS)?).expect("checked"),
    };
    if !cadence.available() {
        return Err("a daily digest isn't available yet".into());
    }
    if preset != Preset::Custom {
        if present("events") || present("scope") {
            return Err("events and scope are set only with the custom preset".into());
        }
        return Ok(BoardPref::preset(preset, cadence));
    }
    let events_v = o.get("events").filter(|v| !v.is_null());
    let Some(events_v) = events_v else {
        return Err("pick at least one event, or choose off".into());
    };
    let arr = events_v.as_array().ok_or_else(|| {
        format!(
            "Invalid input: expected array, received {}",
            zod_type_name(events_v)
        )
    })?;
    let ids: Vec<&str> = BoardEvent::ALL.iter().map(|e| e.id()).collect();
    let mut events = Vec::with_capacity(arr.len());
    for e in arr {
        let ev = e
            .as_str()
            .and_then(BoardEvent::parse)
            .ok_or_else(|| enum_msg(&ids))?;
        // Every event has a producer today; one listed ahead of its producer
        // is refused, so nobody can choose something that never arrives.
        if !ev.available() {
            return Err(format!("{} isn't available yet", ev.label()));
        }
        events.push(ev);
    }
    if events.is_empty() {
        return Err("pick at least one event, or choose off".into());
    }
    let scope = match o.get("scope").filter(|v| !v.is_null()) {
        None => Scope::Mine,
        Some(_) => Scope::parse(&enum_member(o, "scope", SCOPE_IDS)?).expect("checked"),
    };
    if scope == Scope::Mine && events.iter().any(|e| e.board_wide_only()) {
        return Err(
            "new tickets reach you only with scope board — nobody is in a ticket before it exists"
                .into(),
        );
    }
    Ok(BoardPref::custom(&events, scope, cadence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use BoardEvent::*;

    fn shapes() -> Vec<(&'static str, Candidate)> {
        vec![
            (
                "deliberate watch",
                Candidate {
                    deliberate_watch: true,
                    ..Candidate::involved("u")
                },
            ),
            ("editor-added watch", Candidate::involved("u")),
            ("involved", Candidate::involved("u")),
            ("addressed", Candidate::addressed("u")),
            ("bystander", Candidate::bystander("u")),
        ]
    }

    fn every_pref() -> Vec<BoardPref> {
        let mut out = Vec::new();
        for cadence in [Cadence::Instant, Cadence::Digest] {
            for p in [
                Preset::All,
                Preset::Participating,
                Preset::Mentions,
                Preset::Off,
            ] {
                out.push(BoardPref::preset(p, cadence));
            }
            out.push(BoardPref::custom(&[Comment], Scope::Mine, cadence));
            out.push(BoardPref::custom(&[Created, Status], Scope::Board, cadence));
            out.push(BoardPref::custom(&[Status, Comment], Scope::Mine, cadence));
            out.push(BoardPref::custom(&[Due], Scope::Mine, cadence));
            out.push(BoardPref::custom(&[Due, Completed], Scope::Board, cadence));
        }
        out
    }

    #[test]
    fn presets_expand_to_their_events() {
        let ids = |p: Preset| {
            BoardPref::preset(p, Cadence::Instant)
                .events()
                .iter()
                .map(|e| e.id())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(Preset::All),
            [
                "created",
                "assigned",
                "status",
                "comment",
                "mention",
                "due",
                "completed"
            ]
        );
        assert_eq!(
            ids(Preset::Participating),
            ["assigned", "status", "comment", "mention", "completed"]
        );
        assert_eq!(ids(Preset::Mentions), ["assigned", "mention"]);
        assert!(ids(Preset::Off).is_empty());
        assert_eq!(
            BoardPref::preset(Preset::All, Cadence::Instant).scope(),
            Scope::Board
        );
        for p in [Preset::Participating, Preset::Mentions, Preset::Off] {
            assert_eq!(BoardPref::preset(p, Cadence::Instant).scope(), Scope::Mine);
        }
        // Custom keeps registry order and drops repeats.
        let c = BoardPref::custom(
            &[Completed, Comment, Comment],
            Scope::Board,
            Cadence::Instant,
        );
        assert_eq!(c.events(), [Comment, Completed]);
        assert_eq!(c.scope(), Scope::Board);
        assert_eq!(
            BoardPref::builtin(),
            BoardPref::preset(Preset::Participating, Cadence::Instant)
        );
    }

    // EXISTING USERS SEE NO CHANGE: with no row and no default, every
    // producer's audience gets exactly the row it got before boards had a
    // setting — the involved and the addressed their ticket row, a bystander
    // nothing — and nothing new is ever filed: no board-activity row, no
    // digest, no due reminder, no new-ticket row.
    #[test]
    fn the_builtin_is_todays_audiences() {
        let (p, source) = effective(None, None);
        assert_eq!(source, Source::Builtin);
        // The producers before this module, event by event, with the
        // candidate shape each one fanned out to.
        for (ev, who) in [
            (Assigned, Candidate::addressed("a")),
            (Status, Candidate::involved("w")),
            (Completed, Candidate::involved("w")),
            (Comment, Candidate::involved("w")),
            (Mention, Candidate::addressed("m")),
        ] {
            assert_eq!(resolve(&p, ev, &who), Delivery::Ticket, "{ev:?}");
        }
        for ev in BoardEvent::ALL {
            assert_eq!(
                resolve(&p, ev, &Candidate::bystander("b")),
                Delivery::Skip,
                "{ev:?}"
            );
            for (_, c) in shapes() {
                let d = resolve(&p, ev, &c);
                assert!(
                    !matches!(d, Delivery::BoardActivity | Delivery::Digest),
                    "{ev:?} {c:?}"
                );
            }
            if matches!(ev, Created | Due) {
                for (_, c) in shapes().into_iter().filter(|(_, c)| !c.deliberate_watch) {
                    assert_eq!(resolve(&p, ev, &c), Delivery::Skip, "{ev:?} is new");
                }
            }
        }
    }

    // Every event × every preset × every candidate shape × both cadences,
    // against the written order — every case, not a sample.
    #[test]
    fn resolve_follows_the_written_order_in_every_case() {
        for p in every_pref() {
            for ev in BoardEvent::ALL {
                for (shape, c) in shapes() {
                    let got = resolve(&p, ev, &c);
                    let watch_events =
                        matches!(ev, Status | Comment | Completed | Mention | Assigned);
                    let want = if c.deliberate_watch && watch_events {
                        Delivery::Ticket
                    } else if p.preset == Preset::Off || !p.events().contains(&ev) {
                        Delivery::Skip
                    } else if c.addressed {
                        Delivery::Ticket
                    } else if c.involved {
                        if p.cadence == Cadence::Digest {
                            Delivery::Digest
                        } else {
                            Delivery::Ticket
                        }
                    } else if p.scope() == Scope::Board {
                        if p.cadence == Cadence::Digest {
                            Delivery::Digest
                        } else {
                            Delivery::BoardActivity
                        }
                    } else {
                        Delivery::Skip
                    };
                    assert_eq!(got, want, "{p:?} {ev:?} {shape}");
                }
            }
        }
    }

    // The precedence, spelled as the cases people will ask about.
    #[test]
    fn the_precedence_cases() {
        let off = BoardPref::preset(Preset::Off, Cadence::Instant);
        let watch = Candidate {
            deliberate_watch: true,
            ..Candidate::involved("u")
        };
        // A muted board is not re-awakened by a watch someone else added…
        assert_eq!(
            resolve(&off, Status, &Candidate::involved("u")),
            Delivery::Skip
        );
        // …but a watch they made themselves carries that ticket's news.
        for ev in [Status, Comment, Completed, Mention, Assigned] {
            assert_eq!(resolve(&off, ev, &watch), Delivery::Ticket, "{ev:?}");
        }
        // A watch turns on nothing new.
        assert_eq!(resolve(&off, Due, &watch), Delivery::Skip);
        assert_eq!(resolve(&off, Created, &watch), Delivery::Skip);
        // Off silences assignments and @mentions from the board.
        assert_eq!(
            resolve(&off, Assigned, &Candidate::addressed("u")),
            Delivery::Skip
        );
        assert_eq!(
            resolve(&off, Mention, &Candidate::addressed("u")),
            Delivery::Skip
        );
        // All: the tickets you are in keep their own rows; the rest of the
        // board is one board-activity row.
        let all = BoardPref::preset(Preset::All, Cadence::Instant);
        assert_eq!(
            resolve(&all, Status, &Candidate::involved("u")),
            Delivery::Ticket
        );
        assert_eq!(
            resolve(&all, Status, &Candidate::bystander("u")),
            Delivery::BoardActivity
        );
        assert_eq!(
            resolve(&all, Created, &Candidate::bystander("u")),
            Delivery::BoardActivity
        );
        // Mentions-only drops status news but keeps what points at you.
        let mentions = BoardPref::preset(Preset::Mentions, Cadence::Instant);
        assert_eq!(
            resolve(&mentions, Status, &Candidate::involved("u")),
            Delivery::Skip
        );
        assert_eq!(
            resolve(&mentions, Assigned, &Candidate::addressed("u")),
            Delivery::Ticket
        );
        // A digest never holds what points at you.
        let digest = BoardPref::preset(Preset::All, Cadence::Digest);
        assert_eq!(
            resolve(&digest, Mention, &Candidate::addressed("u")),
            Delivery::Ticket
        );
        assert_eq!(
            resolve(&digest, Status, &Candidate::involved("u")),
            Delivery::Digest
        );
        // Custom without Mention: the mention is dropped (so the comment
        // fan-out must keep that person — see talaria-tasks).
        let comments_only = BoardPref::custom(&[Comment], Scope::Mine, Cadence::Instant);
        assert_eq!(
            resolve(&comments_only, Mention, &Candidate::addressed("u")),
            Delivery::Skip
        );
        assert_eq!(
            resolve(&comments_only, Comment, &Candidate::involved("u")),
            Delivery::Ticket
        );
    }

    #[test]
    fn effective_is_row_then_default_then_builtin() {
        let row = BoardPref::preset(Preset::Off, Cadence::Instant);
        let def = BoardPref::preset(Preset::All, Cadence::Instant);
        assert_eq!(
            effective(Some(&row), Some(&def)),
            (row.clone(), Source::Board)
        );
        assert_eq!(effective(None, Some(&def)), (def.clone(), Source::Default));
        assert_eq!(
            effective(None, None),
            (BoardPref::builtin(), Source::Builtin)
        );
        // The default lives under the reserved key of the prefs blob.
        let blob = json!({ "boards": def.to_stored(), "mention": "both" });
        assert_eq!(default_of(&blob), Some(def));
        // A shape this binary does not know falls through, never to silence.
        assert_eq!(default_of(&json!({ "boards": { "preset": "loud" } })), None);
        assert_eq!(default_of(&json!({ "boards": "off" })), None);
        assert_eq!(default_of(&Value::Null), None);
    }

    #[test]
    fn the_stored_shape_round_trips() {
        for p in every_pref() {
            assert_eq!(
                BoardPref::from_stored(&p.to_stored()),
                Some(p.clone()),
                "{p:?}"
            );
        }
        assert_eq!(
            BoardPref::preset(Preset::Mentions, Cadence::Instant).to_stored(),
            json!({ "preset": "mentions", "cadence": "instant" })
        );
        assert_eq!(
            BoardPref::custom(&[Created, Comment], Scope::Board, Cadence::Instant).to_stored(),
            json!({ "preset": "custom", "events": ["created", "comment"], "scope": "board", "cadence": "instant" })
        );
        // The effective wire is always expanded, in this key order.
        let w = BoardPref::builtin().to_wire();
        assert_eq!(
            w,
            json!({
                "preset": "participating",
                "events": ["assigned", "status", "comment", "mention", "completed"],
                "scope": "mine",
                "cadence": "instant",
            })
        );
        assert_eq!(
            w.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["preset", "events", "scope", "cadence"]
        );
    }

    #[test]
    fn the_write_is_checked() {
        let w = |v: Value| parse_write(&v);
        assert_eq!(
            w(json!({ "preset": "off" })).unwrap(),
            BoardPref::preset(Preset::Off, Cadence::Instant)
        );
        assert_eq!(
            w(json!({ "preset": "all", "cadence": "instant", "inApp": false })).unwrap(),
            BoardPref::preset(Preset::All, Cadence::Instant)
        );
        assert_eq!(
            w(json!({ "preset": "custom", "events": ["comment", "status"] })).unwrap(),
            BoardPref::custom(&[Status, Comment], Scope::Mine, Cadence::Instant)
        );
        assert_eq!(
            w(json!({ "preset": "custom", "events": ["created"], "scope": "board" })).unwrap(),
            BoardPref::custom(&[Created], Scope::Board, Cadence::Instant)
        );
        // Null members are absent ones.
        assert!(w(json!({ "preset": "all", "events": null, "scope": null })).is_ok());
        let err = |v: Value| parse_write(&v).unwrap_err();
        assert_eq!(
            err(json!([])),
            "Invalid input: expected object, received array"
        );
        assert_eq!(
            err(json!({})),
            "Invalid option: expected one of \"all\"|\"participating\"|\"mentions\"|\"off\"|\"custom\""
        );
        assert_eq!(
            err(json!({ "preset": "loud" })),
            "Invalid option: expected one of \"all\"|\"participating\"|\"mentions\"|\"off\"|\"custom\""
        );
        assert_eq!(
            err(json!({ "preset": "all", "events": ["comment"] })),
            "events and scope are set only with the custom preset"
        );
        assert_eq!(
            err(json!({ "preset": "mentions", "scope": "board" })),
            "events and scope are set only with the custom preset"
        );
        assert_eq!(
            err(json!({ "preset": "custom" })),
            "pick at least one event, or choose off"
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": [] })),
            "pick at least one event, or choose off"
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": "comment" })),
            "Invalid input: expected array, received string"
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": ["nope"] })),
            "Invalid option: expected one of \"created\"|\"assigned\"|\"status\"|\"comment\"|\"mention\"|\"due\"|\"completed\""
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": ["comment"], "scope": "team" })),
            "Invalid option: expected one of \"mine\"|\"board\""
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": ["created"] })),
            "new tickets reach you only with scope board — nobody is in a ticket before it exists"
        );
        assert_eq!(
            err(json!({ "preset": "all", "cadence": "hourly" })),
            "Invalid option: expected one of \"instant\"|\"digest\""
        );
        // What has no producer yet cannot be chosen.
        assert_eq!(
            err(json!({ "preset": "all", "cadence": "digest" })),
            "a daily digest isn't available yet"
        );
        // Due-date reminders have one (the `task-due` job): a person opts in
        // with a Custom set, over the tickets they are in or the board.
        assert_eq!(
            w(json!({ "preset": "custom", "events": ["due"] })).unwrap(),
            BoardPref::custom(&[Due], Scope::Mine, Cadence::Instant)
        );
        assert_eq!(
            w(json!({ "preset": "custom", "events": ["status", "due"], "scope": "board" }))
                .unwrap(),
            BoardPref::custom(&[Status, Due], Scope::Board, Cadence::Instant)
        );
    }

    #[test]
    fn the_registry_names_every_event_and_preset() {
        let r = registry_wire();
        let events = r["events"].as_array().unwrap();
        assert_eq!(events.len(), 7);
        assert_eq!(
            events[0],
            json!({ "id": "created", "label": "New ticket on the board", "available": true, "boardWideOnly": true })
        );
        assert_eq!(events[5]["id"], "due");
        assert_eq!(events[5]["label"], "Due soon or overdue");
        assert!(
            events.iter().all(|e| e["available"] == json!(true)),
            "every event has a producer"
        );
        let presets = r["presets"].as_array().unwrap();
        assert_eq!(presets.len(), 5);
        assert_eq!(presets[1]["id"], "participating");
        assert_eq!(presets[1]["scope"], "mine");
        assert_eq!(
            presets[4],
            json!({ "id": "custom", "label": "Custom", "events": null, "scope": null })
        );
        assert_eq!(r["cadences"][1]["available"], json!(DIGEST_SERVED));
        // The default's wire: the built-in when nothing is stored.
        assert_eq!(
            default_wire(&Value::Null),
            json!({ "effective": BoardPref::builtin().to_wire(), "source": "builtin", "stored": null })
        );
    }

    // The probe that lets a producer skip reading a board's membership must
    // say yes for every stored setting whose scope is the board, and no for
    // every other — or a person who chose All would silently hear nothing.
    #[test]
    fn the_board_wide_probe_matches_the_scope() {
        for p in every_pref() {
            assert_eq!(
                hears_board(&p.to_stored()),
                p.scope() == Scope::Board,
                "{p:?}"
            );
        }
        for preset in Preset::ALL {
            if preset == Preset::Custom {
                continue;
            }
            let p = BoardPref::preset(preset, Cadence::Instant);
            assert_eq!(
                hears_board(&p.to_stored()),
                p.scope() == Scope::Board,
                "{p:?}"
            );
        }
    }

    // The probe that lets the due scan skip reading a ticket's audience must
    // say yes for every stored setting that hears due reminders, and no for
    // every other — or a person who opted in would silently hear nothing.
    #[test]
    fn the_due_probe_matches_wanting_due() {
        let mut prefs = every_pref();
        prefs.extend(
            Preset::ALL
                .into_iter()
                .filter(|p| *p != Preset::Custom)
                .map(|p| BoardPref::preset(p, Cadence::Instant)),
        );
        for p in prefs {
            assert_eq!(hears_due(&p.to_stored()), p.wants(Due), "{p:?}");
        }
        // Due is opt-in: of the presets, only All names it.
        for preset in [Preset::Participating, Preset::Mentions, Preset::Off] {
            assert!(!BoardPref::preset(preset, Cadence::Instant).wants(Due));
        }
        assert!(BoardPref::preset(Preset::All, Cadence::Instant).wants(Due));
    }

    // Who a due reminder reaches, spelled as the cases people will ask about.
    #[test]
    fn due_reminders_are_opt_in() {
        let (builtin, _) = effective(None, None);
        let involved = Candidate::involved("u");
        let bystander = Candidate::bystander("u");
        let watch = Candidate {
            deliberate_watch: true,
            ..Candidate::involved("u")
        };
        // Nobody hears one until they choose: the built-in names no due, and
        // a watch turns on nothing new.
        for c in [&involved, &bystander, &watch] {
            assert_eq!(resolve(&builtin, Due, c), Delivery::Skip, "{c:?}");
        }
        // A Custom set over the tickets they are in: those tickets' reminders.
        let mine = BoardPref::custom(&[Due], Scope::Mine, Cadence::Instant);
        assert_eq!(resolve(&mine, Due, &involved), Delivery::Ticket);
        assert_eq!(resolve(&mine, Due, &bystander), Delivery::Skip);
        // All: their own tickets' reminders as rows, the rest of the board
        // into its one counting board-activity row.
        let all = BoardPref::preset(Preset::All, Cadence::Instant);
        assert_eq!(resolve(&all, Due, &involved), Delivery::Ticket);
        assert_eq!(resolve(&all, Due, &bystander), Delivery::BoardActivity);
        // A daily digest holds them for the digest.
        let digest = BoardPref::preset(Preset::All, Cadence::Digest);
        assert_eq!(resolve(&digest, Due, &involved), Delivery::Digest);
    }

    // A settings read that fails: `listeners` answers the built-in, which
    // keeps a status move's audience but tells nobody of a due date, so the
    // due scan asks `try_listeners`, which hands the error back for the scan
    // to retry. No database: the pool points at a closed port.
    #[tokio::test]
    async fn a_failed_settings_read_is_the_due_scans_to_retry() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy("postgres://nobody@127.0.0.1:9/none")
            .unwrap();
        let board = "00000000-0000-0000-0000-00000000000b";
        let who = [Candidate::involved("00000000-0000-0000-0000-000000000001")];
        assert!(try_listeners(&pg, board, Due, &who).await.is_err());
        let status = listeners(&pg, board, Status, &who).await;
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].1, Delivery::Ticket, "the built-in keeps it");
        let due = listeners(&pg, board, Due, &who).await;
        assert_eq!(due[0].1, Delivery::Skip, "the built-in is silence");
        // Nobody to decide for: no read, so no error.
        assert!(
            try_listeners(&pg, board, Due, &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn candidates_fold_by_person() {
        let folded = fold(&[
            Candidate::involved("a"),
            Candidate {
                deliberate_watch: true,
                ..Candidate::involved("b")
            },
            Candidate::addressed("a"),
            Candidate::bystander("c"),
        ]);
        assert_eq!(folded.len(), 3);
        assert!(folded[0].addressed && folded[0].involved && !folded[0].deliberate_watch);
        assert!(folded[1].deliberate_watch);
        assert!(!folded[2].involved);
    }
}
