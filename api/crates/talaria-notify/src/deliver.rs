// What happens around a write: the delivery decision taken inside the write's
// transaction, and the tail after commit. The row is the record; every
// channel here is a delivery on top of it (docs/notifications/delivery.md).
//
// THE DECISION (`decide`, pure), from the class's channels (prefs v2), the
// person's DND and working hours, what the producer said (`NotifyMeta`), the
// instance switches, and whether one of the person's clients is in use:
//
//   breaks = meta.force_alert || (class breaks through && dnd.allowNeedsYou)
//   held   = dnd.active && !breaks
//   alert  = Banner iff Desktop on && !quiet && !held, else Feed     (I-15)
//   push   = Push on && pushEnabled && !quiet && !skip_push && !held:
//              a client looking or active → deferred 60 s (the ack or a read
//              cancels it, defer.rs); nobody there → now
//   email  = Email on && emailEnabled && !quiet && frequency != off:
//              due after 15 min (Needs you) or the person's frequency, and
//              never before a pause ends; the batcher sends it (mail.rs)
//
// After commit, a push decided "now" goes out with the category's urgency and
// lifetime and the row id as its Topic. Nothing else is sent from here: the
// deferred push and the mail are durable columns on the row, so a restart
// loses neither.

use serde_json::Value;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_push::{PushNote, Urgency};

use crate::classes::{Category, class_def};
use crate::meta::{Alert, Coalesce, Filed, NotifyMeta};
use crate::prefs::{
    DeliverySwitches, FOREVER_MS, allow_needs_you, channels_of, dnd_state, email_frequency,
    legacy_route, next_local_time,
};
use crate::write::NotificationInput;

/// The delivery engine's own feature flags (I-23): Following is quiet by
/// default, DND and working hours are enforced on the server, and
/// missed-activity mail is batched.
pub const FEATURES: &[&str] = &["email-batch", "quiet-following", "dnd-enforced"];

/// How long a push waits while one of the person's clients is in use (O-13).
pub const PUSH_DEFER_MS: i64 = crate::presence::PUSH_DEFER_MS;

/// What a write does about web push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushPlan {
    /// Not pushed: the channel is off, the producer muted it, DND holds it,
    /// or this process has no push plane.
    Skip,
    /// Nobody is at a screen: push now.
    Now,
    /// A client is in use: push at `push_due_ms` unless a client acks the row
    /// or the person reads it first.
    Defer,
}

/// What a write will do beyond landing, decided from the person's prefs as
/// the write's own transaction read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The class's legacy route — what a rolled-back binary reads.
    pub route: &'static str,
    pub category: Category,
    /// How clients present the row (I-15).
    pub alert: Alert,
    pub push: PushPlan,
    /// When a deferred push is due, epoch ms.
    pub push_due_ms: Option<i64>,
    /// When the missed-activity mail for this row is due, epoch ms.
    pub email_due_ms: Option<i64>,
    /// DND held this row: no banner, no push, mail after the pause.
    pub held: bool,
}

impl Decision {
    /// Whether this write records `pushed_at` itself (a push now).
    pub fn records_push(&self) -> bool {
        self.push == PushPlan::Now
    }
}

/// What the write knows beyond the person's blob.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The person's time zone (`users.timezone`, else UTC).
    pub zone: &'a str,
    pub now_ms: i64,
    pub switches: DeliverySwitches,
    /// One of the person's clients is looking or active (presence winner).
    pub present: bool,
    /// This process can push (the plane is installed at boot).
    pub plane: bool,
    /// A test-only override of every mail delay, in seconds.
    pub email_delay_override_secs: Option<u64>,
}

