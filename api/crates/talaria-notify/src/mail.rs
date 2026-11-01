// The mail plane — the one door out, the missed-activity batcher, and the
// wave-1 outbox with its breaker and drain. The transport itself
// (SMTP/Resend) and the shared mail chrome live in talaria-email; the digest
// builds on the same door.
//
// Notification mail is the BATCHER's (W2-E, docs/notifications/delivery.md):
// a write sets `email_due_at` on the row, and the batcher sends each person
// ONE mail with every row of theirs that came due and is still unread —
// durable (the state is the row), cancelled by a read, retried with backoff.
// The in-memory outbox below is no longer fed by the write path; its drain
// stays registered for one release so a queue accepted by the previous
// binary still drains (PLAN3 §13 retires it).

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};

use futures_util::future::BoxFuture;
use serde_json::Value;
use sqlx::PgPool;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_realtime::RealtimeDeps;

use crate::NotificationInput;
use crate::NotifyDeps;
use crate::classes::{Category, class_def, kind_reason, notify_class_of};
use crate::prefs::{
    DELIVERY_KEY, DeliverySwitches, allow_needs_you, channels_of, dnd_state, email_frequency,
    prefs_and_zone, switches_of,
};

// ── THE ONE DOOR OUT ─────────────────────────────────────────────────────────
//
// Every mail Talaria sends ON BEHALF OF A NOTIFICATION OR A DIGEST leaves
// through `send_gated_mail`, and nothing else in this crate may call the
// transport for that purpose — scripts/check-invariants.mjs fails the tree on
// a caller that tries, because this exact bug has been written twice:
//
//   · notification mail asked the switch, so it was gated.
//   · the digest called the transport directly and never asked, so an admin
//     who turned email OFF still got a daily digest mailed to every user in
//     the workspace — from a control that is named "email delivery", is
//     audited, and did not do what its name said.
//
// The fix is not another `if enabled` at the second call site. Two copies of
// a rule is how the first one drifts. It is one function both paths must go
// through (the digest crosses through this same door).
//
// It reads the switch FRESH on every single mail, not once per batch. A kill
// switch you throw at 03:00 has to stop the drain that is already running, not
// the one after it — and one small settings read per mail is nothing next to
// an SMTP round trip.

async fn read_delivery_switch(pg: &PgPool) -> Result<bool, sqlx::Error> {
    let stored: Option<Value> = sqlx::query_scalar("select value from app_settings where key = $1")
        .bind(DELIVERY_KEY)
        .fetch_optional(pg)
        .await?;
    // Anything that is not exactly `true` is off. A hand-edited settings row
    // must not be able to turn mail on by accident.
    Ok(stored.as_ref().and_then(|v| v.get("emailEnabled")) == Some(&Value::Bool(true)))
}

/// The 10s-cached switch read the write path decides with: both instance
/// switches, from one row. A failed read is the default (no mail; push as
/// wave 1) and is NOT cached: a blip must not pin the wrong answer for ten
/// seconds. The batcher and every send re-read fresh anyway, so the cache's
/// whole job is saving a settings query per notification.
const DELIVERY_TTL_MS: i64 = 10_000;
static DELIVERY_GATE: LazyLock<Mutex<Option<(i64, DeliverySwitches)>>> =
    LazyLock::new(|| Mutex::new(None));

pub(crate) async fn delivery_switches(pg: &PgPool) -> DeliverySwitches {
    let now = wall_ms();
    if let Some((at, v)) = *DELIVERY_GATE.lock().unwrap_or_else(|p| p.into_inner())
        && now - at < DELIVERY_TTL_MS
    {
        return v;
    }
    let read: Result<Option<Value>, sqlx::Error> =
        sqlx::query_scalar("select value from app_settings where key = $1")
            .bind(DELIVERY_KEY)
            .fetch_optional(pg)
            .await;
    match read {
        Ok(v) => {
            let v = switches_of(&v.unwrap_or(Value::Null));
            *DELIVERY_GATE.lock().unwrap_or_else(|p| p.into_inner()) = Some((now, v));
            v
        }
        Err(_) => DeliverySwitches::default(),
    }
}

/// What send_gated_mail reports. `blocked` is the instance switch being OFF —
/// explicitly NOT a failure: nothing was attempted, no transport was involved,
/// and nothing should be retried until an admin turns it back on. Callers
/// that count failures (the outbox breaker, the digest's retry) must not
/// count it.
#[derive(Clone, Debug, PartialEq)]
pub struct GatedSendResult {
    pub ok: bool,
    pub blocked: bool,
    pub error: Option<String>,
}

// One log line a minute is enough to say "mail is off and things are being
// refused". Per mail, on a workspace-wide digest pass, it is the outage.
static BLOCKED_WARN: LazyLock<Mutex<(i64, i64)>> = LazyLock::new(|| Mutex::new((0, 0)));

/// One mail offered to the door.
pub struct GatedMail<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub html: &'a str,
    pub text: Option<&'a str>,
    /// Becomes `List-Unsubscribe` — every mail that goes to a person because
    /// of a standing preference needs it. Callers pass the settings URL or
    /// nothing: a header pointing at a URL that does not resolve is worse
    /// than no header, so the no-verified-domain case omits it.
    pub unsubscribe_url: Option<&'a str>,
    /// What is being refused, for the log line.
    pub what: &'a str,
}

/// Send one piece of notification-or-digest mail, IF this instance is allowed
/// to send mail at all.
pub async fn send_gated_mail(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    mail: &GatedMail<'_>,
) -> GatedSendResult {
    let on = match read_delivery_switch(pg).await {
        Ok(on) => on,
        Err(e) => {
            // We do not know whether we are allowed to send, so we do not
            // send. Failing OPEN here would mean a transient database blip
            // mails the workspace out of an instance whose operator had
            // switched mail off — the one outcome that cannot be taken back.
            tracing::error!(
                "[mail] could not read the delivery switch — not sending \"{}\": {e}",
                mail.what
            );
            return GatedSendResult {
                ok: false,
                blocked: false,
                error: Some("could not read the email delivery switch".into()),
            };
        }
    };
    if !on {
        let mut w = BLOCKED_WARN.lock().unwrap_or_else(|p| p.into_inner());
        w.1 += 1;
        let now = wall_ms();
        if now - w.0 > 60_000 {
            tracing::warn!(
                "[mail] email delivery is OFF for this instance — refused {} mail(s), most recently \"{}\". An admin can turn it back on under Settings → Notifications.",
                w.1,
                mail.what
            );
            *w = (now, 0);
        }
        return GatedSendResult {
            ok: false,
            blocked: true,
            error: Some("email delivery is switched off for this instance".into()),
        };
    }
    let mut input = talaria_email::EmailInput::new(mail.to, mail.subject, mail.html, mail.text);
    if let Some(url) = mail.unsubscribe_url {
        input
            .headers
            .push(("List-Unsubscribe".into(), format!("<{url}>")));
    }
    match talaria_email::send_email(pg, sb, &input).await {
        talaria_email::SendOutcome::Sent => GatedSendResult {
            ok: true,
            blocked: false,
            error: None,
        },
        talaria_email::SendOutcome::Failed(e) => GatedSendResult {
            ok: false,
            blocked: false,
            error: Some(e),
        },
    }
}

// ── The outbox ───────────────────────────────────────────────────────────────
//
// Serial, not a fan of parallel sends: a ticket that notifies eight watchers
// would otherwise open eight SMTP connections at once, and a burst is exactly
// when a provider starts rate-limiting.
//
// A real queue, with the four things a queue needs to be safe:
//   DEPTH    — MAX_QUEUED, and past it new mail is refused at the door.
//   DROP     — refuse the NEWEST rather than evicting the oldest. Load
//              shedding at the entrance is predictable and it never throws
//              away work already accepted. The in-app row has landed either
//              way, so a dropped mail costs a notification its second
//              delivery channel, not its existence.
//   BREAKER  — after enough consecutive failures the drain stops trying for a
//              while. Without it a dead transport burns the full send deadline
//              per item forever, on every tick, and the log is the outage.
//   COUNTERS — sent, failed, dropped, discarded. A queue you cannot see is
//              indistinguishable from a queue that is working.

/// How many mails may wait. Sized for the realistic worst case — an agent
/// fleet fanning a burst of problems out to every admin — and small enough
/// that the memory is irrelevant. If this is ever hit, the answer is not a
/// bigger number (the transport is not keeping up and a larger queue only
/// delays the same outcome) — it is the alert the log line is there to
/// produce.
pub const MAX_QUEUED: usize = 500;

/// How long one drain may spend sending before it yields to the next tick.
/// Shorter than the job's interval so a drain does not routinely overlap its
/// own schedule.
pub const DRAIN_BUDGET_MS: i64 = 25_000;

