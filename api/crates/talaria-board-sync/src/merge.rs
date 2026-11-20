// THE CONFLICT RULE, as one pure function: a field-level three-way merge.
//
// For every mapped field, u is what ClickUp says now and a is `applied`, the
// last upstream value the sync wrote. u == a: ClickUp did not change the
// field, so whatever Talaria holds stands (a person's edit survives). u != a:
// ClickUp changed it, so u is written and becomes the new a. Tags and human
// assignees merge as SETS — (local − a) ∪ u — so a label or a person added in
// Talaria stays while ClickUp's own changes land. Agent assignees are never
// touched. "Re-sync from ClickUp" forces every field to upstream.
//
// ORDER OF WRITES IS THE CALLER'S, and it matters: `applied` must only ever
// describe writes that landed. So this function returns the new `applied` for
// the plain fields (written together in one update), and leaves the status —
// which goes through the column guards and may be held — to the caller, which
// records `applied.status` only after its move succeeds.

use serde::{Deserialize, Serialize};
use talaria_tasks_types::is_human_assignee;

/// The last upstream value written per field. `seeded` false means "never
/// applied" — an item recovered after a crash between the ticket's insert and
/// its item row — and forces a full write, exactly like a first sync.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Applied {
    pub seeded: bool,
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub tags: Vec<String>,
    /// The mapped human assignees (`user:<id>`).
    pub humans: Vec<String>,
    /// The lowercased ClickUp status name last applied, and the column the
    /// sync wrote for it. None until a move (or a placement) has landed.
    pub status: Option<String>,
    pub column: Option<String>,
    /// The ClickUp parent the ticket hangs off (top-level parent id), once
    /// written; `parent_pending` while that parent has no ticket yet.
    pub parent: Option<String>,
    pub parent_pending: Option<String>,
    pub creator: Option<String>,
    pub created_ms: Option<i64>,
}

/// ClickUp's values, already mapped and clamped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Upstream {
    pub title: String,
    pub description: Option<String>,
    /// Lowercased ClickUp status name.
    pub status: String,
    pub priority: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub tags: Vec<String>,
    pub humans: Vec<String>,
}

/// The ticket as Talaria holds it now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Local {
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub tags: Vec<String>,
    pub assignees: Vec<String>,
}

/// What to write. `None` leaves a field alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<String>,
    pub due: Option<Option<String>>,
    pub start: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    /// ClickUp's status changed (or a force): the caller places and guards it.
    pub status: Option<String>,
    /// `applied` after the plain fields land (status untouched).
    pub applied: Applied,
}

impl Plan {
    /// Anything for the update to write besides the status?
    pub fn writes(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.priority.is_some()
            || self.due.is_some()
            || self.start.is_some()
            || self.tags.is_some()
            || self.assignees.is_some()
    }
}

/// (local − base) ∪ up, keeping local's order and appending what is new.
fn set_merge(local: &[String], base: &[String], up: &[String]) -> Vec<String> {
    let mut out: Vec<String> = local
        .iter()
        .filter(|v| !base.contains(v) || up.contains(v))
        .cloned()
        .collect();
    for v in up {
        if !out.contains(v) {
            out.push(v.clone());
        }
    }
    out
}

fn scalar<T: Clone + PartialEq>(force: bool, up: &T, base: &T, local: &T) -> Option<T> {
    if !force && up == base {
        return None; // untouched upstream: Talaria's value stands
    }
    if up == local {
        return None; // already what ClickUp says — nothing to write
    }
    Some(up.clone())
}

