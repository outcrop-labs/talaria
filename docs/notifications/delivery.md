# Delivery engine

What is delivered where and when: the class × channel matrix, DND enforcement, presence
gating, the deferred push and its ack, the missed-activity email, retention, and the push
edge. The interfaces it builds on are in [the contract](./contract.md); the user-facing
story is [`../NOTIFICATIONS.md`](../NOTIFICATIONS.md).

The row is the record. Every write lands in-app, counted, whatever the person's settings
say; everything below is a delivery on top of it. The code is
`api/crates/talaria-notify/src/` (`deliver.rs`, `write.rs`, `defer.rs`, `mail.rs`,
`presence.rs`, `retention.rs`, `classes.rs`, `prefs.rs`) and `api/crates/talaria-push/`.

## The decision at write time

`deliver::decide` is pure. It runs inside the write's transaction, on the prefs and time
zone that the seq bump returned, and its answer lands on the row:

| Input | Where it comes from |
|---|---|
| The class's Desktop, Push and Email channels | `prefs::channels_of` (prefs v2, I-13) |
| DND and working hours | `prefs::dnd_state` in `users.timezone` (I-14) |
| What the producer said | `NotifyMeta` (`quiet`, `skip_push`, `force_alert`) |
| The instance switches | `app_settings.notify_delivery`: `emailEnabled` (off unless exactly `true`), `pushEnabled` (on unless exactly `false`), read through a 10 s cache |
| Whether a client is in use | the presence winner (I-17), read before the transaction |

The rules:

```
breaks = meta.force_alert || (class breaks through && dnd.allowNeedsYou)
held   = dnd.active && !breaks
alert  = banner  iff Desktop && !quiet && !held          else feed   (I-15)
push   = Push && pushEnabled && plane && !quiet && !skip_push && !held
           a client looking or active → push_due_at = now + 60 s
           nobody there               → push now, pushed_at = now
email  = Email && emailEnabled && !quiet && frequency != off
           email_due_at = now + delay, and never before a pause ends
           delay = 15 min for Needs you, else 15 min / 1 h / the next 09:00 local
```

- **A held row** (DND, or outside working hours) is a feed row: no banner, no push, and its
  mail is due when the pause ends. A pause with no end ("until I resume") mails nothing.
- **Needs you breaks through** (`approval_pending`, `judge_escalation`, `agent_blocked`,
  `workchain_turn`) unless the person turned `allowNeedsYou` off. A producer's
  `force_alert` always does.
- **Nothing slow runs under the person's row lock.** The presence read (one bounded
  `HGETALL`, 250 ms at most) and the switch read happen before `begin`. A caller must never
  file a notification while holding that person's `users` row itself: the write locks it
  until commit (contract note N7).
- **A coalesce** (`Realert`) is a new version of the row. It takes this write's push
  decision (a pending deferral is replaced, or cancelled when this write does not push),
  forgets the old version's ack, and arms a mail only when the row has none pending and
  none sent. A `Quiet` coalesce changes none of the delivery columns. `refresh_notification`
  behaves like a `Realert` coalesce and never mails again.

### Defaults

The registry (`classes.rs`) defaults are wave 1's effective answer, with two changes:

- **Following is quiet** (EB-05, O-9). `work_complete`, `workchain_paused` and
  `channel_activity` default to the feed only: no banner, no push, no mail. A person who
  stored a legacy route (`in_app`, `both`) or a v2 channel keeps what they chose, because a
  stored value always wins over the default.
- **The `account` class** (Direct; banner, push and mail on) carries
  `agent_google_principal`, the notice that an agent now acts as you on Google. It used to
  fall through into the quiet `work_complete` bucket (SE-14).

The digest's derived default reads only the nine original classes (C27), so neither change
moves anyone's digest.

## Presence and the deferred push

- The write path reaches the presence store through a Redis handle installed at boot
  (`presence::install_presence_redis`, beside the push plane in `talaria-jobs`), because the
  contract's `NotifyDeps` carries no Redis connection. **No handle, a Redis error or a
  timeout means there is no winner, and the push goes now.** Delivery fails open.