/// Consecutive failures before the breaker opens, and how long it stays open.
/// Five because a single bad address should not stop everyone else's mail,
/// and five in a row is a transport, not an address.
pub const BREAKER_AFTER: u32 = 5;
pub const BREAKER_COOLDOWN_MS: i64 = 5 * 60_000;

/// How many times one mail is offered to the transport before it is given up
/// on. Three because the failures worth retrying are the transient ones — a
/// timeout, a 429, a provider blip — and a bad address fails identically
/// three times and costs nothing but three log lines. Past it the mail is
/// abandoned OUT LOUD, and the notification is still sitting unread in the
/// recipient's in-app inbox, which is the whole point of not pre-filing it
/// read.
pub const MAX_SEND_ATTEMPTS: u32 = 3;

/// One accepted mail. The notification fields are OWNED copies — the queue
/// outlives the request that filed the row. Clone because the drain PEEKS: the
/// send works on a clone while the row itself stays in the queue for the
/// whole await.
#[derive(Clone)]
struct QueuedMail {
    user_id: String,
    kind: String,
    title: String,
    body: Option<String>,
    href: Option<String>,
    queued_at: i64,
    mark_read_id: Option<String>,
    attempts: u32,
}

struct OutboxState {
    queue: VecDeque<QueuedMail>,
    dropped: usize,
    discarded: usize,
    sent: usize,
    failed: usize,
    abandoned: usize,
    consecutive_failures: u32,
    paused_until: i64,
}

/// The queue plus its two locks. The std mutex covers only short synchronous
/// sections (push, peek, shift, counters) and is NEVER held across an await —
/// that is what makes enqueue cheap enough to call from a request path while
/// a drain is mid-send. The tokio mutex below it is the serial consumer.
pub struct Outbox {
    state: Mutex<OutboxState>,
    serial: tokio::sync::Mutex<()>,
}

fn outbox_state() -> OutboxState {
    OutboxState {
        queue: VecDeque::new(),
        dropped: 0,
        discarded: 0,
        sent: 0,
        failed: 0,
        abandoned: 0,
        consecutive_failures: 0,
        paused_until: 0,
    }
}

impl Outbox {
    /// Accept one mail for later sending, or refuse it. NEVER sends, never
    /// blocks, never fails. The write path no longer calls it (the batcher
    /// sends notification mail); it stays for the drain's one-release tail.
    pub fn enqueue(&self, user_id: &str, n: &NotificationInput<'_>, mark_read_id: Option<String>) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.queue.len() >= MAX_QUEUED {
            s.dropped += 1;
            // The first drop is the interesting one; after that, one line per
            // hundred is enough to show it is still happening without
            // becoming the outage.
            if s.dropped == 1 || s.dropped.is_multiple_of(100) {
                tracing::error!(
                    "[notifications] mail queue is full at {MAX_QUEUED} — dropping \"{}\" for user {user_id} ({} dropped since boot). The in-app notification still landed. The transport is not keeping up.",
                    n.kind,
                    s.dropped
                );
            }
            return;
        }
        s.queue.push_back(QueuedMail {
            user_id: user_id.to_string(),
            kind: n.kind.to_string(),
            title: n.title.to_string(),
            body: n.body.map(str::to_string),
            href: n.href.map(str::to_string),
            queued_at: wall_ms(),
            mark_read_id,
            attempts: 0,
        });
    }

    async fn drain(&self, deps: &DrainDeps, budget_ms: i64) -> MailDrainResult {
        let _serial = self.serial.lock().await;
        let mut result = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            MailDrainResult {
                sent: 0,
                failed: 0,
                requeued: 0,
                abandoned: 0,
                discarded: 0,
                remaining: s.queue.len(),
                dropped: s.dropped,
                paused: false,
            }
        };

        // Read the switch FRESH, every drain, and treat it as authoritative
        // over anything already queued. "Off" has to mean no mail leaves —
        // including the mail that was accepted while it was still on. That is
        // what makes it a kill switch rather than a setting that takes effect
        // eventually.
        let on = match (deps.delivery)().await {
            Ok(on) => on,
            Err(e) => {
                // The switch could not be read, so we do not know whether we
                // are allowed to send. Fail CLOSED and keep the queue: sending
                // on an unread switch is the one outcome that cannot be taken
                // back, and a transient database blip must not also destroy
                // the backlog.
                tracing::error!(
                    "[notifications] could not read the delivery switch — sending nothing this pass: {e}"
                );
                return result;
            }
        };
        if !on {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !s.queue.is_empty() {
                result.discarded = s.queue.len();
                s.discarded += result.discarded;
                // Dropped rather than held. Held mail is a trap: an admin who
                // turns email on for the first time three months in would get
                // the entire backlog of every mention since boot delivered in
                // one burst, which is precisely the mass-mailing this switch
                // exists to prevent.
                let n = result.discarded;
                s.queue.clear();
                tracing::warn!(
                    "[notifications] email delivery is off — discarded {n} queued mail(s). Every one of them is still in its recipient's in-app inbox."
                );
            }
            result.remaining = 0;
            return result;
        }

        if (deps.now)() < self.paused_until() {
            result.paused = true;
            return result;
        }

        let deadline = (deps.now)() + budget_ms;
        // Bounded by what was queued when this pass STARTED. A failed mail
        // goes back on the queue now (see below) and without this bound a
        // transport that is refusing everything would spin the same handful of
        // items round the loop for the whole budget instead of yielding to the
        // next tick. Each item gets at most one attempt per drain.
        let mut offers = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.queue.len()
        };
        while offers > 0 {
            offers -= 1;
            // PEEKED, not popped. Taking the item off the queue before the
            // send would make a failure destroy it: a mail that timed out
            // would simply vanish — nothing requeued, nothing retried, and no
            // in-app trace of it either. Nothing leaves this queue until it
            // has been delivered, refused by the switch, or given up on out
            // loud. (The clone is the peek: the row itself stays in the queue
            // for the whole await.)
            let Some(item) = self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .queue
                .front()
                .cloned()
            else {
                break;
            };
            if (deps.now)() >= deadline {
                break;
            }
            let r = (deps.send)(QueuedMailPayload::from(&item)).await;
            if r.blocked {
                // The switch went off WHILE this drain was running. Same rule
                // as the top of this function, applied to the mail in hand as
                // well as the backlog: stop now, throw the rest away, and
                // never report a refused mail as sent. `item` was PEEKED, not
                // popped, so it is discarded with the rest.
                let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                let lost = s.queue.len();
                result.discarded += lost;
                s.discarded += lost;
                s.queue.clear();
                tracing::warn!(
                    "[notifications] email delivery was switched off mid-drain — discarded {lost} queued mail(s). Every one of them is still in its recipient's in-app inbox."
                );
                break;
            }
            if r.ok {
                {
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.queue.pop_front();
                    s.sent += 1;
                    s.consecutive_failures = 0;
                    result.sent += 1;
                }
                // `delivered` and `ok` are not the same answer: a recipient
                // with no address is `ok` and nothing left the building, so
                // the row must stay unread. Only a message a provider ACCEPTED
                // files it read.
                if r.delivered
                    && let Some(id) = item.mark_read_id
                {
                    (deps.mark_read)(id).await;
                }
            } else {
                let mut item = item;
                item.attempts += 1;
                let trip_breaker;
                {
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.queue.pop_front();
                    s.failed += 1;
                    s.consecutive_failures += 1;
                    result.failed += 1;
                    if item.attempts < MAX_SEND_ATTEMPTS {
                        // Back of the queue, not the front: everything else
                        // queued deserves an attempt before this one gets a
                        // second. Pushed unconditionally even at MAX_QUEUED —
                        // it was accepted once already, and refusing it here
                        // would be the silent loss this whole design is about.
                        s.queue.push_back(item.clone());
                        result.requeued += 1;
                    } else {
                        s.abandoned += 1;
                        result.abandoned += 1;
                        tracing::error!(
                            "[notifications] giving up on the \"{}\" email for user {} after {} attempts (last error: {}). The notification itself is unread in their in-app inbox — the email was lost, the notification was not.",
                            item.kind,
                            item.user_id,
                            item.attempts,
                            r.error.as_deref().unwrap_or("unknown error")
                        );
                    }
                    trip_breaker = s.consecutive_failures >= BREAKER_AFTER;
                    if trip_breaker {
                        s.paused_until = (deps.now)() + BREAKER_COOLDOWN_MS;
                        result.paused = true;
                        let left = s.queue.len();
                        tracing::error!(
                            "[notifications] {} sends failed in a row — pausing delivery for 5 minutes. {left} mail(s) still queued; they will be attempted after the pause, or dropped if the queue fills first.",
                            s.consecutive_failures
                        );
                    }
                }
                if trip_breaker {
                    break;
                }
            }
        }

        result.remaining = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queue
            .len();
        result
    }

    fn paused_until(&self) -> i64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .paused_until
    }

    fn stats(&self, now: i64) -> NotificationMailStats {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let oldest = s
            .queue
            .iter()
            .map(|m| m.queued_at)
            .min()
            .map(|at| (now - at).max(0));
        NotificationMailStats {
            queued: s.queue.len(),
            capacity: MAX_QUEUED,
            oldest_queued_ms: oldest,
            sent: s.sent,
            failed: s.failed,
            abandoned: s.abandoned,
            dropped: s.dropped,
            discarded: s.discarded,
            paused_for_ms: (s.paused_until - now).max(0),
        }
    }
}