pub fn merge(up: &Upstream, local: &Local, base: &Applied, force: bool) -> Plan {
    let force = force || !base.seeded;
    let mut applied = base.clone();
    applied.seeded = true;

    let title = scalar(force, &up.title, &base.title, &local.title);
    let description = scalar(
        force,
        &up.description,
        &base.description,
        &local.description,
    );
    let priority = scalar(force, &up.priority, &base.priority, &local.priority);
    let due = scalar(force, &up.due, &base.due, &local.due);
    let start = scalar(force, &up.start, &base.start, &local.start);
    applied.title = up.title.clone();
    applied.description = up.description.clone();
    applied.priority = up.priority.clone();
    applied.due = up.due.clone();
    applied.start = up.start.clone();

    // Tags: a set. A force replaces them with ClickUp's.
    let tags = if force {
        (local.tags != up.tags).then(|| up.tags.clone())
    } else if up.tags != base.tags {
        let next = set_merge(&local.tags, &base.tags, &up.tags);
        (next != local.tags).then_some(next)
    } else {
        None
    };
    applied.tags = up.tags.clone();

    // Assignees: only the humans merge; agents are never added or removed.
    let local_humans: Vec<String> = local
        .assignees
        .iter()
        .filter(|a| is_human_assignee(a))
        .cloned()
        .collect();
    let next_humans = if force {
        Some(up.humans.clone())
    } else if up.humans != base.humans {
        Some(set_merge(&local_humans, &base.humans, &up.humans))
    } else {
        None
    };
    let assignees = next_humans.and_then(|humans| {
        let mut next: Vec<String> = local
            .assignees
            .iter()
            .filter(|a| !is_human_assignee(a) || humans.contains(a))
            .cloned()
            .collect();
        for h in &humans {
            if !next.contains(h) {
                next.push(h.clone());
            }
        }
        (next != local.assignees).then_some(next)
    });
    applied.humans = up.humans.clone();

    let status =
        (force || base.status.as_deref() != Some(up.status.as_str())).then(|| up.status.clone());

    Plan {
        title,
        description,
        priority,
        due,
        start,
        tags,
        assignees,
        status,
        applied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up() -> Upstream {
        Upstream {
            title: "Ship it".into(),
            description: Some("plan".into()),
            status: "in progress".into(),
            priority: "high".into(),
            due: None,
            start: None,
            tags: vec!["backend".into()],
            humans: vec!["user:dana".into()],
        }
    }

    fn base_of(u: &Upstream) -> Applied {
        Applied {
            seeded: true,
            title: u.title.clone(),
            description: u.description.clone(),
            priority: u.priority.clone(),
            due: u.due.clone(),
            start: u.start.clone(),
            tags: u.tags.clone(),
            humans: u.humans.clone(),
            status: Some(u.status.clone()),
            column: Some("in_progress".into()),
            ..Default::default()
        }
    }

    fn local_of(u: &Upstream) -> Local {
        Local {
            title: u.title.clone(),
            description: u.description.clone(),
            priority: u.priority.clone(),
            due: u.due.clone(),
            start: u.start.clone(),
            tags: u.tags.clone(),
            assignees: u.humans.clone(),
        }
    }

    #[test]
    fn an_unchanged_upstream_keeps_the_local_edit() {
        let u = up();
        let base = base_of(&u);
        let mut local = local_of(&u);
        local.title = "Ship it — edited here".into();
        let p = merge(&u, &local, &base, false);
        assert_eq!(
            p.title, None,
            "ClickUp did not change it: Talaria's title stands"
        );
        assert!(!p.writes());
        assert_eq!(p.status, None);
    }

    #[test]
    fn a_changed_upstream_overwrites_and_other_local_edits_survive() {
        let u0 = up();
        let base = base_of(&u0);
        let mut local = local_of(&u0);
        local.title = "edited here".into();
        let mut u = u0.clone();
        u.priority = "urgent".into();
        let p = merge(&u, &local, &base, false);
        assert_eq!(p.priority.as_deref(), Some("urgent"));
        assert_eq!(p.title, None, "the title was not changed upstream");
        assert_eq!(p.applied.priority, "urgent");
        // The same field changed on both sides: ClickUp wins.
        let mut u2 = u0.clone();
        u2.title = "renamed in ClickUp".into();
        let p2 = merge(&u2, &local, &base, false);
        assert_eq!(p2.title.as_deref(), Some("renamed in ClickUp"));
    }

    #[test]
    fn the_first_sync_writes_everything() {
        let u = up();
        let p = merge(&u, &Local::default(), &Applied::default(), false);
        assert_eq!(p.title.as_deref(), Some("Ship it"));
        assert_eq!(p.description, Some(Some("plan".into())));
        assert_eq!(p.status.as_deref(), Some("in progress"));
        assert!(p.applied.seeded);
        assert_eq!(
            p.applied.status, None,
            "status is the caller's to record after the move"
        );
    }

    #[test]
    fn tags_and_humans_merge_as_sets_and_keep_local_additions() {
        let u0 = up();
        let base = base_of(&u0);
        let mut local = local_of(&u0);
        local.tags.push("talaria-only".into());
        local.assignees.push("user:mia".into());
        local.assignees.push("hermes".into()); // an agent
        let mut u = u0.clone();
        u.tags = vec!["frontend".into()]; // backend removed upstream, frontend added
        u.humans = vec!["user:dana".into(), "user:lee".into()];
        let p = merge(&u, &local, &base, false);
        assert_eq!(
            p.tags.unwrap(),
            vec!["talaria-only".to_string(), "frontend".to_string()]
        );
        assert_eq!(
            p.assignees.unwrap(),
            vec![
                "user:dana".to_string(),
                "user:mia".to_string(),
                "hermes".to_string(),
                "user:lee".to_string()
            ]
        );
    }

    #[test]
    fn agent_assignees_are_never_touched() {
        let u0 = up();
        let base = base_of(&u0);
        let mut local = local_of(&u0);
        local.assignees = vec!["hermes".into(), "user:dana".into()];
        let mut u = u0.clone();
        u.humans = vec![];
        let p = merge(&u, &local, &base, false);
        assert_eq!(p.assignees.unwrap(), vec!["hermes".to_string()]);
        let forced = merge(&u, &local, &base, true);
        assert_eq!(forced.assignees.unwrap(), vec!["hermes".to_string()]);
    }

    #[test]
    fn force_sets_every_field_to_upstream() {
        let u = up();
        let base = base_of(&u);
        let local = Local {
            title: "local".into(),
            description: None,
            priority: "low".into(),
            due: Some("2026-01-01T00:00:00.000Z".into()),
            start: None,
            tags: vec!["mine".into(), "backend".into()],
            assignees: vec!["user:mia".into(), "agent-x".into()],
        };
        let p = merge(&u, &local, &base, true);
        assert_eq!(p.title.as_deref(), Some("Ship it"));
        assert_eq!(p.description, Some(Some("plan".into())));
        assert_eq!(p.priority.as_deref(), Some("high"));
        assert_eq!(p.due, Some(None));
        assert_eq!(p.tags.unwrap(), vec!["backend".to_string()]);
        assert_eq!(
            p.assignees.unwrap(),
            vec!["agent-x".to_string(), "user:dana".to_string()]
        );
        assert_eq!(p.status.as_deref(), Some("in progress"));
    }

    #[test]
    fn an_unseeded_item_is_a_first_sync() {
        let u = up();
        let mut base = base_of(&u);
        base.seeded = false;
        let p = merge(&u, &Local::default(), &base, false);
        assert!(p.writes());
        assert_eq!(p.status.as_deref(), Some("in progress"));
    }

    #[test]
    fn a_held_status_is_asked_again_next_time() {
        // The caller left applied.status at the old value because a guard held
        // the move: the next merge asks for the move again.
        let u0 = up();
        let mut base = base_of(&u0);
        base.status = Some("to do".into());
        let p = merge(&u0, &local_of(&u0), &base, false);
        assert_eq!(p.status.as_deref(), Some("in progress"));
    }
}