/// The decision for one write. Pure.
pub fn decide(blob: &Value, class: &str, meta: &NotifyMeta, cx: &Context<'_>) -> Decision {
    let def = class_def(class).or_else(|| class_def(crate::classes::FALLBACK_CLASS));
    let category = def.map(|d| d.category).unwrap_or(Category::Following);
    let channels = channels_of(blob, class);
    let route = legacy_route(channels);
    let dnd = dnd_state(blob, cx.zone, cx.now_ms);
    let breaks =
        meta.force_alert || (def.is_some_and(|d| d.breaks_through) && allow_needs_you(blob));
    let held = dnd.active && !breaks;
    let alert = if channels.desktop && !meta.quiet && !held {
        Alert::Banner
    } else {
        Alert::Feed
    };
    let wants_push =
        channels.push && cx.switches.push && cx.plane && !meta.quiet && !meta.skip_push && !held;
    let push = match (wants_push, cx.present) {
        (false, _) => PushPlan::Skip,
        (true, true) => PushPlan::Defer,
        (true, false) => PushPlan::Now,
    };
    let push_due_ms = (push == PushPlan::Defer).then_some(cx.now_ms + PUSH_DEFER_MS);
    let email_due_ms = if channels.email && cx.switches.email && !meta.quiet {
        email_due(blob, def.map(|d| d.email_delay_min).unwrap_or(0), cx).and_then(|due| {
            if !held {
                return Some(due);
            }
            // Never before the pause ends; a pause with no end ("until I
            // resume", a schedule with no working day) mails nothing — the
            // person comes back to the feed.
            match dnd.until {
                Some(until) if until < FOREVER_MS => Some(due.max(until)),
                _ => None,
            }
        })
    } else {
        None
    };
    Decision {
        route,
        category,
        alert,
        push,
        push_due_ms,
        email_due_ms,
        held,
    }
}

/// When the missed-activity mail for a row filed now is due: `delay_min`
/// minutes when the class names one (Needs you: 15), else the person's
/// frequency — 15 minutes, an hour, or the next 09:00 on their own clock —
/// and never for `off`.
pub fn email_due(blob: &Value, delay_min: u32, cx: &Context<'_>) -> Option<i64> {
    let frequency = email_frequency(blob);
    if frequency == "off" {
        return None;
    }
    if let Some(secs) = cx.email_delay_override_secs {
        return Some(cx.now_ms + secs as i64 * 1000);
    }
    if delay_min > 0 {
        return Some(cx.now_ms + i64::from(delay_min) * 60_000);
    }
    Some(match frequency {
        "15m" => cx.now_ms + 15 * 60_000,
        "1d" => next_local_time(cx.zone, cx.now_ms, 9 * 60),
        _ => cx.now_ms + 60 * 60_000,
    })
}

/// A push's lifetime and urgency by category (WP-12, BT-10): a decision a
/// person is blocking is worth waking a phone for and stays deliverable for a
/// day; a direct message for half a day; news for an hour, at low urgency.
pub fn push_policy(category: Category) -> (u32, Urgency) {
    match category {
        Category::NeedsYou => (86_400, Urgency::High),
        Category::Direct => (43_200, Urgency::Normal),
        Category::Following | Category::Workspace => (3_600, Urgency::Low),
    }
}

/// The push `Topic` for a row: its id without dashes — 32 characters of the
/// URL-safe alphabet — so a re-alert of the row replaces a push the service
/// has not delivered yet instead of queueing a second one.
pub fn push_topic(id: &str) -> String {
    id.chars().filter(|c| *c != '-').collect()
}

/// The note one row pushes as.
pub fn push_note(
    id: &str,
    kind: &str,
    title: &str,
    body: &str,
    href: &str,
    renotify: bool,
    created_ms: i64,
) -> PushNote {
    let (ttl, urgency) = push_policy(crate::classes::category_of(kind));
    PushNote {
        id: id.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        href: href.to_string(),
        kind: kind.to_string(),
        renotify,
        created_at: epoch_ms_to_iso(created_ms),
        ttl_secs: Some(ttl),
        urgency: Some(urgency),
        topic: Some(push_topic(id)),
    }
}