/// The process outbox — one per process, because the job that drains it is
/// `per_instance`: no other instance can see this queue.
pub fn outbox() -> &'static Outbox {
    static OUTBOX: LazyLock<Outbox> = LazyLock::new(|| Outbox {
        state: Mutex::new(outbox_state()),
        serial: tokio::sync::Mutex::new(()),
    });
    &OUTBOX
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailDrainResult {
    pub sent: usize,
    pub failed: usize,
    /// Failed this pass and put BACK on the queue for the next one.
    pub requeued: usize,
    /// Failed MAX_SEND_ATTEMPTS times and given up on. The notification is
    /// still unread in the recipient's inbox; only the email was lost.
    pub abandoned: usize,
    /// Queued mail thrown away because delivery is switched off.
    pub discarded: usize,
    /// Still queued when this drain gave up its slot.
    pub remaining: usize,
    /// Refused at the door since boot.
    pub dropped: usize,
    /// The breaker was open, so nothing was attempted.
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NotificationMailStats {
    pub queued: usize,
    /// How many the queue may hold before it starts refusing at the door, so
    /// a reader can say "412 of 500" instead of picking its own idea of "a
    /// lot".
    pub capacity: usize,
    /// How long the oldest thing on the queue has been waiting. THE number
    /// for a stuck outbox: depth alone cannot tell a busy minute from a queue
    /// that has not moved since Tuesday, and this can. None when the queue is
    /// empty.
    pub oldest_queued_ms: Option<i64>,
    pub sent: usize,
    pub failed: usize,
    pub abandoned: usize,
    pub dropped: usize,
    pub discarded: usize,
    /// The breaker is open — the drain is short-circuiting and nothing is
    /// being attempted until this reaches zero.
    pub paused_for_ms: i64,
}

/// What the outbox is doing, for verification and for anything that wants to
/// report on it (alerts.rs turns these numbers into the observability
/// surface). PROCESS-LOCAL, like the queue it describes, so it reports on the
/// instance that answered the request.
pub fn notification_mail_stats() -> NotificationMailStats {
    outbox().stats(wall_ms())
}

/// What one send-attempt reports back to the drain. `delivered` separates
/// "nothing went wrong" from "a provider took it" — only the second may file
/// the in-app row read, and the address-less recipient is exactly the case
/// where the two answers differ.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendOneResult {
    pub ok: bool,
    pub blocked: bool,
    pub delivered: bool,
    pub error: Option<String>,
}

// The drain's four edges, injectable so the whole queue machinery runs in
// tests with no database, no transport and no clock. Production wiring is
// `real_drain_deps`.
pub type DeliveryFn = Arc<dyn Fn() -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;
pub type SendOneFn =
    Arc<dyn Fn(QueuedMailPayload) -> BoxFuture<'static, SendOneResult> + Send + Sync>;
pub type MarkReadFn = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The mail as the send edge receives it — the queued row minus its queue
/// bookkeeping (attempts, mark_read_id), owned end to end so it crosses the
/// boxed future without borrowing it.
pub struct QueuedMailPayload {
    pub user_id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub href: Option<String>,
}

impl From<&QueuedMail> for QueuedMailPayload {
    fn from(m: &QueuedMail) -> Self {
        QueuedMailPayload {
            user_id: m.user_id.clone(),
            kind: m.kind.clone(),
            title: m.title.clone(),
            body: m.body.clone(),
            href: m.href.clone(),
        }
    }
}

pub struct DrainDeps {
    pub delivery: DeliveryFn,
    pub send: SendOneFn,
    pub mark_read: MarkReadFn,
    pub now: NowFn,
}

/// Send what is queued, within a time budget — called by the notification-mail
/// job and safe to call directly. Concurrent callers serialize: the queue is a
/// single serial consumer by design, and two drains would be the
/// parallel-connection burst the queue exists to avoid.
pub async fn drain_notification_mail(deps: &DrainDeps, budget_ms: i64) -> MailDrainResult {
    outbox().drain(deps, budget_ms).await
}

// ── The email itself ─────────────────────────────────────────────────────────

/// This deployment's canonical base URL, or None when it has no VERIFIED
/// domain. Every absolute in-app link in a mail
/// is built on it — the digest's deep links and its button, the notification
/// mail's settings link — so "no verified domain" has to mean "no links",
/// never "links that do not resolve".
pub async fn instance_base_url(pg: &PgPool) -> Option<String> {
    let cfg = talaria_instance::get_instance_domain(pg).await;
    let verified = cfg.get("verified") == Some(&Value::Bool(true));
    let domain = cfg.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    if !verified || domain.is_empty() {
        return None;
    }
    Some(format!("https://{domain}"))
}

async fn app_url(pg: &PgPool, path: &str) -> Option<String> {
    let slash = if path.starts_with('/') { "" } else { "/" };
    instance_base_url(pg)
        .await
        .map(|base| format!("{base}{slash}{path}"))
}

pub const NOTIFY_SETTINGS_PATH: &str = "/settings/notifications";

/// The one sentence that goes in every notification mail's "why am I getting
/// this" slot and its text part's closer.
const WHAT_IS_THIS: &str = "Sent by Talaria because of your notification settings";

struct EmailParts {
    subject: String,
    html: String,
    text: String,
}

/// Compose one notification email from the house style (email_shell,
/// email_button, the escaping) in email.rs. Pure — the deep link and the
/// settings URL are resolved by the caller because they are instance facts,
/// not composition facts.
fn notification_email_parts(
    title: &str,
    body: Option<&str>,
    href: Option<&str>,
    deep_link: Option<&str>,
    settings_url: Option<&str>,
) -> EmailParts {
    use talaria_email::{email_button, email_escape, email_shell};
    let body = body.unwrap_or("").trim();
    let cta = match deep_link {
        Some(link) => email_button(link, "Open in Talaria"),
        // No verified instance domain, so no absolute link exists. Say where
        // it is rather than shipping a dead <a href="/boards/…"> that resolves
        // against the mail client.
        None if !href.unwrap_or("").is_empty() => format!(
            "<p style=\"margin:24px 0;font-size:13px;color:#8a8a84\">Open Talaria and go to <code>{}</code>.</p>",
            email_escape(href.unwrap_or(""))
        ),
        None => String::new(),
    };
    let footer = format!(
        "{}{}",
        email_escape(WHAT_IS_THIS),
        match settings_url {
            Some(u) => format!(
                " · <a href=\"{}\" style=\"color:#8a8a84\">change what Talaria emails you</a>",
                email_escape(u)
            ),
            None => " · change what Talaria emails you under Settings → Notifications".to_string(),
        }
    );
    let content_body = if body.is_empty() {
        String::new()
    } else {
        format!(
            "<p style=\"white-space:pre-wrap\">{}</p>",
            email_escape(body)
        )
    };
    let text = [
        Some(title.to_string()),
        (!body.is_empty()).then(|| body.to_string()),
        deep_link
            .map(str::to_string)
            .or_else(|| href.filter(|h| !h.is_empty()).map(str::to_string)),
        Some(format!(
            "{WHAT_IS_THIS}. Change what Talaria emails you: {}",
            settings_url.unwrap_or("Settings → Notifications")
        )),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    EmailParts {
        subject: title.to_string(),
        html: email_shell(
            &email_escape(title),
            &format!("{content_body}{cta}"),
            &footer,
        ),
        text,
    }
}

async fn send_notification_email(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    m: &QueuedMailPayload,
) -> SendOneResult {
    // The recipient lookup failing is a FAILED SEND, not "no address":
    // anything that breaks composition counts toward the breaker instead of
    // escaping into the drain.
    let to = match sqlx::query_scalar::<_, Option<String>>(
        "select email from users where id = $1::uuid",
    )
    .bind(&m.user_id)
    .fetch_optional(pg)
    .await
    {
        Err(e) => {
            tracing::error!(
                "[notifications] could not build the \"{}\" email for user {}: {e}",
                m.kind,
                m.user_id
            );
            return SendOneResult {
                ok: false,
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
        Ok(row) => row
            .flatten()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty()),
    };
    let Some(to) = to else {
        tracing::warn!(
            "[notifications] no email address for user {} — \"{}\" stays in-app only",
            m.user_id,
            m.kind
        );
        return SendOneResult {
            ok: true,
            ..Default::default()
        };
    };
    let href = m.href.as_deref().filter(|h| !h.is_empty());
    let deep_link = match href {
        Some(h) => app_url(pg, h).await,
        None => None,
    };
    if href.is_some() && deep_link.is_none() {
        tracing::warn!(
            "[notifications] no verified instance domain — sending mail without a clickable link (Admin → Org → Domain)"
        );
    }
    let settings_url = app_url(pg, NOTIFY_SETTINGS_PATH).await;
    let parts = notification_email_parts(
        &m.title,
        m.body.as_deref(),
        href,
        deep_link.as_deref(),
        settings_url.as_deref(),
    );
    let what = format!("notification \"{}\"", m.kind);
    let r = send_gated_mail(
        pg,
        sb,
        &GatedMail {
            to: &to,
            subject: &parts.subject,
            html: &parts.html,
            text: Some(&parts.text),
            unsubscribe_url: settings_url.as_deref(),
            what: &what,
        },
    )
    .await;
    // A refusal by the master switch is not a transport failure and must not
    // count toward the breaker: the mail server is fine, the operator said no.
    if !r.ok && !r.blocked {
        tracing::error!(
            "[notifications] email to {to} for \"{}\" failed: {}",
            m.kind,
            r.error.as_deref().unwrap_or("unknown error")
        );
    }
    SendOneResult {
        ok: r.ok,
        blocked: r.blocked,
        delivered: r.ok,
        error: r.error,
    }
}

async fn mark_read_on_delivery(pg: &PgPool, notification_id: &str) {
    if let Err(e) = sqlx::query(
        "update notifications set read_at = now() \
         where id = $1::uuid and read_at is null",
    )
    .bind(notification_id)
    .execute(pg)
    .await
    {
        // Harmless in the direction that matters: the row stays unread, so
        // the person sees it twice rather than not at all.
        tracing::error!(
            "[notifications] mailed {notification_id} but could not file the row read: {e}"
        );
    }
}

/// Production drain edges over the pool + secretbox. The boot path in jobs.rs
/// builds these (the secretbox loads through AppState) and registers the job.
pub fn real_drain_deps(pg: PgPool, sb: talaria_secretbox::SecretBox) -> Arc<DrainDeps> {
    let delivery: DeliveryFn = {
        let pg = pg.clone();
        Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { read_delivery_switch(&pg).await.map_err(|e| e.to_string()) })
        })
    };
    let send: SendOneFn = {
        let pg = pg.clone();
        let sb = sb.clone();
        Arc::new(move |m: QueuedMailPayload| {
            let pg = pg.clone();
            let sb = sb.clone();
            Box::pin(async move { send_notification_email(&pg, &sb, &m).await })
        })
    };
    let mark_read: MarkReadFn = {
        let pg = pg.clone();
        Arc::new(move |id: String| {
            let pg = pg.clone();
            Box::pin(async move { mark_read_on_delivery(&pg, &id).await })
        })
    };
    let now: NowFn = Arc::new(wall_ms);
    Arc::new(DrainDeps {
        delivery,
        send,
        mark_read,
        now,
    })
}

