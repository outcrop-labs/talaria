// What a ClickUp task becomes on a board, as pure functions: the status table
// and its intake fallback, the guards on a sync's column moves, the clamps the
// create route would have applied, priorities, people and the comment author
// label. No database, no clock — every rule here is unit-tested against
// synthetic boards.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use talaria_clickup::{Person, Task};
use talaria_statuses::{BoardStatus, OFF_BOARD_STATUSES, StatusMeta, meta_of};

/// The create route's clamps (boards_id_tasks.rs), re-applied because the
/// sync writes through the engine, not the route.
pub const TITLE_MAX: usize = 300;
pub const DESCRIPTION_MAX: usize = 20_000;
pub const TAGS_MAX: usize = 20;
pub const TAG_LEN_MAX: usize = 40;

/// Lowercased ClickUp status name → board column key.
pub type StatusMap = BTreeMap<String, String>;

/// One current problem: per ticket (rewritten each time it syncs) or for the
/// whole run. `fix` is the sentence that tells a person what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub kind: String,
    pub detail: String,
    pub fix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clickup_url: Option<String>,
}

impl Problem {
    pub fn new(kind: &str, detail: impl Into<String>, fix: impl Into<String>) -> Problem {
        Problem {
            kind: kind.into(),
            detail: detail.into(),
            fix: fix.into(),
            ticket_id: None,
            ticket_ref: None,
            clickup_url: None,
        }
    }
}

/// The board's columns, read once per step.
#[derive(Debug, Clone)]
pub struct Columns {
    pub list: Vec<BoardStatus>,
    pub meta: StatusMeta,
}

impl Columns {
    pub fn new(list: Vec<BoardStatus>) -> Columns {
        let meta = meta_of(&list);
        Columns { list, meta }
    }

    pub fn has(&self, key: &str) -> bool {
        self.list.iter().any(|s| s.key == key)
    }

    pub fn label(&self, key: &str) -> String {
        self.list
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| key.to_string())
    }

    pub fn agent_start(&self, key: &str) -> bool {
        self.meta.agent_start_keys.iter().any(|k| k == key)
    }

    /// THE SYNC'S INTAKE: the first open column that is not terminal, not
    /// Blocked and NOT an agent pickup column. `StatusMeta::default_key` does
    /// not filter agent-start (a board whose Inbox was deleted serves the
    /// agent-start Assigned as its intake), and an unmapped ticket parked in a
    /// pickup queue would start whatever agent a person later assigns. None:
    /// the board has no such column, and the ticket is refused with a problem.
    pub fn intake(&self) -> Option<String> {
        self.list
            .iter()
            .find(|s| {
                s.category == "open"
                    && !s.agent_start
                    && !self.meta.terminal(&s.key)
                    && s.category != "blocked"
            })
            .map(|s| s.key.clone())
    }
}

/// A column a person may map a ClickUp status to, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub key: String,
    pub label: String,
    pub category: String,
    /// Entering it starts agent work for the agents assigned to a ticket. The
    /// sync never adds agents and never moves a ticket that has some into one.
    pub agent_start: bool,
    pub terminal: bool,
}

/// Every board column plus Blocked; the off-board failed and cancelled are
/// not columns and are never offered.
pub fn offered_targets(cols: &Columns) -> Vec<Target> {
    cols.list
        .iter()
        .filter(|s| !OFF_BOARD_STATUSES.contains(&s.key.as_str()))
        .map(|s| Target {
            key: s.key.clone(),
            label: s.label.clone(),
            category: s.category.clone(),
            agent_start: s.agent_start,
            terminal: cols.meta.terminal(&s.key),
        })
        .collect()
}

/// The preview's pre-fill: the board's previous connection to the same list
/// first (a reconnect keeps its table), else an EXACT, case-insensitive label
/// or key match. Nothing is guessed: everything else starts unmapped.
pub fn suggest(status: &str, cols: &Columns, previous: &StatusMap) -> Option<String> {
    let name = status.trim().to_lowercase();
    if let Some(prev) = previous.get(&name)
        && cols.has(prev)
        && !OFF_BOARD_STATUSES.contains(&prev.as_str())
    {
        return Some(prev.clone());
    }
    cols.list
        .iter()
        .filter(|s| !OFF_BOARD_STATUSES.contains(&s.key.as_str()))
        .find(|s| s.label.trim().to_lowercase() == name || s.key.to_lowercase() == name)
        .map(|s| s.key.clone())
}