/// The tail of a write, after commit and the announcement. A fresh row, or a
/// `Realert` coalesce, pushes now when the decision says so; a deferred push
/// and the mail are already on the row for their jobs.
pub(crate) fn after_write(
    user_id: &str,
    n: &NotificationInput<'_>,
    meta: &NotifyMeta,
    filed: &Filed,
    d: &Decision,
    href: &str,
) {
    let alerts = filed.inserted || meta.coalesce == Coalesce::Realert;
    if d.push == PushPlan::Now && alerts {
        // Detached and quiet like the nudge: the row is the record, the
        // delivery is best-effort on top.
        talaria_push::push_notification(
            user_id,
            push_note(
                &filed.id,
                n.kind,
                n.title,
                n.body.unwrap_or(""),
                href,
                !filed.inserted,
                filed.created_ms,
            ),
        );
    }
}

/// The tail of `refresh_notification`, after the row was brought up to date:
/// pushed again now as a re-alert when the decision says so; a deferral and
/// the mail are on the row for their jobs.
pub(crate) fn after_refresh(
    user_id: &str,
    n: &NotificationInput<'_>,
    id: &str,
    created_ms: i64,
    href: &str,
    d: &Decision,
) {
    if d.push == PushPlan::Now {
        talaria_push::push_notification(
            user_id,
            push_note(
                id,
                n.kind,
                n.title,
                n.body.unwrap_or(""),
                href,
                true,
                created_ms,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::DeliverySwitches;
    use serde_json::json;

    const NOW: i64 = 1_767_268_800_000; // 2026-01-01T12:00:00Z, a Thursday

    fn cx() -> Context<'static> {
        Context {
            zone: "UTC",
            now_ms: NOW,
            switches: DeliverySwitches {
                email: true,
                push: true,
            },
            present: false,
            plane: true,
            email_delay_override_secs: None,
        }
    }

    fn m() -> NotifyMeta {
        NotifyMeta::default()
    }

    #[test]
    fn the_matrix_decides_each_channel_on_its_own() {
        // Defaults: a mention banners, pushes now and mails in an hour.
        let d = decide(&json!({}), "mention", &m(), &cx());
        assert_eq!(
            (d.alert, d.push, d.category, d.held),
            (Alert::Banner, PushPlan::Now, Category::Direct, false)
        );
        assert_eq!(d.email_due_ms, Some(NOW + 3_600_000));
        assert_eq!(d.push_due_ms, None);
        // Desktop off: a feed row, still pushed (Push is its own channel).
        let blob = json!({ "channels": { "mention": { "desktop": false } } });
        let d = decide(&blob, "mention", &m(), &cx());
        assert_eq!((d.alert, d.push), (Alert::Feed, PushPlan::Now));
        // Push off: banners, no push, still mailed.
        let blob = json!({ "channels": { "mention": { "push": false } } });
        let d = decide(&blob, "mention", &m(), &cx());
        assert_eq!((d.alert, d.push), (Alert::Banner, PushPlan::Skip));
        assert!(d.email_due_ms.is_some());
        // Email off: no mail; the legacy route reads in_app.
        let blob = json!({ "channels": { "mention": { "email": false } } });
        let d = decide(&blob, "mention", &m(), &cx());
        assert_eq!((d.email_due_ms, d.route), (None, "in_app"));
        // The legacy email-only route: a feed row, no push, mail.
        let d = decide(&json!({ "dm": "email" }), "dm", &m(), &cx());
        assert_eq!(
            (d.alert, d.push, d.route),
            (Alert::Feed, PushPlan::Skip, "email")
        );
        assert!(d.email_due_ms.is_some());
    }

    #[test]
    fn a_producers_mute_and_the_instance_switches_win() {
        let quiet = NotifyMeta {
            quiet: true,
            ..Default::default()
        };
        let d = decide(&json!({}), "mention", &quiet, &cx());
        assert_eq!(
            (d.alert, d.push, d.email_due_ms),
            (Alert::Feed, PushPlan::Skip, None)
        );
        let skip = NotifyMeta {
            skip_push: true,
            ..Default::default()
        };
        let d = decide(&json!({}), "mention", &skip, &cx());
        assert_eq!((d.alert, d.push), (Alert::Banner, PushPlan::Skip));
        assert!(d.email_due_ms.is_some());
        let mut off = cx();
        off.switches = DeliverySwitches {
            email: false,
            push: false,
        };
        let d = decide(&json!({}), "mention", &m(), &off);
        assert_eq!((d.push, d.email_due_ms), (PushPlan::Skip, None));
        assert_eq!(d.alert, Alert::Banner, "the switches are not the matrix");
        // No plane in this process: nothing to push with.
        let mut planeless = cx();
        planeless.plane = false;
        assert_eq!(
            decide(&json!({}), "mention", &m(), &planeless).push,
            PushPlan::Skip
        );
    }

    #[test]
    fn following_is_quiet_unless_a_route_was_stored() {
        for kind in ["task-status", "research", "brand-new-kind"] {
            let class = crate::classes::notify_class_of(kind);
            let d = decide(&json!({}), class, &m(), &cx());
            assert_eq!(
                (d.alert, d.push, d.email_due_ms),
                (Alert::Feed, PushPlan::Skip, None),
                "{kind}"
            );
        }
        // A person who stored a legacy route keeps it: `both` banners,
        // pushes and mails.
        let d = decide(
            &json!({ "work_complete": "both" }),
            "work_complete",
            &m(),
            &cx(),
        );
        assert_eq!((d.alert, d.push), (Alert::Banner, PushPlan::Now));
        assert!(d.email_due_ms.is_some());
        // The account class (the Google principal notice) alerts and mails.
        let d = decide(&json!({}), "account", &m(), &cx());
        assert_eq!(
            (d.alert, d.push, d.category),
            (Alert::Banner, PushPlan::Now, Category::Direct)
        );
    }

    #[test]
    fn a_present_client_defers_the_push() {
        let mut here = cx();
        here.present = true;
        let d = decide(&json!({}), "dm", &m(), &here);
        assert_eq!(d.push, PushPlan::Defer);
        assert_eq!(d.push_due_ms, Some(NOW + 60_000));
        assert!(!d.records_push());
        assert_eq!(d.alert, Alert::Banner, "presence does not change the row");
    }

    #[test]
    fn dnd_holds_the_row_and_needs_you_breaks_through() {
        let until = NOW + 3_600_000;
        let paused = json!({ "dnd": { "pausedUntil": epoch_ms_to_iso(until) } });
        let d = decide(&paused, "dm", &m(), &cx());
        assert!(d.held);
        assert_eq!((d.alert, d.push), (Alert::Feed, PushPlan::Skip));
        // Mail waits for the pause: due at the later of the delay and its end.
        assert_eq!(d.email_due_ms, Some(until.max(NOW + 3_600_000)));
        let short = json!({ "dnd": { "pausedUntil": epoch_ms_to_iso(NOW + 2 * 3_600_000) } });
        assert_eq!(
            decide(&short, "dm", &m(), &cx()).email_due_ms,
            Some(NOW + 2 * 3_600_000)
        );
        // A Needs-you class breaks through: banner, push, its 15-minute mail.
        let d = decide(&paused, "approval_pending", &m(), &cx());
        assert!(!d.held);
        assert_eq!((d.alert, d.push), (Alert::Banner, PushPlan::Now));
        assert_eq!(d.email_due_ms, Some(NOW + 15 * 60_000));
        // …unless the person said no break-through.
        let strict =
            json!({ "dnd": { "pausedUntil": epoch_ms_to_iso(until), "allowNeedsYou": false } });
        let d = decide(&strict, "approval_pending", &m(), &cx());
        assert_eq!(
            (d.alert, d.push, d.held),
            (Alert::Feed, PushPlan::Skip, true)
        );
        // A producer's force_alert always does.
        let forced = NotifyMeta {
            force_alert: true,
            ..Default::default()
        };
        let d = decide(&strict, "dm", &forced, &cx());
        assert_eq!((d.alert, d.push), (Alert::Banner, PushPlan::Now));
        // "Until I resume": no mail at all while held.
        let forever = json!({ "dnd": { "pausedUntil": crate::prefs::FOREVER } });
        assert_eq!(decide(&forever, "dm", &m(), &cx()).email_due_ms, None);
    }

    #[test]
    fn outside_working_hours_is_a_pause() {
        // Working hours 09:00–10:00 every day; now is 12:00 UTC.
        let blob = json!({ "dnd": { "schedule": {
            "enabled": true, "days": [0,1,2,3,4,5,6], "start": "09:00", "end": "10:00" } } });
        let d = decide(&blob, "dm", &m(), &cx());
        assert!(d.held);
        assert_eq!((d.alert, d.push), (Alert::Feed, PushPlan::Skip));
        // Mail is due when working hours start again: 09:00 tomorrow.
        assert_eq!(d.email_due_ms, Some(NOW + 21 * 3_600_000));
        // Inside them, nothing is held.
        let inside = json!({ "dnd": { "schedule": {
            "enabled": true, "days": [0,1,2,3,4,5,6], "start": "11:00", "end": "13:00" } } });
        assert!(!decide(&inside, "dm", &m(), &cx()).held);
    }

    #[test]
    fn the_mail_delay_follows_the_frequency() {
        let f = |freq: &str| json!({ "email": { "frequency": freq } });
        assert_eq!(email_due(&f("15m"), 0, &cx()), Some(NOW + 15 * 60_000));
        assert_eq!(email_due(&f("1h"), 0, &cx()), Some(NOW + 60 * 60_000));
        // 1d: the next 09:00 on the person's clock — tomorrow, from noon UTC.
        assert_eq!(email_due(&f("1d"), 0, &cx()), Some(NOW + 21 * 3_600_000));
        let mut ny = cx();
        ny.zone = "America/New_York"; // 07:00 local: today at 09:00 = 14:00Z
        assert_eq!(email_due(&f("1d"), 0, &ny), Some(NOW + 2 * 3_600_000));
        assert_eq!(email_due(&f("off"), 0, &cx()), None);
        // A Needs-you class waits its own 15 minutes, whatever the frequency…
        assert_eq!(email_due(&f("1d"), 15, &cx()), Some(NOW + 15 * 60_000));
        // …but `off` is off.
        assert_eq!(email_due(&f("off"), 15, &cx()), None);
        let mut fast = cx();
        fast.email_delay_override_secs = Some(5);
        assert_eq!(email_due(&f("1d"), 15, &fast), Some(NOW + 5_000));
    }

    #[test]
    fn push_headers_follow_the_category() {
        assert_eq!(push_policy(Category::NeedsYou), (86_400, Urgency::High));
        assert_eq!(push_policy(Category::Direct), (43_200, Urgency::Normal));
        assert_eq!(push_policy(Category::Following), (3_600, Urgency::Low));
        assert_eq!(push_policy(Category::Workspace), (3_600, Urgency::Low));
        let id = "5e000000-0000-4000-8000-0000000001a1";
        let topic = push_topic(id);
        assert_eq!(topic, id.replace('-', ""));
        assert_eq!(topic.len(), 32);
        assert!(talaria_push::valid_topic(&topic));
        let n = push_note(id, "approval_pending", "t", "b", "/x", false, NOW);
        assert_eq!(
            (n.ttl_secs, n.urgency, n.topic.as_deref()),
            (Some(86_400), Some(Urgency::High), Some(topic.as_str()))
        );
        assert_eq!(n.created_at, epoch_ms_to_iso(NOW));
        let n = push_note(id, "task-status", "t", "b", "/x", true, NOW);
        assert_eq!((n.urgency, n.renotify), (Some(Urgency::Low), true));
    }
}