// ── The job ──────────────────────────────────────────────────────────────────

/// Fast enough that a mention email is not noticeably late, slow enough that
/// an empty queue costs nothing. Longer than DRAIN_BUDGET_MS so a full drain
/// does not routinely collide with the next tick.
pub const NOTIFY_MAIL_EVERY_MS: u64 = 30_000;
/// Let a deploy settle before this instance starts mailing anyone. Nothing
/// else may be derived from this number: it is a settle window, not a
/// coordination point.
pub const NOTIFY_MAIL_FIRST_RUN_DELAY_MS: u64 = 20_000;
/// The outside bound for ONE drain. The scheduler treats a run past its bound
/// as HUNG (observed, never aborted), which is the only way a drain that never
/// returns becomes visible to anybody. The real worst case is one full budget
/// plus the send already in flight when the budget expires; two minutes is
/// that, rounded up.
pub const NOTIFY_MAIL_MAX_RUN_MS: u64 = 2 * 60_000;

/// The job's message, pure so the sentence shaping is testable next to the
/// drain it reports on. "Nothing to do" means NOTHING TO DO — an empty queue
/// and a closed breaker. It used to also mean "the breaker is open and N
/// mails are going nowhere", because that pass sends nothing, fails nothing
/// and discards nothing, so a stuck outbox logged the same quiet line as a
/// healthy idle one. A pass that left mail on the queue has something to say,
/// and the two states it can be in — paused, or simply out of budget — are
/// not the same news.
fn job_message(r: &MailDrainResult, oldest_queued_ms: Option<i64>) -> Option<String> {
    if r.sent == 0 && r.failed == 0 && r.discarded == 0 && r.remaining == 0 {
        return None;
    }
    if r.sent == 0 && r.failed == 0 && r.discarded == 0 {
        let waited = oldest_queued_ms
            .map(|ms| {
                format!(
                    ", oldest waiting {} minute(s)",
                    (ms as f64 / 60_000.0).round()
                )
            })
            .unwrap_or_default();
        return Some(if r.paused {
            format!(
                "NOTHING SENT — delivery is paused after repeated failures; {} mail(s) still queued{waited}. Every one of them is still in their recipient's in-app inbox.",
                r.remaining
            )
        } else {
            format!(
                "nothing sent this pass — {} mail(s) still queued{waited}",
                r.remaining
            )
        });
    }
    let mut m = format!("sent {}, failed {}", r.sent, r.failed);
    if r.requeued > 0 {
        m.push_str(&format!(", requeued {} for the next pass", r.requeued));
    }
    if r.abandoned > 0 {
        m.push_str(&format!(
            ", GAVE UP on {} after {} attempts (still unread in-app)",
            r.abandoned, MAX_SEND_ATTEMPTS
        ));
    }
    if r.discarded > 0 {
        m.push_str(&format!(", discarded {} (delivery off)", r.discarded));
    }
    if r.remaining > 0 {
        m.push_str(&format!(", {} still queued", r.remaining));
    }
    if r.dropped > 0 {
        m.push_str(&format!(", {} dropped since boot", r.dropped));
    }
    if r.paused {
        m.push_str(" — delivery is paused after repeated failures");
    }
    Some(m)
}

/// The scheduler owns the timing — this module owns the work. `per_instance`
/// because the queue is in THIS process's memory: a Redis lease would let one
/// instance win the tick and leave every other instance's queue undrained
/// forever.
pub fn notification_mail_job_spec(deps: Arc<DrainDeps>) -> talaria_scheduler::JobSpec {
    use talaria_scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::NotificationMail,
        every_ms: NOTIFY_MAIL_EVERY_MS,
        first_run_delay_ms: Some(NOTIFY_MAIL_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(NOTIFY_MAIL_MAX_RUN_MS),
        per_instance: true,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let r = drain_notification_mail(&deps, DRAIN_BUDGET_MS).await;
                Ok(job_message(&r, notification_mail_stats().oldest_queued_ms))
            })
        }),
    }
}

pub fn register_notification_mail_job(deps: Arc<DrainDeps>) {
    talaria_scheduler::register_job(notification_mail_job_spec(deps));
}

// ── The missed-activity batcher ──────────────────────────────────────────────
//
// Every minute: the people with a row whose mail came due (`email_due_at <=
// now()`). For each, EVERY row of theirs still pending — unread, not
// archived, not yet mailed, with a due time, whether or not it is due yet —
// at most 50 per person and 1,000 per pass, so a burst is one mail, not one
// mail per row as each comes due a few seconds after the last. The rows are
// CLAIMED with `for update skip locked` by moving their due time ten minutes
// on (a lease: a process that dies mid-send leaves them to come due again,
// and two processes never take the same row). Then per person:
//   · re-read which of them are still unread (cancelled by read, right up to
//     the send) and whether the person still wants each one mailed (the
//     class's Email channel, the frequency, DND — a row held by a pause that
//     began after it was filed waits for the pause's end);
//   · ONE mail through `send_gated_mail`, so the kill switch, the transport
//     bounds and the fail-closed rule are the digest's and the outbox's;
//   · accepted → `emailed_at`, and the legacy email-only route's rows are
//     filed read (wave 1's "marked read after an accepted mail");
//   · refused by the switch → the rows are DISCARDED (no due time): a mail
//     the operator switched off is not held for later, exactly the outbox's
//     rule;
//   · failed → `email_attempts + 1` and the due time backs off 5, then 15
//     minutes; the third failure gives up, out loud, and the rows stay unread
//     in the inbox.
// Five failed people in a row is a transport, not an address: the pass stops
// and leaves the rest for five minutes.

