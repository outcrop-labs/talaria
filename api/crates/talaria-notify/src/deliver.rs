// What happens after a row is written: the closed-tab push and the queued
// mail. The row is the record; everything here is best-effort on top of it.

use serde_json::Value;
use talaria_agent_auth::epoch_ms_to_iso;

use crate::NotifyDeps;
use crate::classes::notify_class_of;
use crate::mail::outbox;
use crate::prefs::{prefs_blob, resolve_prefs};
use crate::write::NotificationInput;

/// The tail of `add_notification`, after the row landed and was announced.
pub(crate) fn after_insert(
    user_id: &str,
    n: &NotificationInput<'_>,
    notification_id: &str,
    created_ms: i64,
    href: &str,
    route: &str,
    will_mail: bool,
) {
    // Closed tabs ride the same row: whenever the prefs route keeps an
    // in-app copy (in_app or both — email-only is the one route where the
    // person asked this platform to leave their screens alone), the push
    // plane fans it to every subscribed browser. Detached and quiet like the
    // nudge above: the row is the record, the delivery is best-effort on top.
    if route != "email" {
        talaria_push::push_notification(
            user_id,
            talaria_push::PushNote {
                id: notification_id.to_string(),
                title: n.title.to_string(),
                body: n.body.unwrap_or("").to_string(),
                href: href.to_string(),
                kind: n.kind.to_string(),
                renotify: false,
                created_at: epoch_ms_to_iso(created_ms),
            },
        );
    }
    if will_mail {
        // `both` deliberately passes null: that route wants the in-app copy to
        // stay unread whatever the mail does.
        outbox().enqueue(
            user_id,
            n,
            if route == "email" {
                Some(notification_id.to_string())
            } else {
                None
            },
        );
    }
}

/// The tail of `refresh_notification`, after the row was brought up to date.
pub(crate) async fn after_refresh(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
    id: String,
    created_ms: i64,
    href: String,
) {
    let class = notify_class_of(n.kind);
    let blob = prefs_blob(&deps.pg, user_id).await.unwrap_or(Value::Null);
    let route = resolve_prefs(&blob)
        .get(class)
        .and_then(|v| v.as_str())
        .unwrap_or("in_app")
        .to_string();
    if route != "email" {
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
            },
        );
    }
}