/// A saved table's refusal, in the sentence the route answers with.
pub fn validate_map(map: &StatusMap, cols: &Columns) -> Result<StatusMap, String> {
    let mut out = StatusMap::new();
    for (name, key) in map {
        let name = name.trim().to_lowercase();
        if name.is_empty() {
            return Err("a status name in the mapping is empty".into());
        }
        if name.chars().count() > 200 {
            return Err("a status name in the mapping is too long".into());
        }
        if OFF_BOARD_STATUSES.contains(&key.as_str()) {
            return Err(format!(
                "\"{key}\" is not a column — ClickUp statuses map to a board column or Blocked"
            ));
        }
        if !cols.has(key) {
            return Err(format!("\"{key}\" is not a column on this board"));
        }
        out.insert(name, key.clone());
    }
    Ok(out)
}

/// Where a ClickUp status puts a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// The table maps it to a column the board still has.
    Mapped(String),
    /// No mapping (a status added in ClickUp later, or a mapping to a column
    /// deleted since): the sync's intake, with a problem.
    Unmapped(String),
    /// Unmapped, and the board has no intake to park it in.
    Nowhere,
}

impl Placement {
    pub fn column(&self) -> Option<&str> {
        match self {
            Placement::Mapped(k) | Placement::Unmapped(k) => Some(k),
            Placement::Nowhere => None,
        }
    }
}

pub fn place(status: &str, map: &StatusMap, cols: &Columns) -> Placement {
    let name = status.trim().to_lowercase();
    if let Some(key) = map.get(&name)
        && cols.has(key)
        && !OFF_BOARD_STATUSES.contains(&key.as_str())
    {
        return Placement::Mapped(key.clone());
    }
    match cols.intake() {
        Some(k) => Placement::Unmapped(k),
        None => Placement::Nowhere,
    }
}

/// What the guards say about one column move the sync wants to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveVerdict {
    Move,
    /// Already there.
    Stay,
    /// The move is a person's to make; the sentence says why.
    Hold(String),
}

/// THE SYNC'S MOVE RULES, enforced in the engine whatever the mapping says.
/// The platform actor would be allowed every one of these moves; the sync
/// declines them because each one takes a decision away from a person or an
/// agent mid-flight:
///   · out of review — leaving review resolves the reviewer's approval;
///   · out of Blocked — a person unblocks a ticket;
///   · out of a working column while an agent's work session is live on it;
///   · into a pickup (agent-start) column while it has agent assignees —
///     that would start their work, and the sync never starts an agent on
///     the ticket it moves.
/// One consequence is NOT a guard: a close mapped to a done column is a done
/// move like a person's, so a workchain step it closes hands on, and the next
/// step's agents start as if a person had closed it (docs/BOARD-SYNC.md says
/// so; the owner mapped the status and a person closed the task in ClickUp).
pub fn guard_move(
    cur: &str,
    target: &str,
    cols: &Columns,
    has_agents: bool,
    live_session: bool,
) -> MoveVerdict {
    if cur == target {
        return MoveVerdict::Stay;
    }
    let to = cols.label(target);
    if cols.meta.review_keys.iter().any(|k| k == cur) {
        return MoveVerdict::Hold(format!(
            "ClickUp moved this to \"{to}\", but it is in review here — a person moves it on"
        ));
    }
    if cur == "blocked" {
        return MoveVerdict::Hold(format!(
            "ClickUp moved this to \"{to}\", but it is Blocked here — a person unblocks it"
        ));
    }
    if live_session && cols.meta.working_keys.iter().any(|k| k == cur) {
        return MoveVerdict::Hold(format!(
            "ClickUp moved this to \"{to}\", but an agent is working on it here — it stays until the work stops"
        ));
    }
    if has_agents && cols.agent_start(target) {
        return MoveVerdict::Hold(format!(
            "ClickUp moved this to \"{to}\", which would start the agents assigned to it — the sync never starts them"
        ));
    }
    MoveVerdict::Move
}