/// How often the batcher looks for due rows.
pub const MISSED_MAIL_EVERY_MS: u64 = 60_000;
/// Rows one person's mail carries at most; the rest come due next pass.
pub const MISSED_MAIL_PER_USER: i64 = 50;
/// Rows one pass claims at most.
pub const MISSED_MAIL_PER_PASS: i64 = 1_000;
/// How long a claim holds its rows (the lease a dead process leaves behind).
pub const MISSED_MAIL_LEASE_SECS: i32 = 600;
/// The waits after the first and second failure, in minutes; the third gives
/// up (`MAX_SEND_ATTEMPTS`).
pub const MISSED_MAIL_BACKOFF_MIN: [i64; 2] = [5, 15];
/// People failed in a row before the pass stops, and how long the rest wait.
pub const MISSED_MAIL_BREAKER_AFTER: u32 = 5;
pub const MISSED_MAIL_BREAKER_SECS: i32 = 300;
/// How long one pass may spend before it puts the rest back (due now) for
/// the next tick. It bounds the pass, and so the job's lease — which is how
/// late mail can be after a process dies mid-pass.
pub const MISSED_MAIL_BUDGET_MS: i64 = 45_000;

/// TEST ONLY: `TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS=<n>` makes every
/// missed-activity delay and every backoff step n seconds and the batcher tick
/// every 5 s, so a live run can watch a batch, a cancel-by-read and three
/// failures in minutes. Read once; loud at registration.
pub fn test_email_delay_secs() -> Option<u64> {
    static V: LazyLock<Option<u64>> = LazyLock::new(|| {
        std::env::var("TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
    });
    *V
}

/// One mail as the batcher hands it to the send edge.
#[derive(Clone, Debug, PartialEq)]
pub struct MissedMail {
    pub user_id: String,
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub unsubscribe_url: Option<String>,
    /// The rows it carries, in mail order.
    pub ids: Vec<String>,
}

pub type MissedSendFn =
    Arc<dyn Fn(MissedMail) -> BoxFuture<'static, GatedSendResult> + Send + Sync>;

/// The batcher's edges: the pool, the publish half (a mail that files rows
/// read tells the person's tabs), and the send — `send_gated_mail` in
/// production, a fake in the live suite.
pub struct MissedMailDeps {
    pub pg: PgPool,
    pub realtime: RealtimeDeps,
    pub send: MissedSendFn,
}

/// Production edges. The secretbox is resolved PER SEND through `sb`, never
/// held: a key rotation installs a new box, and a clone taken at boot could
/// not open the transport password the rotation re-sealed.
pub fn real_missed_mail_deps(
    pg: PgPool,
    sb: talaria_push::SecretBoxSource,
    realtime: RealtimeDeps,
) -> Arc<MissedMailDeps> {
    let send: MissedSendFn = {
        let pg = pg.clone();
        Arc::new(move |m: MissedMail| {
            let pg = pg.clone();
            let sb = sb.clone();
            Box::pin(async move {
                let sb = match sb().await {
                    Ok(sb) => sb,
                    Err(e) => {
                        return GatedSendResult {
                            ok: false,
                            blocked: false,
                            error: Some(format!("the secretbox did not load: {e}")),
                        };
                    }
                };
                send_gated_mail(
                    &pg,
                    &sb,
                    &GatedMail {
                        to: &m.to,
                        subject: &m.subject,
                        html: &m.html,
                        text: Some(&m.text),
                        unsubscribe_url: m.unsubscribe_url.as_deref(),
                        what: "missed-activity mail",
                    },
                )
                .await
            })
        })
    };
    Arc::new(MissedMailDeps { pg, realtime, send })
}

/// One claimed row.
#[derive(Clone, Debug)]
struct Claimed {
    id: String,
    user_id: String,
    kind: String,
    title: String,
    body: String,
    href: String,
    attempts: i32,
}

/// What one pass did, for the job's log line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissedMailPass {
    /// Mails a provider accepted, and the rows they carried.
    pub mailed: usize,
    pub rows: usize,
    /// Mails that failed and will be retried.
    pub failed: usize,
    /// Rows given up on after the last attempt.
    pub gave_up: usize,
    /// Rows dropped because delivery is switched off.
    pub discarded: usize,
    /// Rows put back: read meanwhile, no longer wanted, held by DND, or left
    /// by an open breaker.
    pub deferred: usize,
}

impl MissedMailPass {
    fn message(&self) -> Option<String> {
        if *self == MissedMailPass::default() {
            return None;
        }
        let mut m = format!(
            "mailed {} person(s) ({} row(s)), failed {}",
            self.mailed, self.rows, self.failed
        );
        if self.gave_up > 0 {
            m.push_str(&format!(
                ", GAVE UP on {} row(s) after {MAX_SEND_ATTEMPTS} attempts (still unread in-app)",
                self.gave_up
            ));
        }
        if self.discarded > 0 {
            m.push_str(&format!(", discarded {} (delivery off)", self.discarded));
        }
        if self.deferred > 0 {
            m.push_str(&format!(", {} put back", self.deferred));
        }
        Some(m)
    }
}

async fn claim_due(pg: &PgPool) -> Result<Vec<Claimed>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, String, String, i32)> = sqlx::query_as(
        "with due as ( \
           select distinct user_id from notifications \
            where email_due_at <= now() and emailed_at is null \
              and read_at is null and archived_at is null \
            limit 200), \
         locked as ( \
           select n.id, n.user_id, n.email_due_at from notifications n \
             join due on due.user_id = n.user_id \
            where n.email_due_at is not null and n.emailed_at is null \
              and n.read_at is null and n.archived_at is null \
            order by n.email_due_at limit $1 \
            for update of n skip locked), \
         picked as ( \
           select id from (select id, row_number() over \
                             (partition by user_id order by email_due_at, id) as rn \
                           from locked) r \
            where rn <= $2) \
         update notifications n set email_due_at = now() + $3::int * interval '1 second' \
           from picked where n.id = picked.id \
         returning n.id::text, n.user_id::text, n.kind, n.title, n.body, n.href, \
                   n.email_attempts",
    )
    .bind(MISSED_MAIL_PER_PASS)
    .bind(MISSED_MAIL_PER_USER)
    .bind(MISSED_MAIL_LEASE_SECS)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, user_id, kind, title, body, href, attempts)| Claimed {
            id,
            user_id,
            kind,
            title,
            body,
            href,
            attempts,
        })
        .collect())
}

/// Put rows back to come due at `secs` from now (0 = now).
async fn requeue(pg: &PgPool, ids: &[String], secs: i64) {
    if ids.is_empty() {
        return;
    }
    if let Err(e) = sqlx::query(
        "update notifications set email_due_at = now() + $2::bigint * interval '1 second' \
         where id = any($1::uuid[]) and emailed_at is null",
    )
    .bind(ids)
    .bind(secs)
    .execute(pg)
    .await
    {
        tracing::error!(
            "[notifications] could not put {} mail row(s) back: {e}",
            ids.len()
        );
    }
}

/// Take rows out of the mail plane for good (no due time; nothing sent).
async fn drop_mail(pg: &PgPool, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    if let Err(e) = sqlx::query(
        "update notifications set email_due_at = null \
         where id = any($1::uuid[]) and emailed_at is null",
    )
    .bind(ids)
    .execute(pg)
    .await
    {
        tracing::error!(
            "[notifications] could not retire {} mail row(s): {e}",
            ids.len()
        );
    }
}

fn backoff_secs(attempts_after: i32) -> i64 {
    if let Some(s) = test_email_delay_secs() {
        return s as i64;
    }
    let i = (attempts_after.max(1) as usize - 1).min(MISSED_MAIL_BACKOFF_MIN.len() - 1);
    MISSED_MAIL_BACKOFF_MIN[i] * 60
}

/// A failed send: each row's attempts + 1; the last attempt gives up (no due
/// time), the others back off. Answers how many rows were given up on.
async fn record_failure(pg: &PgPool, rows: &[&Claimed]) -> usize {
    let mut gave_up = Vec::new();
    for r in rows {
        let next = r.attempts + 1;
        let res = if next >= MAX_SEND_ATTEMPTS as i32 {
            gave_up.push(r.id.clone());
            sqlx::query(
                "update notifications set email_attempts = $2, email_due_at = null \
                 where id = $1::uuid",
            )
            .bind(&r.id)
            .bind(next)
            .execute(pg)
            .await
        } else {
            sqlx::query(
                "update notifications set email_attempts = $2, \
                        email_due_at = now() + $3::bigint * interval '1 second' \
                 where id = $1::uuid",
            )
            .bind(&r.id)
            .bind(next)
            .bind(backoff_secs(next))
            .execute(pg)
            .await
        };
        if let Err(e) = res {
            tracing::error!(
                "[notifications] could not record a failed mail for {}: {e}",
                r.id
            );
        }
    }
    gave_up.len()
}

