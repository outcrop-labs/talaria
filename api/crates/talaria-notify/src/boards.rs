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
// send time re-reads it. The row, once filed, is the record.
//
// A BOARD'S OWN CHANNELS. A setting may also say how this board's rows reach
// the person — Alerts (the banner and the push, together) and Email, each on,
// off, or (None) as the class matrix in Settings → Notifications says. That
// too is decided at fan-out: the producer files the row with the override
// (`NotifyMeta::channels`), the writer stores it on the row, and the deferred
// push and the mail batcher re-check the matrix through the stored override
// (`prefs::channels_with`) — never by reading this module. In-app is not a
// choice: every row lands in the inbox.
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
use talaria_body::{
    boolean_msg, enum_member, enum_msg, nullable_boolean_member, object_msg, zod_type_name,
};

pub use crate::meta::ChannelOverride;

/// The reserved key of `users.notify_prefs` that holds a person's default
/// for every board (`prefs::merge_prefs` writes it, `default_of` reads it).
pub const BOARDS_KEY: &str = "boards";

/// The features this module serves in `GET /api/notifications` (I-23):
/// the setting (`board-prefs`) and a board's own channels
/// (`board-channels`). `task-due` and `board-digest` join when their
/// producers ship.
pub const FEATURES: &[&str] = &["board-prefs", "board-channels"];

/// Due-date reminders have a producer (`task-due`). Until then the event is
/// listed but refused, so nobody can choose something that never arrives.
const DUE_SERVED: bool = false;
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
    /// A ticket is due soon or overdue (no producer yet: `DUE_SERVED`).
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

    /// Whether a person may choose it yet (its producer exists).
    pub fn available(self) -> bool {
        self != BoardEvent::Due || DUE_SERVED
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
    /// How this board's rows reach them, where it is not the class matrix's
    /// answer (None everywhere: the matrix).
    pub channels: BoardChannels,
}

/// How one board's rows reach a person, where it is not the class matrix's
/// answer — two choices, each `Some(on)` or `None` (the matrix decides):
/// - `alerts`: the banner and the push, together. On pops the row up and
///   pushes it; off keeps it in the inbox alone. The inbox keeps every row
///   either way, so in-app is not a choice (I-13).
/// - `email`: the missed-activity mail.
///
/// Push is deliberately not a board choice of its own (mobile push
/// configuration is out of scope); the matrix keeps its per-class Push.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardChannels {
    pub alerts: Option<bool>,
    pub email: Option<bool>,
}

impl BoardChannels {
    /// Nothing chosen: the matrix decides both.
    pub fn is_none(&self) -> bool {
        self.alerts.is_none() && self.email.is_none()
    }

    /// The override this board's rows are filed with
    /// (`NotifyMeta::channels`): Alerts sets Desktop and Push together, so a
    /// row the board turns quiet neither pops up nor pushes, and one it turns
    /// loud does both.
    pub fn overrides(&self) -> ChannelOverride {
        ChannelOverride {
            desktop: self.alerts,
            push: self.alerts,
            email: self.email,
        }
    }
}