/// ClickUp 1/2/3/4/none → urgent/high/medium/low/medium.
pub fn priority_of(p: Option<u8>) -> &'static str {
    match p {
        Some(1) => "urgent",
        Some(2) => "high",
        Some(4) => "low",
        _ => "medium",
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn clamp_title(name: &str) -> String {
    let t = name.trim();
    if t.is_empty() {
        return "(untitled)".into();
    }
    take_chars(t, TITLE_MAX)
}

/// The description, cut at the create route's 20,000 with a trailing line
/// that says where the rest is. `true` when it was cut.
pub fn clamp_description(text: Option<&str>, url: Option<&str>) -> (Option<String>, bool) {
    let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return (None, false);
    };
    if t.chars().count() <= DESCRIPTION_MAX {
        return (Some(t.to_string()), false);
    }
    let note = format!(
        "\n\n(truncated — full text in ClickUp: {})",
        url.unwrap_or("the ClickUp task")
    );
    let keep = DESCRIPTION_MAX.saturating_sub(note.chars().count());
    (Some(format!("{}{note}", take_chars(t, keep))), true)
}

/// The first 20 tags at 40 characters each, de-duplicated. `true` when any
/// were dropped or cut.
pub fn clamp_tags(tags: &[String]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    for t in tags {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        let c = take_chars(t, TAG_LEN_MAX);
        if c.chars().count() < t.chars().count() {
            cut = true;
        }
        if out.contains(&c) {
            continue;
        }
        if out.len() == TAGS_MAX {
            cut = true;
            break;
        }
        out.push(c);
    }
    (out, cut)
}

/// The ClickUp parent a sub-task hangs off in Talaria. ClickUp nests deeper
/// than Talaria's one level, so it is the TOP-LEVEL parent.
pub fn parent_of(task: &Task) -> Option<String> {
    task.top_level_parent
        .clone()
        .or_else(|| task.parent.clone())
}

/// An imported comment's author: "<ClickUp name> (ClickUp)". Never an email,
/// never resolved against users — it cannot land as a teammate.
pub fn author_label(user: Option<&Person>) -> String {
    let name = user
        .map(|u| u.display())
        .unwrap_or_else(|| "a ClickUp member".into());
    let clean: String = name.replace('@', " at ").chars().take(80).collect();
    format!("{} (ClickUp)", clean.trim())
}