/// One pass of the batcher. The job's work, and safe to call directly (the
/// live suite does).
pub async fn run_missed_mail(deps: &MissedMailDeps) -> Result<MissedMailPass, String> {
    let pg = &deps.pg;
    let mut pass = MissedMailPass::default();
    // The kill switch first, FRESH: off means due rows are dropped, not
    // held — turning mail back on must never deliver a backlog in one burst.
    match read_delivery_switch(pg).await {
        Err(e) => {
            return Err(format!(
                "could not read the delivery switch — sending nothing this pass: {e}"
            ));
        }
        Ok(false) => {
            let dropped = sqlx::query(
                "update notifications set email_due_at = null \
                 where id in (select id from notifications \
                              where email_due_at <= now() and emailed_at is null \
                              limit 1000 for update skip locked)",
            )
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
            if dropped > 0 {
                tracing::warn!(
                    "[notifications] email delivery is off — discarded {dropped} due mail row(s). Every one of them is still in its recipient's in-app inbox."
                );
            }
            pass.discarded = dropped as usize;
            return Ok(pass);
        }
        Ok(true) => {}
    }
    let claimed = claim_due(pg).await.map_err(|e| e.to_string())?;
    if claimed.is_empty() {
        return Ok(pass);
    }
    // Per person, in the order their first row came due.
    let mut people: Vec<(String, Vec<Claimed>)> = Vec::new();
    for c in claimed {
        match people.iter_mut().find(|(u, _)| *u == c.user_id) {
            Some((_, rows)) => rows.push(c),
            None => people.push((c.user_id.clone(), vec![c])),
        }
    }
    let base = instance_base_url(pg).await;
    let settings_url = base.as_ref().map(|b| format!("{b}{NOTIFY_SETTINGS_PATH}"));
    let notify = NotifyDeps {
        pg: pg.clone(),
        realtime: deps.realtime.clone(),
    };
    let mut failed_in_a_row = 0u32;
    let deadline = wall_ms() + MISSED_MAIL_BUDGET_MS;
    let mut people = people.into_iter();
    while let Some((user_id, rows)) = people.next() {
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        if wall_ms() >= deadline {
            // Out of budget: this person and everyone behind them come due
            // again at once, for the next tick.
            let mut rest = ids;
            rest.extend(
                people
                    .by_ref()
                    .flat_map(|(_, rows)| rows.into_iter().map(|r| r.id)),
            );
            requeue(pg, &rest, 0).await;
            pass.deferred += rest.len();
            break;
        }
        // Cancelled by read, right up to the send.
        let unread: Vec<String> = match sqlx::query_scalar(
            "select id::text from notifications \
             where id = any($1::uuid[]) and read_at is null and archived_at is null",
        )
        .bind(&ids)
        .fetch_all(pg)
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[notifications] could not re-read {user_id}'s mail rows: {e}");
                requeue(pg, &ids, 60).await;
                pass.deferred += ids.len();
                continue;
            }
        };
        let (blob, zone) = prefs_and_zone(pg, &user_id).await;
        let now = wall_ms();
        let dnd = dnd_state(&blob, &zone, now);
        let mut send: Vec<&Claimed> = Vec::new();
        let mut unwanted: Vec<String> = Vec::new();
        let mut held: Vec<String> = Vec::new();
        for r in &rows {
            if !unread.contains(&r.id) {
                continue; // read meanwhile: never mailed, and no longer due
            }
            let class = notify_class_of(&r.kind);
            if !channels_of(&blob, class).email || email_frequency(&blob) == "off" {
                unwanted.push(r.id.clone());
                continue;
            }
            let breaks =
                class_def(class).is_some_and(|d| d.breaks_through) && allow_needs_you(&blob);
            if dnd.active && !breaks {
                held.push(r.id.clone());
                continue;
            }
            send.push(r);
        }
        drop_mail(pg, &unwanted).await;
        pass.deferred += unwanted.len();
        if !held.is_empty() {
            match dnd.until {
                Some(until) if until < crate::prefs::FOREVER_MS => {
                    requeue(pg, &held, ((until - now) / 1000).max(1)).await
                }
                _ => drop_mail(pg, &held).await,
            }
            pass.deferred += held.len();
        }
        if send.is_empty() {
            continue;
        }
        let to: Option<String> = match sqlx::query_scalar::<_, Option<String>>(
            "select email from users where id = $1::uuid",
        )
        .bind(&user_id)
        .fetch_optional(pg)
        .await
        {
            Ok(v) => v
                .flatten()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty()),
            Err(e) => {
                tracing::error!("[notifications] could not read {user_id}'s address: {e}");
                let ids: Vec<String> = send.iter().map(|r| r.id.clone()).collect();
                requeue(pg, &ids, 60).await;
                pass.deferred += ids.len();
                continue;
            }
        };
        let send_ids: Vec<String> = send.iter().map(|r| r.id.clone()).collect();
        let Some(to) = to else {
            tracing::warn!(
                "[notifications] no email address for user {user_id} — {} row(s) stay in-app only",
                send.len()
            );
            drop_mail(pg, &send_ids).await;
            pass.deferred += send_ids.len();
            continue;
        };
        let items: Vec<MailItem> = send
            .iter()
            .map(|r| MailItem {
                category: crate::classes::category_of(&r.kind),
                title: r.title.clone(),
                reason: kind_reason(&r.kind)
                    .or_else(|| class_def(notify_class_of(&r.kind)).map(|c| c.label))
                    .unwrap_or("Notification")
                    .to_string(),
                body: r.body.clone(),
                href: r.href.clone(),
            })
            .collect();
        let parts = missed_mail_parts(&items, base.as_deref(), settings_url.as_deref());
        let result = (deps.send)(MissedMail {
            user_id: user_id.clone(),
            to: to.clone(),
            subject: parts.subject,
            html: parts.html,
            text: parts.text,
            unsubscribe_url: settings_url.clone(),
            ids: send_ids.clone(),
        })
        .await;
        if result.ok {
            failed_in_a_row = 0;
            pass.mailed += 1;
            pass.rows += send_ids.len();
            if let Err(e) = sqlx::query(
                "update notifications set emailed_at = now() where id = any($1::uuid[])",
            )
            .bind(&send_ids)
            .execute(pg)
            .await
            {
                // The mail left; the row may be mailed again next pass. Said
                // out loud, because a duplicate is the lesser harm here.
                tracing::error!("[notifications] mailed {user_id} but could not record it: {e}");
            }
            // The legacy email-only route: the mail IS the delivery, so the
            // in-app copy is filed read — only now, after a provider took it.
            let email_only: Vec<String> = send
                .iter()
                .filter(|r| {
                    let c = channels_of(&blob, notify_class_of(&r.kind));
                    c.email && !c.desktop && !c.push
                })
                .map(|r| r.id.clone())
                .collect();
            if !email_only.is_empty() {
                match sqlx::query_scalar::<_, String>(
                    "update notifications set read_at = now() \
                     where id = any($1::uuid[]) and read_at is null returning id::text",
                )
                .bind(&email_only)
                .fetch_all(pg)
                .await
                {
                    Ok(read) if !read.is_empty() => {
                        crate::inbox::publish_read(&notify, &user_id, &read, false).await
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(
                        "[notifications] mailed {user_id} but could not file the email-only rows read: {e}"
                    ),
                }
            }
        } else if result.blocked {
            // The switch went off mid-pass: drop this person's rows and every
            // claimed row behind them. Nothing is held for later.
            drop_mail(pg, &send_ids).await;
            pass.discarded += send_ids.len();
            let rest: Vec<String> = people
                .by_ref()
                .flat_map(|(_, rows)| rows.into_iter().map(|r| r.id))
                .collect();
            drop_mail(pg, &rest).await;
            pass.discarded += rest.len();
            tracing::warn!(
                "[notifications] email delivery was switched off mid-pass — discarded {} mail row(s). Every one of them is still in its recipient's in-app inbox.",
                pass.discarded
            );
            break;
        } else {
            pass.failed += 1;
            failed_in_a_row += 1;
            let gave_up = record_failure(pg, &send).await;
            pass.gave_up += gave_up;
            tracing::error!(
                "[notifications] the missed-activity mail to {to} failed: {}{}",
                result.error.as_deref().unwrap_or("unknown error"),
                if gave_up > 0 {
                    format!(" — GAVE UP on {gave_up} row(s); they stay unread in-app")
                } else {
                    String::new()
                }
            );
            if failed_in_a_row >= MISSED_MAIL_BREAKER_AFTER {
                let rest: Vec<String> = people
                    .by_ref()
                    .flat_map(|(_, rows)| rows.into_iter().map(|r| r.id))
                    .collect();
                requeue(pg, &rest, i64::from(MISSED_MAIL_BREAKER_SECS)).await;
                pass.deferred += rest.len();
                tracing::error!(
                    "[notifications] {failed_in_a_row} missed-activity mails failed in a row — pausing the rest of this pass for 5 minutes"
                );
                break;
            }
        }
    }
    Ok(pass)
}

/// One row as the mail lists it.
#[derive(Clone, Debug)]
struct MailItem {
    category: Category,
    title: String,
    reason: String,
    body: String,
    href: String,
}

const MAIL_SECTIONS: &[(Category, &str)] = &[
    (Category::NeedsYou, "Needs you"),
    (Category::Direct, "Direct"),
    (Category::Following, "Following"),
    (Category::Workspace, "Workspace"),
];