- The thresholds: looking means input in the last 60 s, active means input in the last
  120 s, an entry is stale at 75 s, and the deferral is 60 s (`presence::PUSH_DEFER_MS`).
- The winner cache (`presence:win:<user>`) is a compare-and-set in one Lua script. The value
  is `<at>|<pair>`, and a beat whose entries were read earlier than the cached answer never
  overwrites it (contract note N2). A beat publishes the `presence` frame only when it moved
  the pair.
- **The ack (I-18).** The client that toasted a row to someone looking, or posted an OS
  banner for it, calls `POST /api/notifications/ack`, which sets `surfaced_at`.
- **The deferred-push job** (`notify-deferred-push`, leased, every 5 s) claims rows whose
  deferral is due and that nobody has acked or read, with `for update skip locked`. It stamps
  `pushed_at` in the same statement and then pushes. Before pushing it reads the person's
  prefs again, so a pause or a Push switch turned off within the minute is obeyed. Claiming
  before sending means two processes never push one row, and no row lock is held while a
  push service answers. A process killed between the claim and the POST loses that one
  push; it never sends twice. A deferral that came due while the api was down fires once,
  5 s after boot.

## Missed-activity mail

The batcher (`notify-missed-mail`, leased, every 60 s) replaces wave 1's in-memory outbox.
The write path no longer enqueues anything. The old drain stays registered for one release
so that a queue accepted by the previous binary still drains (PLAN3 §13 retires it).

1. **The kill switch first, read fresh.** When mail is off, due rows are dropped
   (`email_due_at` null) rather than held, so turning mail back on never delivers a backlog
   in one burst.
2. **Claim, per person.** It finds the people with a due row. For each, it takes every row
   of theirs still pending (unread, unarchived, unmailed, with a due time), whether or not
   that row is due yet. So a burst is one mail, not one mail per row as each comes due a few
   seconds after the last. The limits are 50 rows per person and 1,000 per pass, taken with
   `for update skip locked`. Each claimed row's due time moves 10 minutes on. That move is a
   lease: if the process dies mid-send, the rows come due again.
3. **Re-check per person, right up to the send.** Rows read meanwhile are skipped and never
   mailed. A row whose class's Email channel is now off, or whose frequency is now `off`,
   leaves the mail plane. A row held by a pause that began after it was filed waits for the
   pause to end.
4. **One mail.** The subject is "N unread in Talaria". The items are grouped by category
   (Needs you, Direct, Following, Workspace), each with its title, why it showed, the first
   line of its body and its link. It carries `List-Unsubscribe` to the settings page, and
   it goes through `send_gated_mail`, so the kill switch, the transport bounds and the
   fail-closed rule are the digest's. With no verified instance domain, the mail has no
   links and no unsubscribe header; it names the path instead.
5. **The outcomes:**
   - **Accepted:** `emailed_at` is set. Rows of the legacy email-only route (Desktop and
     Push off, Email on) are filed read, which keeps wave 1's "read after an accepted mail",
     and the read-sync event goes out.
   - **No address:** the rows stay in-app only.
   - **Refused by the switch mid-pass:** this person's rows and every claimed row behind
     them are dropped.
   - **Failed:** `email_attempts + 1`. The due time backs off 5 minutes, then 15, and the
     third failure gives up and is logged; the rows stay unread in the inbox. After five
     people fail in a row, the pass stops and the rest wait 5 minutes.
   - **Out of budget:** a pass stops taking people after 45 s, and the rest come due again
     at once.

## The push edge (`talaria-push`)

- **Headers by category** (WP-12, BT-10):

  | Category | Urgency | TTL |
  |---|---|---|
  | Needs you | `high` | 24 h |
  | Direct | `normal` | 12 h |
  | Following, Workspace | `low` | 1 h |

  `Topic` is the row id without dashes, so a re-alert replaces a push the service has not
  delivered yet.