/// Talaria people the sync can see, by lowercased email.
#[derive(Debug, Clone, Default)]
pub struct People {
    /// Board members (owner, editor, viewer — and team members): email → id.
    pub members: HashMap<String, String>,
    /// Every Talaria user: email → (id, name).
    pub users: HashMap<String, (String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignee {
    Member(String),
    NotOnBoard(String),
    NoAccount(String),
    NoEmail(String),
}

pub fn assignee(p: &Person, people: &People) -> Assignee {
    let Some(email) = p
        .email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
    else {
        return Assignee::NoEmail(p.display());
    };
    if let Some(id) = people.members.get(&email) {
        return Assignee::Member(id.clone());
    }
    match people.users.get(&email) {
        Some((_, name)) => Assignee::NotOnBoard(if name.is_empty() { email } else { name.clone() }),
        None => Assignee::NoAccount(email),
    }
}

/// The mapped human assignees (`user:<id>`) and one problem per person who
/// could not be assigned.
pub fn map_assignees(task: &Task, people: &People) -> (Vec<String>, Vec<Problem>) {
    let mut humans = Vec::new();
    let mut problems = Vec::new();
    for p in &task.assignees {
        match assignee(p, people) {
            Assignee::Member(id) => {
                let a = format!("user:{id}");
                if !humans.contains(&a) {
                    humans.push(a);
                }
            }
            Assignee::NotOnBoard(name) => problems.push(Problem::new(
                "assignee-not-member",
                format!("{name} is assigned in ClickUp and is on Talaria, but not on this board"),
                "Add them in Board settings → People, then sync again",
            )),
            Assignee::NoAccount(email) => problems.push(Problem::new(
                "assignee-no-account",
                format!("{email} is assigned in ClickUp and has no Talaria account"),
                "Invite them to Talaria and add them to this board, or leave it unassigned",
            )),
            Assignee::NoEmail(name) => problems.push(Problem::new(
                "assignee-no-email",
                format!("{name} is assigned in ClickUp, which does not share their email"),
                "Assign the ticket in Talaria by hand",
            )),
        }
    }
    (humans, problems)
}

/// Epoch ms as the ISO timestamp the task engine binds.
pub fn iso(ms: Option<i64>) -> Option<String> {
    ms.map(talaria_agent_auth::epoch_ms_to_iso)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn status(key: &str, category: &str, agent_start: bool) -> BoardStatus {
        BoardStatus {
            id: Some(key.into()),
            key: key.into(),
            label: key.replace('_', " "),
            color: "slate".into(),
            category: category.into(),
            agent_start,
            position: 0,
            system: None,
        }
    }

    /// The shipped defaults plus the injected Blocked.
    pub(crate) fn default_board() -> Columns {
        Columns::new(vec![
            status("inbox", "open", false),
            status("assigned", "open", true),
            status("in_progress", "active", true),
            status("blocked", "blocked", false),
            status("quality_review", "review", false),
            status("done", "done", false),
        ])
    }

    #[test]
    fn suggestions_are_exact_label_matches_or_the_last_table_only() {
        let cols = default_board();
        let none = StatusMap::new();
        assert_eq!(suggest("Done", &cols, &none).as_deref(), Some("done"));
        assert_eq!(
            suggest("in progress", &cols, &none).as_deref(),
            Some("in_progress")
        );
        assert_eq!(suggest("to do", &cols, &none), None, "never guessed");
        assert_eq!(suggest("complete", &cols, &none), None);
        let prev: StatusMap = [("complete".to_string(), "done".to_string())].into();
        assert_eq!(suggest("Complete", &cols, &prev).as_deref(), Some("done"));
    }

    #[test]
    fn an_unmapped_status_lands_in_the_sync_intake_never_an_agent_column() {
        let cols = default_board();
        let map: StatusMap = [("qa".to_string(), "deleted_col".to_string())].into();
        assert_eq!(
            place("QA", &map, &cols),
            Placement::Unmapped("inbox".into())
        );
        assert_eq!(
            place("whatever", &StatusMap::new(), &cols),
            Placement::Unmapped("inbox".into())
        );
        // Inbox deleted: default_key would be the agent-start Assigned; the
        // sync's intake refuses it.
        let no_inbox = Columns::new(vec![
            status("assigned", "open", true),
            status("in_progress", "active", true),
            status("done", "done", false),
        ]);
        assert_eq!(no_inbox.meta.default_key.as_deref(), Some("assigned"));
        assert_eq!(no_inbox.intake(), None);
        assert_eq!(place("x", &StatusMap::new(), &no_inbox), Placement::Nowhere);
        let mapped: StatusMap = [("doing".to_string(), "in_progress".to_string())].into();
        assert_eq!(
            place("Doing", &mapped, &cols),
            Placement::Mapped("in_progress".into())
        );
    }

    #[test]
    fn the_table_refuses_off_board_and_unknown_targets_but_offers_agent_columns() {
        let cols = default_board();
        let bad: StatusMap = [("x".to_string(), "failed".to_string())].into();
        assert!(validate_map(&bad, &cols).is_err());
        let unknown: StatusMap = [("x".to_string(), "nope".to_string())].into();
        assert!(validate_map(&unknown, &cols).is_err());
        let ok: StatusMap = [
            ("In Progress".to_string(), "in_progress".to_string()),
            ("complete".to_string(), "done".to_string()),
            ("stuck".to_string(), "blocked".to_string()),
        ]
        .into();
        let v = validate_map(&ok, &cols).unwrap();
        assert_eq!(
            v.get("in progress").map(String::as_str),
            Some("in_progress")
        );
        let offered = offered_targets(&cols);
        let ip = offered.iter().find(|t| t.key == "in_progress").unwrap();
        assert!(
            ip.agent_start,
            "offered, and the preview says it starts agent work"
        );
        assert!(offered.iter().find(|t| t.key == "done").unwrap().terminal);
        assert!(
            !offered
                .iter()
                .any(|t| t.key == "failed" || t.key == "cancelled")
        );
    }

    #[test]
    fn the_guards_hold_review_blocked_live_work_and_agent_pickup() {
        let cols = default_board();
        assert_eq!(
            guard_move("inbox", "inbox", &cols, false, false),
            MoveVerdict::Stay
        );
        assert_eq!(
            guard_move("inbox", "done", &cols, false, false),
            MoveVerdict::Move
        );
        assert!(matches!(
            guard_move("quality_review", "done", &cols, false, false),
            MoveVerdict::Hold(_)
        ));
        assert!(matches!(
            guard_move("blocked", "inbox", &cols, false, false),
            MoveVerdict::Hold(_)
        ));
        assert!(matches!(
            guard_move("in_progress", "done", &cols, true, true),
            MoveVerdict::Hold(_)
        ));
        assert_eq!(
            guard_move("in_progress", "done", &cols, true, false),
            MoveVerdict::Move
        );
        assert!(matches!(
            guard_move("inbox", "assigned", &cols, true, false),
            MoveVerdict::Hold(_)
        ));
        assert_eq!(
            guard_move("inbox", "assigned", &cols, false, false),
            MoveVerdict::Move,
            "no agents assigned: entering a pickup column starts nothing"
        );
    }

    #[test]
    fn priorities_map_and_none_is_medium() {
        assert_eq!(priority_of(Some(1)), "urgent");
        assert_eq!(priority_of(Some(2)), "high");
        assert_eq!(priority_of(Some(3)), "medium");
        assert_eq!(priority_of(Some(4)), "low");
        assert_eq!(priority_of(None), "medium");
    }

    #[test]
    fn the_route_clamps_are_reapplied() {
        let long = "x".repeat(400);
        assert_eq!(clamp_title(&long).chars().count(), TITLE_MAX);
        assert_eq!(clamp_title("  "), "(untitled)");
        let body = "y".repeat(25_000);
        let (d, cut) = clamp_description(Some(&body), Some("https://app.clickup.com/t/abc"));
        let d = d.unwrap();
        assert!(cut);
        assert_eq!(d.chars().count(), DESCRIPTION_MAX);
        assert!(d.ends_with("(truncated — full text in ClickUp: https://app.clickup.com/t/abc)"));
        assert_eq!(clamp_description(Some("  "), None), (None, false));
        let tags: Vec<String> = (0..25).map(|i| format!("t{i}")).collect();
        let (t, cut) = clamp_tags(&tags);
        assert_eq!(t.len(), TAGS_MAX);
        assert!(cut);
        let (t, cut) = clamp_tags(&["a".repeat(50), "b".into(), "b".into()]);
        assert_eq!(t, vec!["a".repeat(40), "b".to_string()]);
        assert!(cut);
    }

    #[test]
    fn a_sub_sub_task_hangs_off_its_top_level_parent() {
        let mut t = Task::parse(
            &serde_json::json!({"id": "c", "name": "c", "parent": "b", "top_level_parent": "a"}),
        )
        .unwrap();
        assert_eq!(parent_of(&t).as_deref(), Some("a"));
        t.top_level_parent = None;
        assert_eq!(parent_of(&t).as_deref(), Some("b"));
        t.parent = None;
        assert_eq!(parent_of(&t), None);
    }

    #[test]
    fn assignee_outcomes_cover_member_stranger_unknown_and_withheld() {
        let mut people = People::default();
        people.members.insert("m@x.io".into(), "u1".into());
        people
            .users
            .insert("m@x.io".into(), ("u1".into(), "Mia".into()));
        people
            .users
            .insert("s@x.io".into(), ("u2".into(), "Sam".into()));
        let p = |email: Option<&str>| Person {
            id: None,
            username: Some("Name".into()),
            email: email.map(String::from),
        };
        assert_eq!(
            assignee(&p(Some("M@x.io")), &people),
            Assignee::Member("u1".into())
        );
        assert_eq!(
            assignee(&p(Some("s@x.io")), &people),
            Assignee::NotOnBoard("Sam".into())
        );
        assert_eq!(
            assignee(&p(Some("z@x.io")), &people),
            Assignee::NoAccount("z@x.io".into())
        );
        assert_eq!(
            assignee(&p(None), &people),
            Assignee::NoEmail("Name".into())
        );
        let task = Task::parse(&serde_json::json!({
            "id": "t", "name": "t",
            "assignees": [{"id": 1, "username": "Mia", "email": "m@x.io"}, {"id": 2, "username": "Zed", "email": "z@x.io"}]
        }))
        .unwrap();
        let (humans, problems) = map_assignees(&task, &people);
        assert_eq!(humans, vec!["user:u1".to_string()]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, "assignee-no-account");
    }

    #[test]
    fn the_author_label_never_carries_an_email_and_always_says_clickup() {
        let p = Person {
            id: None,
            username: None,
            email: Some("dana@example.com".into()),
        };
        let l = author_label(Some(&p));
        assert!(!l.contains('@'));
        assert!(l.ends_with(" (ClickUp)"));
        let odd = Person {
            id: None,
            username: Some("me@home".into()),
            email: None,
        };
        assert!(!author_label(Some(&odd)).contains('@'));
        assert_eq!(author_label(None), "a ClickUp member (ClickUp)");
    }
}