/// The first line of a body, at most 160 characters.
fn snippet(body: &str) -> String {
    let line = body.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() <= 160 {
        return line.to_string();
    }
    let mut s: String = line.chars().take(159).collect();
    s.push('…');
    s
}

/// Compose the batch mail. Pure: the base URL and settings link are
/// instance facts the caller resolves. Items are grouped by category in the
/// center's order; each carries its title, why it showed, the first line of
/// its body, and its link (or, with no verified domain, where to find it).
fn missed_mail_parts(
    items: &[MailItem],
    base: Option<&str>,
    settings_url: Option<&str>,
) -> EmailParts {
    use talaria_email::{email_button, email_escape, email_shell};
    let n = items.len();
    let subject = format!("{n} unread in Talaria");
    let link = |href: &str| -> Option<String> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        base.map(|b| {
            let slash = if href.starts_with('/') { "" } else { "/" };
            format!("{b}{slash}{href}")
        })
    };
    let mut html = String::new();
    let mut text: Vec<String> = vec![subject.clone(), String::new()];
    for (cat, label) in MAIL_SECTIONS {
        let group: Vec<&MailItem> = items.iter().filter(|i| i.category == *cat).collect();
        if group.is_empty() {
            continue;
        }
        html.push_str(&format!(
            "<p style=\"margin:20px 0 6px;font-size:12px;font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:#8a8a84\">{}</p>",
            email_escape(label)
        ));
        text.push(label.to_uppercase());
        for i in group {
            let title = match link(&i.href) {
                Some(url) => format!(
                    "<a href=\"{}\" style=\"color:inherit;font-weight:600\">{}</a>",
                    email_escape(&url),
                    email_escape(&i.title)
                ),
                None => format!("<strong>{}</strong>", email_escape(&i.title)),
            };
            let snip = snippet(&i.body);
            let where_ = match (link(&i.href), i.href.trim().is_empty()) {
                (None, false) => format!(
                    "<br><span style=\"font-size:12px;color:#8a8a84\">Open Talaria and go to <code>{}</code></span>",
                    email_escape(i.href.trim())
                ),
                _ => String::new(),
            };
            html.push_str(&format!(
                "<p style=\"margin:0 0 12px\">{title} <span style=\"font-size:12px;color:#8a8a84\">· {}</span>{}{where_}</p>",
                email_escape(&i.reason),
                if snip.is_empty() {
                    String::new()
                } else {
                    format!("<br><span style=\"color:#55554f\">{}</span>", email_escape(&snip))
                }
            ));
            text.push(format!("- {} ({})", i.title, i.reason));
            if !snip.is_empty() {
                text.push(format!("  {snip}"));
            }
            if let Some(url) = link(&i.href)
                .or_else(|| (!i.href.trim().is_empty()).then(|| i.href.trim().to_string()))
            {
                text.push(format!("  {url}"));
            }
        }
        text.push(String::new());
    }
    if let Some(b) = base {
        html.push_str(&email_button(&format!("{b}/"), "Open Talaria"));
    }
    let footer = format!(
        "{} · {}",
        email_escape(WHAT_IS_THIS),
        match settings_url {
            Some(u) => format!(
                "<a href=\"{}\" style=\"color:#8a8a84\">change what Talaria emails you</a>",
                email_escape(u)
            ),
            None => "change what Talaria emails you under Settings → Notifications".to_string(),
        }
    );
    text.push(format!(
        "{WHAT_IS_THIS}. Change what Talaria emails you: {}",
        settings_url.unwrap_or("Settings → Notifications")
    ));
    EmailParts {
        html: email_shell(
            &email_escape(&subject),
            &format!(
                "<p style=\"margin:0 0 4px\">While you were away, {} came in that you have not seen yet.</p>{html}",
                if n == 1 {
                    "one notification".to_string()
                } else {
                    format!("{n} notifications")
                }
            ),
            &footer,
        ),
        subject,
        text: text.join("\n"),
    }
}

/// The batcher as a scheduler job: leased (it reads rows every instance can
/// reach), every minute — every 5 s under the test-only delay.
pub fn missed_mail_job_spec(deps: Arc<MissedMailDeps>) -> talaria_scheduler::JobSpec {
    use talaria_scheduler::{JobName, JobSpec};
    let fast = test_email_delay_secs().is_some();
    JobSpec {
        name: JobName::NotifyMissedMail,
        every_ms: if fast { 5_000 } else { MISSED_MAIL_EVERY_MS },
        first_run_delay_ms: Some(if fast { 5_000 } else { 30_000 }),
        // One pass stops taking people at its 45 s budget, and the send in
        // hand is bounded at 30 s by the transport. This is also how long a
        // process killed mid-pass holds the lease; its claimed rows wait out
        // their own 10-minute lease (`MISSED_MAIL_LEASE_SECS`).
        max_run_ms: Some(2 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move { run_missed_mail(&deps).await.map(|p| p.message()) })
        }),
    }
}

