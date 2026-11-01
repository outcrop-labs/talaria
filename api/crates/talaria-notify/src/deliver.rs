// What happens around a write: the delivery decision taken inside the write's
// transaction (the row's `alert`, whether it pushes), and the tail after
// commit — the closed-tab push and the queued mail. The row is the record;
// everything here is best-effort on top of it.
//
// Phase 0 is wave 1's behaviour, moved here: push whenever the class's Push
// channel is on (read through prefs v2 — every state wave 1 could store has
// it on exactly when the route keeps an in-app copy, route != `email`), mail
// through the in-memory outbox when the route says so and the instance switch
// is on. W2-E owns this file and turns the decisions into the matrix, DND and
// presence (docs/notifications/delivery.md).

use serde_json::Value;
use talaria_agent_auth::epoch_ms_to_iso;

use crate::NotifyDeps;
use crate::mail::{delivery_gate, outbox};
use crate::meta::{Alert, Coalesce, Filed, NotifyMeta};
use crate::prefs::{channels_of, legacy_route};
use crate::write::NotificationInput;

/// The delivery engine's own feature flags (I-23). W2-E adds its own here.
pub const FEATURES: &[&str] = &[];

/// What a write will do beyond landing, decided from the person's prefs as
/// the write's own transaction read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The class's legacy route — wave 1's whole routing answer.
    pub route: &'static str,
    /// How clients present the row (I-15): `feed` when the producer muted it
    /// or the class's Desktop channel is off.
    pub alert: Alert,
    /// Web-push it (when the push plane is installed).
    pub push: bool,
    /// Queue a mail for it (when the instance switch is on).
    pub mail: bool,
}

impl Decision {
    /// Whether this write records `pushed_at`: it will push, and this process
    /// can.
    pub fn records_push(&self) -> bool {
        self.push && talaria_push::push_plane_installed()
    }
}

/// The decision for one write. Pure.
pub fn decide(blob: &Value, class: &str, meta: &NotifyMeta) -> Decision {
    let channels = channels_of(blob, class);
    let route = legacy_route(channels);
    Decision {
        route,
        alert: if meta.quiet || !channels.desktop {
            Alert::Feed
        } else {
            Alert::Banner
        },
        // Closed tabs ride the same row: whenever the class's Push channel is
        // on, the push plane fans it to every subscribed browser. For every
        // blob wave 1 could write that is exactly "the route keeps an in-app
        // copy" (in_app or both; email-only is the one route where the person
        // asked this platform to leave their screens alone) — `channels_of`
        // derives push from the route — so wave 1's behaviour is unchanged;
        // and a Push switch turned off in the v2 matrix is obeyed from the
        // contract on, whichever stream's UI writes it first.
        push: channels.push && !meta.quiet && !meta.skip_push,
        mail: matches!(route, "email" | "both") && !meta.quiet,
    }
}

/// The tail of a write, after commit and the announcement. A fresh row
/// pushes and mails; a coalesce re-pushes (as a re-alert) unless it was
/// `Quiet`, and never mails — the first row's mail stands, as a refresh's
/// does.
pub(crate) async fn after_write(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
    meta: &NotifyMeta,
    filed: &Filed,
    d: &Decision,
    href: &str,
) {
    let alerts = filed.inserted || meta.coalesce == Coalesce::Realert;
    if d.push && alerts {
        // Detached and quiet like the nudge: the row is the record, the
        // delivery is best-effort on top.
        talaria_push::push_notification(
            user_id,
            talaria_push::PushNote {
                id: filed.id.clone(),
                title: n.title.to_string(),
                body: n.body.unwrap_or("").to_string(),
                href: href.to_string(),
                kind: n.kind.to_string(),
                renotify: !filed.inserted,
                created_at: epoch_ms_to_iso(filed.created_ms),
                ..Default::default()
            },
        );
    }
    // The gate read is the 10s-cached one: a notification is filed on request
    // paths, and the drain re-reads fresh anyway, so the cache's whole job is
    // saving a settings query per mention.
    if d.mail && filed.inserted && delivery_gate(&deps.pg).await {
        // `both` deliberately passes null: that route wants the in-app copy to
        // stay unread whatever the mail does.
        outbox().enqueue(user_id, n, (d.route == "email").then(|| filed.id.clone()));
    }
}

/// The tail of `refresh_notification`, after the row was brought up to date:
/// pushed again as a re-alert when the route keeps an in-app copy; never
/// mailed.
pub(crate) fn after_refresh(
    user_id: &str,
    n: &NotificationInput<'_>,
    id: String,
    created_ms: i64,
    href: String,
    d: &Decision,
) {
    if d.push {
        talaria_push::push_notification(
            user_id,
            talaria_push::PushNote {
                id,
                title: n.title.to_string(),
                body: n.body.unwrap_or("").to_string(),
                href,
                kind: n.kind.to_string(),
                renotify: true,
                created_at: epoch_ms_to_iso(created_ms),
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Wave 1's routing, unchanged: push unless the route is email-only, mail
    // when it is email or both; a muted write is a feed row that does
    // neither.
    #[test]
    fn the_phase_zero_decision_is_wave_ones() {
        let m = NotifyMeta::default();
        let d = decide(&json!({}), "mention", &m);
        assert_eq!(
            d,
            Decision {
                route: "both",
                alert: Alert::Banner,
                push: true,
                mail: true
            }
        );
        let d = decide(&json!({}), "work_complete", &m);
        assert_eq!(
            d,
            Decision {
                route: "in_app",
                alert: Alert::Banner,
                push: true,
                mail: false
            }
        );
        let d = decide(&json!({ "dm": "email" }), "dm", &m);
        assert_eq!(
            d,
            Decision {
                route: "email",
                alert: Alert::Feed,
                push: false,
                mail: true
            }
        );
        let quiet = NotifyMeta {
            quiet: true,
            ..Default::default()
        };
        let d = decide(&json!({}), "mention", &quiet);
        assert_eq!(
            d,
            Decision {
                route: "both",
                alert: Alert::Feed,
                push: false,
                mail: false
            }
        );
        let skip = NotifyMeta {
            skip_push: true,
            ..Default::default()
        };
        assert!(!decide(&json!({}), "mention", &skip).push);
        // Desktop off in the matrix: still pushed (Push is its own channel),
        // but the row is a feed row.
        let blob = json!({ "channels": { "mention": { "desktop": false } } });
        let d = decide(&blob, "mention", &m);
        assert_eq!(d.alert, Alert::Feed);
        assert!(d.push);
        // Push off in the matrix: not pushed, though the route (email and a
        // screen) still reads `both` and the row still banners.
        let blob = json!({ "channels": { "mention": { "push": false } } });
        let d = decide(&blob, "mention", &m);
        assert_eq!(
            (d.route, d.alert, d.push, d.mail),
            ("both", Alert::Banner, false, true)
        );
        // Every legacy route wave 1 could store: push iff the route is not
        // email-only — wave 1's rule, through the channels.
        for (route, push) in [("in_app", true), ("both", true), ("email", false)] {
            let d = decide(&json!({ "dm": route }), "dm", &m);
            assert_eq!(d.push, push, "{route}");
            assert_eq!(d.route, route);
        }
    }
}