- **The VAPID keypair** is unsealed once and cached. The cache is keyed by the sealed
  `private` string, and the cheap `SELECT` stays on every delivery, so a re-mint, a restore
  or a rotation's re-seal is seen at once (SE-15). `vapid_unseals()` counts the unseals;
  each one logs at debug.
- **The secretbox is read per delivery** from the process's current box
  (`install_push_plane_with`). A key rotation installs a new box, and a clone taken at boot
  could not open the key that the rotation re-sealed. The mail batcher resolves its box the
  same way.
- **Hung endpoints.**
  - A timeout is retried once, so a hung endpoint holds a process-wide permit for at most
    2 × 15 s. 429, 5xx and connect failures are still retried twice.
  - A row that timed out, or has failed three times running, is skipped for 2^failures
    minutes (60 at most) after its last attempt.
  - Ten failures with no delivery for seven days prune the row.
  - A 2xx resets the count.
  - The person's other devices never wait on a hung one: they are posted four at a time.
- **The per-device test** (`send_test`, `POST /api/notifications/test`) is one direct
  delivery to that subscription. It ignores presence, DND and the backoff window, and it is
  not retried. The payload is `{id: "talaria-test-<ms>", title: "Talaria test
  notification", body: "<instance>: this device gets alerts", href:
  "/settings/notifications", kind: "test"}` with Urgency `high` and a TTL of 5 minutes. The
  row is tended like any delivery, so the device list shows the result. It still obeys the
  instance push switch. The route allows 5 tests per person per 10 minutes (O-20).

## Retention and the push prune

Two daily jobs, both leased:

- **`notify-retention`** deletes rows read more than 90 days ago and rows unread for more
  than 180 days (O-15), in batches of 10,000, together with the `inbox_focus_state` rows
  that pointed at them (keyed by the row id as text, with no foreign key).
- **`push-prune`** (C26):
  - **The marker.** `talaria-session` writes a liveness marker,
    `sesshash:<hex sha256 of the session id>`, beside every session with the same TTL. It
    is written in one MULTI at login, refreshed when the session changes, and deleted at
    logout.
  - **Dead sessions.** A subscription whose `session_hash` has no marker belonged to a
    session that ended (logged out, or expired after its 7 days), and is deleted.
  - **Backfill first.** Before reading markers, the pass writes one for every live session
    that lacks it (a session minted before the marker existed), so a deploy never mistakes a
    live session for a dead one.
  - **Database-only rules.** Rows with no `session_hash` (from before wave 1) go after 30
    days with no delivery. Rows that have failed ten times with no delivery for a week also
    go.
  - **No Redis handle, no session pass.** It never guesses.

## Jobs and test-only settings

| Job | Every | Leased | Work |
|---|---|---|---|
| `notify-deferred-push` | 5 s | yes (20 s) | push the due, un-acked, unread deferrals |
| `notify-missed-mail` | 60 s | yes (2 min) | the missed-activity batcher |
| `notify-retention` | 24 h | yes | aged rows and their inbox-focus rows |
| `push-prune` | 24 h | yes | subscriptions of ended sessions, idle and failing ones |
| `notification-mail` | 30 s | per instance | wave 1's outbox drain (one release, then retired) |

The number in brackets is the job's `max_run_ms`. It is also how long a process killed
mid-pass holds the job's lease, so it bounds how late the first deferred push or mail
after a crash can be. It is kept short on purpose: a pass that outlives it is reported as
hung, and a second instance starting meanwhile is harmless, because every claim skips
locked rows.

Two environment variables exist for live tests only. Each logs a warning at boot:

- `TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS=<n>`: every mail delay and backoff step becomes n
  seconds, and the batcher ticks every 5 s.
- `TALARIA_TEST_NOTIFY_DAILY_EVERY_MS=<n>`: both daily jobs run every n ms.

## Features

`deliver::FEATURES` = `email-batch`, `quiet-following`, `dnd-enforced`;
`presence::FEATURES` = `presence-gated-push`. A client turns the matching UI on by feature,
never by version.