pub fn register_missed_mail_job(deps: Arc<MissedMailDeps>) {
    if let Some(s) = test_email_delay_secs() {
        tracing::warn!(
            "[notifications] TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS={s}: every missed-activity mail delay and backoff is {s}s and the batcher ticks every 5s. TEST ONLY."
        );
    }
    talaria_scheduler::register_job(missed_mail_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the mail plane ───────────────────────────────────────────────────────

    use std::sync::Mutex as StdMutex;

    /// Everything a drain test wants to steer or observe, behind Arc<Mutex>
    /// so the boxed futures in DrainDeps can reach it.
    struct FakeEdge {
        delivery: StdMutex<Result<bool, String>>,
        outcomes: StdMutex<Vec<SendOneResult>>,
        marked_read: StdMutex<Vec<String>>,
        sent_kinds: StdMutex<Vec<String>>,
        clock: StdMutex<i64>,
    }

    impl Default for FakeEdge {
        fn default() -> Self {
            FakeEdge {
                delivery: StdMutex::new(Ok(true)),
                outcomes: StdMutex::new(Vec::new()),
                marked_read: StdMutex::new(Vec::new()),
                sent_kinds: StdMutex::new(Vec::new()),
                clock: StdMutex::new(0),
            }
        }
    }

    impl FakeEdge {
        fn deps(self: &Arc<Self>) -> DrainDeps {
            let delivery: DeliveryFn = {
                let f = Arc::clone(self);
                Arc::new(move || {
                    let f = Arc::clone(&f);
                    Box::pin(async move { f.delivery.lock().unwrap().clone() })
                })
            };
            let send: SendOneFn = {
                let f = Arc::clone(self);
                Arc::new(move |m: QueuedMailPayload| {
                    let f = Arc::clone(&f);
                    Box::pin(async move {
                        f.sent_kinds.lock().unwrap().push(m.kind.clone());
                        let next = {
                            let mut outcomes = f.outcomes.lock().unwrap();
                            (!outcomes.is_empty()).then(|| outcomes.remove(0))
                        };
                        next.unwrap_or(SendOneResult {
                            ok: true,
                            delivered: true,
                            ..Default::default()
                        })
                    })
                })
            };
            // outcomes pops from the FRONT like the queue does.
            let mark_read: MarkReadFn = {
                let f = Arc::clone(self);
                Arc::new(move |id: String| {
                    let f = Arc::clone(&f);
                    Box::pin(async move {
                        f.marked_read.lock().unwrap().push(id);
                    })
                })
            };
            let now: NowFn = {
                let f = Arc::clone(self);
                Arc::new(move || *f.clock.lock().unwrap())
            };
            DrainDeps {
                delivery,
                send,
                mark_read,
                now,
            }
        }
    }

    fn fresh_outbox(n: usize) -> Outbox {
        let o = Outbox {
            state: Mutex::new(outbox_state()),
            serial: tokio::sync::Mutex::new(()),
        };
        for i in 0..n {
            o.enqueue(
                &format!("user-{i}"),
                &NotificationInput {
                    kind: "mention",
                    title: &format!("title {i}"),
                    body: Some("body"),
                    href: Some("/boards/x"),
                },
                (i % 2 == 0).then(|| format!("notif-{i}")),
            );
        }
        o
    }

    fn ok_delivered() -> SendOneResult {
        SendOneResult {
            ok: true,
            delivered: true,
            ..Default::default()
        }
    }

    fn fail(msg: &str) -> SendOneResult {
        SendOneResult {
            ok: false,
            error: Some(msg.into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_off_switch_discards_the_whole_queue() {
        let f = Arc::new(FakeEdge {
            delivery: StdMutex::new(Ok(false)),
            ..Default::default()
        });
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 0);
        assert_eq!(r.discarded, 3);
        assert_eq!(r.remaining, 0);
        assert_eq!(o.stats(0).queued, 0);
        // Held mail is a trap; the whole point is that it is GONE.
        assert_eq!(o.stats(0).discarded, 3);
        assert!(f.sent_kinds.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_switch_fails_closed_and_keeps_the_queue() {
        let f = Arc::new(FakeEdge {
            delivery: StdMutex::new(Err("db down".into())),
            ..Default::default()
        });
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 0);
        assert_eq!(r.discarded, 0);
        // Nothing sent AND nothing thrown away — the backlog survives the blip.
        assert_eq!(r.remaining, 3);
        assert_eq!(o.stats(0).queued, 3);
    }

    #[tokio::test]
    async fn a_happy_drain_marks_read_only_where_the_route_says_so() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 3);
        assert_eq!(r.remaining, 0);
        // even indexes queued with a mark_read_id (route 'email'), odd without
        // (route 'both') — only the first kind may be filed read.
        assert_eq!(
            *f.marked_read.lock().unwrap(),
            vec!["notif-0".to_string(), "notif-2".to_string()]
        );
    }

    #[tokio::test]
    async fn an_ok_without_delivery_is_sent_but_never_marks_read() {
        // The address-less recipient: ok, nothing left the building.
        let f = Arc::new(FakeEdge {
            outcomes: StdMutex::new(vec![SendOneResult {
                ok: true,
                delivered: false,
                ..Default::default()
            }]),
            ..Default::default()
        });
        let o = fresh_outbox(1);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.failed, 0);
        assert!(f.marked_read.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failing_mail_retries_then_is_abandoned_out_loud() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(1);
        for pass in 1..=MAX_SEND_ATTEMPTS {
            *f.outcomes.lock().unwrap() = vec![fail("smtp timed out")];
            let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
            assert_eq!(r.failed, 1, "pass {pass}");
            if pass < MAX_SEND_ATTEMPTS {
                assert_eq!(r.requeued, 1, "pass {pass}");
                assert_eq!(r.remaining, 1, "pass {pass}");
                assert_eq!(r.abandoned, 0);
            } else {
                assert_eq!(r.abandoned, 1);
                assert_eq!(r.remaining, 0);
            }
        }
        // Three failures for ONE mail never trip the breaker (5 consecutive).
        assert_eq!(o.stats(0).paused_for_ms, 0);
    }

    #[tokio::test]
    async fn five_consecutive_failures_trip_the_breaker_mid_drain() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(7);
        *f.outcomes.lock().unwrap() = vec![fail("x"); 7];
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        // The 5th consecutive failure pauses delivery and stops the pass.
        assert_eq!(r.failed, BREAKER_AFTER as usize);
        assert_eq!(r.requeued, 5);
        assert!(r.paused);
        assert_eq!(r.remaining, 7); // 5 requeued at the back + 2 never offered
        assert!(o.stats(0).paused_for_ms > 0);
        // And the pause holds: a later pass attempts nothing even though the
        // switch is on.
        f.outcomes.lock().unwrap().clear();
        *f.delivery.lock().unwrap() = Ok(true);
        let r2 = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r2.sent, 0);
        assert!(r2.paused);
        // Past the cooldown the same queue is attempted again.
        *f.clock.lock().unwrap() += BREAKER_COOLDOWN_MS + 1;
        let r3 = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r3.sent, 7);
    }

    #[tokio::test]
    async fn a_blocked_send_mid_drain_discards_the_rest() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(3);
        *f.outcomes.lock().unwrap() = vec![
            ok_delivered(),
            SendOneResult {
                ok: false,
                blocked: true,
                error: Some("email delivery is switched off for this instance".into()),
                delivered: false,
            },
        ];
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.failed, 0);
        // The peeked-but-refused item counts in the discard, and so does the
        // one behind it.
        assert_eq!(r.discarded, 2);
        assert_eq!(r.remaining, 0);
    }

    #[tokio::test]
    async fn the_budget_expires_before_the_queue_does() {
        let f = Arc::new(FakeEdge::default());
        // Each send eats the entire budget, so exactly one goes out per pass.
        let send: SendOneFn = {
            let f = Arc::clone(&f);
            Arc::new(move |m: QueuedMailPayload| {
                let f = Arc::clone(&f);
                Box::pin(async move {
                    f.sent_kinds.lock().unwrap().push(m.kind.clone());
                    *f.clock.lock().unwrap() += 10_000;
                    ok_delivered()
                })
            })
        };
        let o = fresh_outbox(2);
        let deps = DrainDeps {
            delivery: f.deps().delivery,
            send,
            mark_read: f.deps().mark_read,
            now: f.deps().now,
        };
        let r = o.drain(&deps, 10_000).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.remaining, 1);
    }

    #[test]
    fn a_full_queue_refuses_at_the_door_without_losing_accepted_work() {
        let o = Outbox {
            state: Mutex::new(outbox_state()),
            serial: tokio::sync::Mutex::new(()),
        };
        for i in 0..=MAX_QUEUED {
            o.enqueue(
                &format!("user-{i}"),
                &NotificationInput {
                    kind: "dm",
                    title: "t",
                    body: None,
                    href: None,
                },
                None,
            );
        }
        let s = o.stats(0);
        assert_eq!(s.queued, MAX_QUEUED);
        assert_eq!(s.dropped, 1); // the 501st — the newest — is refused
        assert_eq!(s.capacity, MAX_QUEUED);
        assert!(s.oldest_queued_ms.is_some()); // a stuck-outbox reader needs this
    }

    // ── the sentences ────────────────────────────────────────────────────────

    #[test]
    fn job_messages_are_the_pinned_sentences() {
        // Quiet pass: nothing to do at all.
        assert_eq!(
            job_message(
                &MailDrainResult {
                    ..Default::default()
                },
                None
            ),
            None
        );
        // Paused with mail on the queue: the loud one.
        let paused = MailDrainResult {
            remaining: 3,
            paused: true,
            ..Default::default()
        };
        assert_eq!(
            job_message(&paused, Some(90_000)).as_deref(),
            Some(
                "NOTHING SENT — delivery is paused after repeated failures; 3 mail(s) still queued, oldest waiting 2 minute(s). Every one of them is still in their recipient's in-app inbox."
            )
        );
        // Out of budget: the quiet one, no oldest to name.
        let budget = MailDrainResult {
            remaining: 2,
            ..Default::default()
        };
        assert_eq!(
            job_message(&budget, None).as_deref(),
            Some("nothing sent this pass — 2 mail(s) still queued")
        );
        // The full tally, every clause present and in order.
        let full = MailDrainResult {
            sent: 3,
            failed: 1,
            requeued: 1,
            abandoned: 1,
            discarded: 0,
            remaining: 1,
            dropped: 2,
            paused: false,
        };
        assert_eq!(
            job_message(&full, None).as_deref(),
            Some(
                "sent 3, failed 1, requeued 1 for the next pass, GAVE UP on 1 after 3 attempts (still unread in-app), 1 still queued, 2 dropped since boot"
            )
        );
        // …and the paused tail on an otherwise-productive pass.
        let tail = MailDrainResult {
            sent: 2,
            failed: 5,
            paused: true,
            ..Default::default()
        };
        assert_eq!(
            job_message(&tail, None).as_deref(),
            Some("sent 2, failed 5 — delivery is paused after repeated failures")
        );
    }

    #[test]
    fn the_composed_mail_is_pinned() {
        // Full case: verified domain → deep link, settings URL, real body.
        let p = notification_email_parts(
            "Q3 plan",
            Some("  line one\nline two  "),
            Some("/plans/q3"),
            Some("https://talaria.example/plans/q3"),
            Some("https://talaria.example/settings/notifications"),
        );
        assert_eq!(p.subject, "Q3 plan");
        assert!(p.html.contains(&format!(
            "<p style=\"white-space:pre-wrap\">line one\nline two</p>{}",
            talaria_email::email_button("https://talaria.example/plans/q3", "Open in Talaria")
        )));
        assert!(p.html.contains(
            "Sent by Talaria because of your notification settings · <a href=\"https://talaria.example/settings/notifications\" style=\"color:#8a8a84\">change what Talaria emails you</a>"
        ));
        assert_eq!(
            p.text,
            "Q3 plan\n\nline one\nline two\n\nhttps://talaria.example/plans/q3\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: https://talaria.example/settings/notifications"
        );

        // No verified domain: no dead <a href="/…">, say where it is instead,
        // and the footer falls back to the named path.
        let p = notification_email_parts("Ping", Some("hello"), Some("/inbox"), None, None);
        assert!(p.html.contains(
            "<p style=\"margin:24px 0;font-size:13px;color:#8a8a84\">Open Talaria and go to <code>/inbox</code>.</p>"
        ));
        assert!(
            p.html
                .contains(" · change what Talaria emails you under Settings → Notifications")
        );
        assert_eq!(
            p.text,
            "Ping\n\nhello\n\n/inbox\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: Settings → Notifications"
        );

        // No href, no body: the text part is title + closer, nothing else.
        let p = notification_email_parts("Bare", None, None, None, None);
        assert_eq!(
            p.text,
            "Bare\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: Settings → Notifications"
        );
    }
}