/// The channels a person may choose for a board, in wire order.
const CHANNEL_KEYS: [&str; 2] = ["alerts", "email"];

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
            channels: BoardChannels::default(),
        }
    }

    /// The same setting, reaching the person on these channels.
    pub fn with_channels(self, channels: BoardChannels) -> Self {
        Self { channels, ..self }
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
            channels: BoardChannels::default(),
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
    /// fields): the preset, the cadence, — Custom only — the events and the
    /// scope, and — only when one is chosen — the channels, each chosen one
    /// by name (`{"alerts": false}`).
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
        if !self.channels.is_none() {
            let mut c = Map::new();
            for (k, v) in channel_values(&self.channels) {
                if let Some(on) = v {
                    c.insert(k.into(), json!(on));
                }
            }
            m.insert("channels".into(), Value::Object(c));
        }
        Value::Object(m)
    }

    /// The effective shape the wire serves: always expanded, and every
    /// channel named — `null` where the class matrix decides.
    pub fn to_wire(&self) -> Value {
        let mut channels = Map::new();
        for (k, v) in channel_values(&self.channels) {
            channels.insert(k.into(), json!(v));
        }
        json!({
            "preset": self.preset.id(),
            "events": self.events().iter().map(|e| e.id()).collect::<Vec<_>>(),
            "scope": self.scope().id(),
            "cadence": self.cadence.id(),
            "channels": channels,
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
        channels: BoardChannels,
    ) -> Option<Self> {
        let preset = Preset::parse(preset)?;
        let cadence = match cadence {
            None => Cadence::Instant,
            Some(c) => Cadence::parse(c)?,
        };
        if preset != Preset::Custom {
            return Some(Self::preset(preset, cadence).with_channels(channels));
        }
        let mut chosen = Vec::new();
        for e in events? {
            chosen.push(BoardEvent::parse(e)?);
        }
        let scope = Scope::parse(scope.unwrap_or("mine"))?;
        Some(Self::custom(&chosen, scope, cadence).with_channels(channels))
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
        // A channel that is not a bool (a hand edit) follows the matrix:
        // never silence, and never louder than the person chose.
        let c = o.get("channels").and_then(Value::as_object);
        let pick = |k: &str| c.and_then(|c| c.get(k)).and_then(Value::as_bool);
        Self::from_parts(
            o.get("preset")?.as_str()?,
            events.as_deref(),
            o.get("scope").and_then(Value::as_str),
            o.get("cadence").and_then(Value::as_str),
            BoardChannels {
                alerts: pick(CHANNEL_KEYS[0]),
                email: pick(CHANNEL_KEYS[1]),
            },
        )
    }
}

/// The channels by wire name, in wire order.
fn channel_values(c: &BoardChannels) -> [(&'static str, Option<bool>); 2] {
    [(CHANNEL_KEYS[0], c.alerts), (CHANNEL_KEYS[1], c.email)]
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

/// Each candidate's delivery for one event on one board, and the channels
/// their setting gives this board's rows (file the row with them as
/// `NotifyMeta::channels`), from ONE read of their settings for it and their
/// defaults. A read that fails is logged and answers the built-in for
/// everyone — the audiences the producers had before boards had a setting,
/// on the matrix's channels — never silence.
pub async fn listeners(
    pg: &PgPool,
    board_id: &str,
    ev: BoardEvent,
    candidates: &[Candidate],
) -> Vec<(Candidate, Delivery, ChannelOverride)> {
    let folded = fold(candidates);
    if folded.is_empty() {
        return Vec::new();
    }
    let ids: Vec<String> = folded.iter().map(|c| c.user_id.clone()).collect();
    let prefs = match read_prefs(pg, board_id, &ids).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[notifications] board settings read failed: {e}");
            HashMap::new()
        }
    };
    folded
        .into_iter()
        .map(|c| {
            let (row, default) = prefs.get(&c.user_id).cloned().unwrap_or_default();
            let (p, _) = effective(row.as_ref(), default.as_ref());
            let d = resolve(&p, ev, &c);
            (c, d, p.channels.overrides())
        })
        .collect()
}

type PrefRow = (
    String,
    Option<String>,
    Option<Vec<String>>,
    Option<String>,
    Option<String>,
    Option<bool>,
    Option<bool>,
    Option<Value>,
);

/// user id → (their row for this board, their default), for these people.
async fn read_prefs(
    pg: &PgPool,
    board_id: &str,
    user_ids: &[String],
) -> Result<HashMap<String, (Option<BoardPref>, Option<BoardPref>)>, sqlx::Error> {
    let rows: Vec<PrefRow> = sqlx::query_as(
        "select u.id::text, p.preset, p.events, p.scope, p.cadence, p.alerts, p.email, \
                u.notify_prefs -> 'boards' \
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
        .map(
            |(id, preset, events, scope, cadence, alerts, email, default)| {
                let row = preset.and_then(|p| {
                    BoardPref::from_parts(
                        &p,
                        events.as_deref(),
                        scope.as_deref(),
                        cadence.as_deref(),
                        BoardChannels { alerts, email },
                    )
                });
                let default = default.as_ref().and_then(BoardPref::from_stored);
                (id, (row, default))
            },
        )
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

/// Who among `user_ids` — the people a ticket's text @mentions — gets their
/// mention row, by their setting for the ticket's board, and the channels it
/// reaches them on (file it with them as `NotifyMeta::channels`). The
/// deliberate-watch flag is read for exactly these people: a watch can only
/// keep a mention a board would drop, never add a recipient the caller did
/// not already name. A read that fails keeps everyone (the built-in files
/// every mention, on the matrix's channels).
pub async fn mention_listeners(
    pg: &PgPool,
    board_id: &str,
    task_id: &str,
    user_ids: &[String],
) -> HashMap<String, ChannelOverride> {
    if user_ids.is_empty() {
        return HashMap::new();
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
        .filter(|(_, d, _)| *d == Delivery::Ticket)
        .map(|(c, _, channels)| (c.user_id, channels))
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
    type StoredRow = (
        String,
        String,
        Option<Vec<String>>,
        Option<String>,
        String,
        Option<bool>,
        Option<bool>,
    );
    let rows: Vec<StoredRow> = sqlx::query_as(
        "select board_id::text, preset, events, scope, cadence, alerts, email \
             from board_notify_prefs \
             where user_id = $1::uuid and board_id = any($2::uuid[])",
    )
    .bind(user_id)
    .bind(board_ids)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(board, preset, events, scope, cadence, alerts, email)| {
            BoardPref::from_parts(
                &preset,
                events.as_deref(),
                scope.as_deref(),
                Some(cadence.as_str()),
                BoardChannels { alerts, email },
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
        "insert into board_notify_prefs \
           (user_id, board_id, preset, events, scope, cadence, alerts, email) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6, $7, $8) \
         on conflict (user_id, board_id) do update set \
           preset = excluded.preset, events = excluded.events, scope = excluded.scope, \
           cadence = excluded.cadence, alerts = excluded.alerts, email = excluded.email, \
           updated_at = now()",
    )
    .bind(user_id)
    .bind(board_id)
    .bind(p.preset.id())
    .bind(events)
    .bind(custom.then(|| p.custom_scope.id()))
    .bind(p.cadence.id())
    .bind(p.channels.alerts)
    .bind(p.channels.email)
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

/// A written setting, checked: `{preset, events?, scope?, cadence?,
/// channels?}`. `events` and `scope` belong to Custom alone; Custom needs at
/// least one event; New ticket needs scope board; an event or a cadence with
/// no producer yet is refused. `channels` is `{alerts?, email?}`, each a
/// bool or null (null or absent: the class matrix decides); in-app is always
/// on, so `inApp: false` is refused as it is in the matrix. Unknown members
/// are dropped. A write is the whole setting: what it leaves out goes back to
/// its default. Every message is pinned in the tests below.
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
    let channels = parse_channels(o.get("channels"))?;
    if preset != Preset::Custom {
        if present("events") || present("scope") {
            return Err("events and scope are set only with the custom preset".into());
        }
        return Ok(BoardPref::preset(preset, cadence).with_channels(channels));
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
        if !ev.available() {
            return Err("due-date reminders aren't available yet".into());
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
    Ok(BoardPref::custom(&events, scope, cadence).with_channels(channels))
}

/// The `channels` member of a written setting: absent or null is the
/// matrix for both; an object names `alerts` (the banner and the push,
/// together) and `email`, each a bool or null. In-app is always on — the row
/// is the record — so `inApp: false` is refused and `inApp: true` passes, as
/// in the matrix's own `PATCH /api/notifications {channels}`.
fn parse_channels(v: Option<&Value>) -> Result<BoardChannels, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(BoardChannels::default());
    };
    let c = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
    match c.get("inApp") {
        None | Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => return Err("In-app is always on".into()),
        Some(other) => return Err(boolean_msg(zod_type_name(other))),
    }
    Ok(BoardChannels {
        alerts: nullable_boolean_member(c, CHANNEL_KEYS[0])?,
        email: nullable_boolean_member(c, CHANNEL_KEYS[1])?,
    })
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
            // A board's own channels change no delivery decision, only how
            // the rows reach the person.
            out.push(
                BoardPref::preset(Preset::Participating, cadence).with_channels(BoardChannels {
                    email: Some(true),
                    ..Default::default()
                }),
            );
            out.push(
                BoardPref::custom(&[Created, Comment], Scope::Board, cadence).with_channels(
                    BoardChannels {
                        alerts: Some(false),
                        email: Some(false),
                    },
                ),
            );
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
        // The effective wire is always expanded, in this key order, and
        // names every channel — null where the matrix decides.
        let w = BoardPref::builtin().to_wire();
        assert_eq!(
            w,
            json!({
                "preset": "participating",
                "events": ["assigned", "status", "comment", "mention", "completed"],
                "scope": "mine",
                "cadence": "instant",
                "channels": { "alerts": null, "email": null },
            })
        );
        assert_eq!(
            w.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["preset", "events", "scope", "cadence", "channels"]
        );
        assert_eq!(
            w["channels"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["alerts", "email"]
        );
    }

    // A board's own channels: stored by name only when chosen, served with
    // both named, and a stored value this binary cannot read follows the
    // matrix — never silence, never louder than chosen.
    #[test]
    fn a_boards_own_channels_round_trip() {
        let email_only =
            BoardPref::preset(Preset::Mentions, Cadence::Instant).with_channels(BoardChannels {
                alerts: Some(false),
                email: Some(true),
            });
        assert_eq!(
            email_only.to_stored(),
            json!({ "preset": "mentions", "cadence": "instant",
                    "channels": { "alerts": false, "email": true } })
        );
        assert_eq!(
            email_only.to_wire()["channels"],
            json!({ "alerts": false, "email": true })
        );
        let quiet = BoardPref::builtin().with_channels(BoardChannels {
            alerts: Some(false),
            ..Default::default()
        });
        assert_eq!(
            quiet.to_stored(),
            json!({ "preset": "participating", "cadence": "instant",
                    "channels": { "alerts": false } })
        );
        assert_eq!(
            quiet.to_wire()["channels"],
            json!({ "alerts": false, "email": null })
        );
        for p in [&email_only, &quiet] {
            assert_eq!(BoardPref::from_stored(&p.to_stored()).as_ref(), Some(p));
        }
        // What a row is filed with: Alerts is the banner and the push
        // together, in both directions; Email is the mail; nothing chosen is
        // the matrix.
        assert_eq!(
            email_only.channels.overrides(),
            ChannelOverride {
                desktop: Some(false),
                push: Some(false),
                email: Some(true),
            }
        );
        assert_eq!(
            BoardChannels {
                alerts: Some(true),
                email: None,
            }
            .overrides(),
            ChannelOverride {
                desktop: Some(true),
                push: Some(true),
                email: None,
            }
        );
        assert!(BoardChannels::default().overrides().is_none());
        // Nothing chosen: no channels member at all.
        assert!(BoardPref::builtin().to_stored().get("channels").is_none());
        // Hand edits: a channel that is not a bool follows the matrix, a
        // member that is not a board channel is ignored, and a channels
        // member that is not an object is no choice at all.
        let odd = json!({ "preset": "all", "cadence": "instant",
                          "channels": { "email": "yes", "alerts": false, "push": true,
                                        "inApp": false } });
        assert_eq!(
            BoardPref::from_stored(&odd),
            Some(
                BoardPref::preset(Preset::All, Cadence::Instant).with_channels(BoardChannels {
                    alerts: Some(false),
                    ..Default::default()
                })
            )
        );
        let junk = json!({ "preset": "all", "channels": "loud" });
        assert_eq!(
            BoardPref::from_stored(&junk),
            Some(BoardPref::preset(Preset::All, Cadence::Instant))
        );
        // The default for every board carries them the same way.
        let blob = json!({ "boards": email_only.to_stored() });
        assert_eq!(default_of(&blob), Some(email_only.clone()));
        // The resolver never reads them: who gets a row is the same.
        for ev in BoardEvent::ALL {
            for (_, c) in shapes() {
                assert_eq!(
                    resolve(&email_only, ev, &c),
                    resolve(
                        &BoardPref::preset(Preset::Mentions, Cadence::Instant),
                        ev,
                        &c
                    ),
                    "{ev:?} {c:?}"
                );
            }
        }
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
        // A board's own channels: each a bool or null, the rest dropped —
        // Desktop and Push are not board choices of their own.
        assert_eq!(
            w(json!({ "preset": "all",
                      "channels": { "email": true, "alerts": null, "push": false, "sms": true } }))
            .unwrap(),
            BoardPref::preset(Preset::All, Cadence::Instant).with_channels(BoardChannels {
                email: Some(true),
                ..Default::default()
            })
        );
        assert_eq!(
            w(json!({ "preset": "custom", "events": ["comment"],
                      "channels": { "alerts": false, "email": true } }))
            .unwrap(),
            BoardPref::custom(&[Comment], Scope::Mine, Cadence::Instant).with_channels(
                BoardChannels {
                    alerts: Some(false),
                    email: Some(true),
                }
            )
        );
        // Off keeps its channels: they shape the rows a self-made watch
        // still carries.
        assert_eq!(
            w(json!({ "preset": "off", "channels": { "alerts": false } })).unwrap(),
            BoardPref::preset(Preset::Off, Cadence::Instant).with_channels(BoardChannels {
                alerts: Some(false),
                ..Default::default()
            })
        );
        // Null, an empty object and inApp: true are no choice.
        for none in [
            json!({ "preset": "all", "channels": null }),
            json!({ "preset": "all", "channels": {} }),
            json!({ "preset": "all", "channels": { "inApp": true } }),
        ] {
            assert_eq!(
                w(none.clone()).unwrap(),
                BoardPref::preset(Preset::All, Cadence::Instant),
                "{none}"
            );
        }
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
        assert_eq!(
            err(json!({ "preset": "all", "channels": [] })),
            "Invalid input: expected object, received array"
        );
        assert_eq!(
            err(json!({ "preset": "all", "channels": { "email": "yes" } })),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            err(json!({ "preset": "all", "channels": { "alerts": "loud" } })),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            err(json!({ "preset": "all", "channels": { "inApp": false } })),
            "In-app is always on"
        );
        assert_eq!(
            err(json!({ "preset": "all", "channels": { "inApp": "no" } })),
            "Invalid input: expected boolean, received string"
        );
        // What has no producer yet cannot be chosen.
        assert_eq!(
            err(json!({ "preset": "all", "cadence": "digest" })),
            "a daily digest isn't available yet"
        );
        assert_eq!(
            err(json!({ "preset": "custom", "events": ["due"] })),
            "due-date reminders aren't available yet"
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
        assert_eq!(events[5]["available"], json!(DUE_SERVED));
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
