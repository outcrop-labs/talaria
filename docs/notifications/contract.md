# The notification contract (wave 2)

The interfaces every wave-2 notification stream builds against. It is landed by one pull
request (the "contract"): every schema change, the single write path, the shared Rust
types and wire shapes, the route surfaces, the TypeScript types, and the test and doc
scaffolding. With the contract alone, every wave-1 behaviour is identical: new columns are
written, new wire fields are served, and nothing consumes them yet. The exceptions are the
correctness fixes the contract makes itself: the instance email switch merges instead of
overwriting its row (C28); reading an agent thread publishes the read-sync event; the
digest's derived default is frozen to the original nine classes (C27); a re-alert's
`createdAt` always moves later (a writer queued behind another no longer stamps an earlier
time); a matrix Push switch turned off is obeyed; the Home Inbox's undo of a read restores
the row (its guard compared a microsecond `read_at` with a millisecond timestamp and never
matched); and a desktop client heartbeating from the tray while its OS is idle does not
mark the person online.

This file is frozen for wave 2. A stream that finds the contract wrong says so in its own
pull request, and the fix lands after. The user-facing story and troubleshooting stay in
[`../NOTIFICATIONS.md`](../NOTIFICATIONS.md); each stream's own doc is linked at the end.

## The interfaces

Wave 1's I-1 … I-9 ([`../NOTIFICATIONS.md`](../NOTIFICATIONS.md#contracts-with-the-desktop-shell))
stand, with I-8 amended below.

| Id | Interface | Landed by | Consumed by |
|---|---|---|---|
| I-10 | Per-user commit-ordered `seq`: every alerting write takes the next one in its own transaction; read/archive never do; a snapshot seeing `notify_seq = N` sees every row `seq ≤ N`. GET top-level `seq`; `notification.seq` | phase 0 | W2-H, W2-R, W2-D |
| I-11 | Row wire v2 (`seq, alert, class, category, count, updatedAt, archivedAt, actor, entity, reason`) | phase 0 | W2-R, W2-C, W2-D |
| I-12 | `classes[]` registry; categories; For you = needs_you + direct | phase 0 (W2-E evolves) | W2-C, W2-H, W2-R |
| I-13 | Prefs v2 `channels`, legacy both ways, digest frozen | phase 0 | W2-C, W2-E |
| I-14 | DND + working hours + `emailFrequency`; `dnd_state` is the one evaluator | phase 0 (W2-E enforces) | W2-C, W2-E, W2-R, W2-D |
| I-15 | `alert: 'banner'\|'feed'` (null = banner). No client toasts or banners a feed row; while `dnd.active`, clients banner only rows of a breaks-through class | phase 0 (W2-E decides) | W2-R, W2-D |
| I-16 | Read-sync event v2 (`unreadForYou`, `readIds`, `all`, `resync`); **every client closes its OS banners for read ids** | phase 0 (W2-H extends to archive and inbox-focus) | W2-R, W2-D |
| I-17 | Presence: heartbeat body, `GET /api/me/presence`, `presence` event, the winner rule | phase 0 (W2-E gates on it) | W2-R, W2-D, W2-E, W2-C |
| I-18 | Ack (`POST /api/notifications/ack`): the client that toasted to a looking person, or posted an OS banner, acks; the deferred push fires only for un-acked, unread rows | phase 0 (route), W2-E (job) | W2-R, W2-D |
| I-19 | SSE `id:`/replay/`resync`/`session` frames; `?since` list read | phase 0 (types), W2-H | W2-R, W2-D |
| I-20 | I-3 v2 record (same key, `v`, `seq`) and arrival rule v2 | phase 0 (spec) | W2-R, W2-D |
| I-21 | Summary tag `talaria-summary` on page and watcher; its click opens the bell | W2-D (watcher) | — |
| I-22 | Shell bridge: `desktop_notify_close(tags)`, `desktop_os_idle()`; TS stubs in phase 0. The watcher heartbeats presence (`kind: desktop`) and acks with the instance cookie | phase 0 (TS), W2-D | W2-R |
| I-23 | `features[]` in GET; one const per owning module | phase 0 | everyone |
| I-24 | `NotifyMeta`, `group_key::*`, `entity::*`, `resolve_entity`/`clear_entity`, `add_notification_with` | phase 0 | W2-P, W2-E |

**I-8, amended.** A known id with a later `createdAt` is a re-alert. Since the shell's
review fix N7, a `renotify` post on Linux closes the old OS banner and posts a new one; on
macOS and Windows a re-post under the same identifier replaces it. The rule every consumer
keeps: one OS banner per (instance label, tag) on screen, and a click on a superseded id is
"not ours". "Same OS id" is withdrawn.

**The degrade rule.** Gate new behaviour on `features` (a client) or on the presence of
the contract's API (the server). With another stream absent, a stream behaves as wave 1
does on that axis, so the streams merge in any order after the contract.

## The schema (one block, no backfill)

Nine statements appended to `MIGRATIONS` in `ui/src/server/db/pg.ts`, each in its own
transaction:

1. `users.notify_seq bigint not null default 0`, the per-user cursor.
2. Seventeen columns on `notifications`: `seq`, `group_key`, `count` (default 1),
   `updated_at`, `alert`, `actor_kind`, `actor_id`, `actor_label`, `entity_type`,
   `entity_id`, `archived_at`, `surfaced_at`, `push_due_at`, `pushed_at`, `email_due_at`,
   `emailed_at` and `email_attempts` (default 0).
3. `notifications_user_seq_idx`, unique `(user_id, seq) where seq is not null`.
4. `notifications_open_group_idx`, unique `(user_id, group_key)` over open rows
   (`group_key is not null and read_at is null and archived_at is null`): the
   coalescing target.
5. `notifications_entity_open_idx`, `(entity_type, entity_id)` over unread rows with an
   entity.
6. `notifications_push_due_idx` and 7. `notifications_email_due_idx`, the two due-work
   scans.
8. `channel_members.notify_level` and `muted_until`.
9. `push_subscriptions.user_agent`.

There is **no data backfill**. `seq` stays NULL on rows filed before the contract, and
those rows keep wave 1's arrival rule (I-20). The class is derived from `kind` at read time
(`notify_class_of`); there is no class column, because a stored class would need a
full-table UPDATE and would go stale whenever a kind is remapped. An open pre-contract row
joins a group by adoption at write time (`NotifyMeta.adopt_legacy`), not by an UPDATE over
the table. Every `ADD COLUMN` has a constant or NULL default, which is metadata-only on
PostgreSQL 11 and later.

### The measured cost (C32)

Main's dev database is empty and the live instance is off-limits, so the cost was measured
on a synthetic volume: a scratch database `c32` inside the contract's own Postgres 16.15,
holding a `pg_dump -s` of the wave-1 schema, 200 users and 1,000,000 notifications
(5,000 each, 70 % read, a realistic kind mix, bodies of 80–200 characters, `created_at`
over 180 days), and 5,400 `channel_members` rows. Statements 1–9 ran exactly as the
migration runner runs them, each in its own transaction, while a 2-client `pgbench`
writer inserted one notification and read that person's newest 50 in a loop. Three runs:

| Statement | Lock held | Server time, runs 1 / 2 / 3 | Longest writer wait overlapping it, runs 1 / 2 / 3 |
|---|---|---|---|
| 1 `users` ADD COLUMN | AccessExclusive on `users` | 8 / 151 / 217 ms | 12 / 415 / 433 ms |
| 2 `notifications` 17 × ADD COLUMN | AccessExclusive on `notifications` | 8 / 12 / 9 ms | 17 / 25 / 25 ms |
| 3 seq unique index | Share on `notifications` | 155 / 1,102 / 555 ms | 158 / 784 / 411 ms |
| 4 open-group unique index | Share | 170 / 524 / 228 ms | 184 / 780 / 238 ms |
| 5 open-entity index | Share | 165 / 594 / 797 ms | 184 / 616 / 868 ms |
| 6 push-due index | Share | 168 / 220 / 550 ms | 183 / 616 / 868 ms |
| 7 email-due index | Share | 177 / 185 / 198 ms | 187 / 194 / 207 ms |
| 8 `channel_members` 2 × ADD COLUMN | AccessExclusive on `channel_members` | 2 / 2 / 2 ms | 187 / 409 / 413 ms |
| 9 `push_subscriptions` ADD COLUMN | AccessExclusive on `push_subscriptions` | 2 / 2 / 2 ms | 33 / 8 / 3 ms |
| **The block** | | **0.85 / 2.79 / 2.56 s** (wall with process overhead: 1.7 / 4.0 / 3.8 s) | **0.19 / 0.78 / 0.87 s** |

The `ADD COLUMN`s were metadata-only: applied on their own, statements 1, 2, 8 and 9 left
the `relfilenode` and on-disk size of `users`, `notifications`, `channel_members` and
`push_subscriptions` exactly as they were. The ones whose server time reached ~200 ms were
waiting for the writer's open transaction to commit, not rewriting anything. Each new
partial index is 8 KB on this volume (no row has a seq, a key, an entity or a due time
yet). The host was under IO pressure (`/proc/pressure/io` some avg10 peaked at 33, 38 and
29 % during the three runs), and run 2 overlapped a checkpoint: its 8.7 s worst writer latency came 11 s after
the block had finished, outside every statement's window. The thresholds (block ≤ 10 s,
writer stall ≤ 5 s) held on every run, so neither named fallback (dropping statement 3,
then folding 6 and 7) was applied.

The write path itself, over 1,000 sequential writes spread over 50 of those users. Wave 1's
path on the wave-1 schema: p50 2.76 ms, p95 5.62 ms. After the block, three runs, each beside
the same run's wave-1 path as a baseline:

| | p50, runs 1 / 2 / 3 | p95, runs 1 / 2 / 3 |
|---|---|---|
| Wave 1's insert-only path | 2.82 / 2.77 / 2.66 ms | 10.14 / 3.92 / 3.77 ms |
| `add_notification` (seq, no key) | 3.23 / 3.15 / 2.96 ms | 6.12 / 6.49 / 4.16 ms |
| A keyed write (window and adoption on) | 3.85 / 3.82 / 3.79 ms | 7.74 / 7.56 / 5.48 ms |

Against the steadiest baselines the p95 rose by at most 2.6 ms for the default path and
3.6 ms for a keyed write, under the 5 ms threshold.

## The write path (I-10, I-24)

`add_notification` and `add_notification_capped` keep their signatures, so every
producer compiles untouched; they file through `add_notification_with` with
`NotifyMeta::default()`. One transaction per recipient:

1. `update users set notify_seq = notify_seq + 1 … returning notify_seq, notify_prefs`.
   The row lock is held to commit, so two writers for one person serialize, and seq order
   is commit order. No users row means the person is gone: `RowNotFound`.
2. With a group key and a window: an open row of the group untouched for longer than the
   window loses its key and keeps its content.
3. With a key and `adopt_legacy`, and no open row holding the key: the newest open,
   unkeyed row of the same kind and href takes the key. Same id, so an inbox-focus snooze
   on it stays.
4. Insert, or `on conflict` the open row of the group: content, `count + 1`, the new
   seq, `updated_at`, and for a `Realert` coalesce a new `created_at` (a `Quiet` one keeps
   it). The update applies only to a row that is still open; a row read in between is
   retried as a fresh insert.

**The clock is read under the lock.** `now()` is the transaction's start, and a writer
queued behind the person's row lock started before the commit it waited for. So a fresh
row's `created_at` is `clock_timestamp()`, and a `Realert` (or a refresh) sets
`greatest(clock_timestamp(), created_at + 1 ms)`. A person's `createdAt` therefore rises
with their `seq`, and a re-alert is always "a known id whose `createdAt` moved later" — to
the millisecond the wire carries. (Measured before the fix: 16 of 49 coalesce steps under
50 concurrent writers moved `createdAt` backwards or not at all.)

After commit: `{type:"notification", notificationId, seq}`, the brief nudge, and
`deliver::after_write` — in the contract, wave 1's tail moved: push when the class's Push
channel is on (for every blob wave 1 could store, that is "the route is not email-only"),
unless the producer said `quiet`/`skip_push`; mail through the in-memory outbox as before.
A row's `alert` is `feed` when the producer muted it or the class's Desktop channel is
off, else `banner`.

Reading or archiving never takes a seq. A read row leaves the open-group index, so the
next write files a fresh row. `refresh_notification` keeps its signature and meaning, and
now takes a seq too.

**Reopening a row** (`read_at = null`: the Home Inbox's undo today, W2-H's mark-unread)
goes through `reopen_notifications(pg, user, ids, read_at?)` and nowhere else. A read row
may have been succeeded by a newer open row of its group, and putting it back with its key
would be a second open row for one (person, key), which the unique index refuses. The door
locks the person's users row as writers do, and a reopened row whose key another open row
holds (or a newer row reopened in the same call) comes back without its key. Archived rows
are not reopened. The optional `read_at` guard compares to the millisecond.

## Prefs v2 (I-13), DND (I-14)

Storage stays `users.notify_prefs`. The reserved keys: the original class ids (legacy
routes), `digest`, `prompt`, `channels`, `dnd`, `email`.

- **Channels.** `channels_of(blob, class)` resolves each of desktop, push and email on its
  own: a stored `channels.<class>.<channel>` bool, else what the legacy route meant
  (`in_app` → desktop and push; `email` → email only; `both` → all three), else the
  registry default. In-app is always on; `PATCH {channels:{x:{inApp:false}}}` is a 400.
- **Both ways.** A v2 write merges its bools and rewrites the class's legacy route (email
  and a screen → `both`, email alone → `email`, no email → `in_app`). A legacy write from a
  stale bundle writes the route and the class's channels.
- **What the contract already enforces.** Email follows the route, as in wave 1. Push
  follows `channels.<class>.push`. Desktop sets the row's `alert` (`feed` when off), which
  clients honour from W2-R and W2-D on. DND is stored and evaluated, but not enforced
  until W2-E.
- **Digest frozen (C27).** With no explicit `digest`, it is on when any of the nine
  original classes has email on. A new class cannot switch it on.
- **DND.** `dnd: {pausedUntil, schedule: {enabled, days, start, end}, allowNeedsYou}`.
  `schedule` is working hours: inside them notifications alert, outside they are quiet;
  `days` uses 0 = Sunday, and `start > end` crosses midnight. `pausedUntil =
  9999-12-31T23:59:59.999Z` is "until I resume". `dnd_state(blob, zone, now)` is pure and
  is the one evaluator: the GET serves its answer, and W2-E's delivery calls it. The zone
  is `users.timezone`, else UTC.
- **Email frequency.** `email.frequency` ∈ `15m`, `1h`, `1d`, `off`; default `1h`.
- **The delivery switch (C28)** merges `emailEnabled` into its row instead of replacing
  the row.

## The class registry (I-12)

`api/crates/talaria-notify/src/classes.rs` holds `KIND_CLASS` (kind → class, reason) and
`CLASSES` (id, label, blurb, category, defaults, breaks-through, admin-only, email delay),
served in order as `classes[]`. Members are not served admin-only classes. For you =
`needs_you` + `direct`. The registry adds `participating` and `channel_activity`; neither
has rows until W2-P files their kinds.

Filters (W2-H) read the class through `kind`: `kinds_of(class)`, `kinds_of_category(c)`
and `known_kinds()`. A kind the table does not name is filed under `FALLBACK_CLASS`
(`work_complete`, a Following class), so a filter that includes that class or category is
`kind = any(kinds_of(…)) or kind <> all(known_kinds())`. For you is a positive list
(`for_you_kinds()`), because no fallback kind is For you. `scripts/check-invariants.mjs` fails a writer
whose literal kind the table does not name, and a client file that keeps its own class
list.

## Wire changes (all additive)

- **`GET /api/notifications`** keeps every wave-1 key in order and adds `unreadForYou`,
  `seq` (read in the same statement as the rows), `classes`, `channels`, `dnd` (stored
  plus `active`, `until`, `reason`, `timeZone`), `emailFrequency`, `features` and `next`
  (null until W2-H). Each row adds `seq`, `alert`, `class`, `category`, `count`,
  `updatedAt`, `archivedAt`, `actor` `{kind, id, name}`, `entity` `{type, id}` and
  `reason`.
- **`PATCH /api/notifications`** accepts `channels`, `dnd` and `emailFrequency` and
  answers with them.
- **`PUT /api/notifications`** publishes the read-sync event v2.
- **New routes:** `POST /api/notifications/ack`, `POST /api/notifications/test` (501
  until W2-E; 429 past 5 in 10 minutes), `GET /api/push/devices`, `DELETE
  /api/push/devices/{id}`, `PUT /api/channels/{id}/notify`, `GET /api/me/presence`, and a
  body on `PUT /api/me/presence` (no body is wave 1's ping, exactly).
- **The user stream:** `notification` gains `seq`; `notifications` gains `unreadForYou`,
  `readIds` (≤ 100), `all` and `resync`; new `presence`, `resync` and `session` frames.
  Wave-1 consumers ignore all of it: the shell's watcher reads only `type` and `unread`,
  the page's switch has no default case, and the Kotlin client decodes with
  `ignoreUnknownKeys` and drops unknown types.
- **The channel read and the agent-thread read** publish the read-sync event. The thread
  read published nothing before the contract.
- **Push headers:** a note may carry `TTL`, `Urgency` and `Topic`; with none it sends
  wave 1's `TTL: 3600`, `Urgency: normal` and no `Topic`.

**`features`** in the contract: `seq`, `classes`, `channels`, `dnd-store`,
`presence-store`, `ack`, `devices`, `read-ids`, `channel-levels-store`. Each stream adds
its own in its own module's `FEATURES`.

## The live stream (I-16, I-19)

The types are the contract's; W2-H implements the framing, the replay and the revocation.

- **Read-sync (I-16).** Every read path publishes
  `{"type":"notifications","unread":N,"unreadForYou":M,"readIds":[…],"all":bool}` on
  `user:<id>`. `readIds` holds exactly the rows just read, at most 100; past that it is
  absent and `resync: true` says "refetch". `unread` and `unreadForYou` exclude archived
  rows. Every client closes the OS banners it posted for the read ids (`all`: every
  Talaria banner; `resync`: every banner whose row is no longer unread). `publish_unread`
  (counts only, no ids) remains for a caller that does not know which rows were read; a
  client never closes banners on it.
- **`id:` framing.** A frame carrying a `notification` event has `id: <seq>` before its
  `data:` line. Other frames carry no `id:`.
- **Replay.** `GET /api/me/events?since=S` (or `Last-Event-ID: S`; the query wins): after
  `: connected` and `retry:`, the server sends the current `presence` frame, then one
  `notification` frame per row with `seq > S` in seq order (at most 100), then
  `{"type":"resync","seq":N,"complete":true}`. Over 100 rows it sends only
  `{"type":"resync","seq":N,"complete":false}`, and the client reads `?since` itself. Live
  frames that land during the replay are buffered and deduplicated by seq.
- After the hub's Redis connection comes back, every user stream gets
  `{"type":"resync","seq":N,"complete":false}`.
- A stream whose session ended sends `{"type":"session","state":"ended"}` and closes.
- **`GET /api/notifications?since=S`** (W2-H): the rows with `seq > S`, in seq order, at
  most 200, with `more` and `seq`, from one statement.
- **The GET's top-level `seq` is not a read-through.** It is read in the same snapshot as
  the rows, so every row with a seq at or below it had committed. But the list is the
  newest 50 by `createdAt` (and a `Quiet` coalesce keeps an older `createdAt`), so only a
  `?since` read whose `more` is false advances a client's record (I-20).
- Older consumers ignore all of it (see the wire changes above).

## Presence (I-17) and the ack (I-18)

- **The heartbeat.** `PUT /api/me/presence` with `{clientId, kind: "web"|"desktop", state:
  "looking"|"active"|"idle"|"hidden", osIdleMs?}`, where `clientId` matches
  `[A-Za-z0-9_-]{8,64}` and is stable for the client's life (per tab for the web, `desk-<instance
  id>` for the shell's watcher). The client computes the state. **looking**: visible,
  focused, and input in the last 60 s. **active**: for the web, visible with input in the
  last 120 s; for the desktop, OS input in the last 120 s. Otherwise **idle** or
  **hidden**. A PUT with no body (or no `clientId`) is wave 1's online ping, exactly.
- **The online key** (`GET /api/users`' presence) is set by a no-body ping, by a web
  heartbeat unless it is `hidden`, and by a desktop heartbeat only while `looking` or
  `active` (`presence::marks_online`). The shell runs in the tray with every window
  closed, and an idle machine must not show its owner online all night.
- **The store.** The Redis hash `presence:dev:<userId>` maps `clientId` to
  `{"k":kind,"s":state,"at":ms,"i":osIdleMs}`, with `EXPIRE 120` on every beat. Entries
  older than 75 s are pruned on read.
- **The winner** (`presence::winner`, pure): the most recent `looking` client, else the
  most recent `active` one. The `presence` frame `{"type":"presence","looking","active",
  "clients"}` is published on `user:<id>` only when that pair changes (the previous pair is
  cached under `presence:win:<userId>`, `EX 120`). It is recomputed only on a heartbeat,
  so a client that stops beating is dropped from the winner at the next beat of any of the
  person's clients after 75 s. `GET /api/me/presence` serves `{looking, active, clients:
  [{clientId, kind, state, at}]}`.
- **The ack.** `POST /api/notifications/ack {ids: uuid[] ≤ 200}` → `{ok, acked}` sets
  `surfaced_at` on the caller's own rows (the first ack wins; another person's id acks
  nothing). The client that toasted a row to a looking person, or posted an OS banner for
  it, acks it. W2-E's deferred push fires only for rows still unread and un-acked.
- The thresholds (60 s, 120 s, 75 s, the 60 s push deferral) are consts W2-E may tune.

## The arrival record (I-20)

The record stays under `localStorage['talaria.notify.arrivals.v1']`, keyed by user id,
and gains two fields; nothing is renamed:

```
{ "<userId>": {
    "hw":   "<ISO createdAt high-water>",          // wave 1
    "seen": { "<rowId>": "<ISO createdAt as last surfaced>", … ≤ 300 },   // wave 1
    "at":   <ms of the last successful read>,      // wave 1
    "v":    2,                                     // new
    "seq":  <the notify seq this browser has read THROUGH, or absent> } }   // new
```

- **A row with a `seq`, when the record has one:** an arrival if and only if it is unread,
  `row.seq > rec.seq`, and its id is unseen or its `createdAt` is later than `seen[id]` (a
  re-alert). `row.seq ≤ rec.seq` is never an arrival, and no margin is needed.
- **Otherwise** (a legacy row with no seq, or a wave-1 record): wave 1's rule, the
  high-water minus 120 s plus `seen` and `createdAt`.
- An arrival whose `alert` is `feed` is recorded and never toasted or bannered (I-15).
- `rec.seq` advances only to the `seq` of a `?since` read whose `more` is false.
- The page tracker (W2-R) and the shell's merge (W2-D) write `v` and `seq`. A wave-1
  shell's merge mutates the entry in place and keeps them; its report reads only
  `hw`/`seen`/`at`.

## Notes for the streams

Found while reviewing the contract. Each is a constraint a stream builds within, not a
change to its plan:

- **W2-E's Redis handle.** `NotifyDeps` carries a Postgres pool and the publish plane,
  and no Redis connection, so the write path cannot reach the presence store through it
  (and `lib.rs` is frozen). Install a handle at boot instead, beside
  `talaria_push::install_push_plane` in `talaria-jobs`, as the push plane holds its pool.
  If the handle is absent or Redis errors, report "no winner": fail open toward delivery.
- **The presence winner cache is not atomic.** Two heartbeats racing can each publish,
  and the later write of `presence:win` can be the older pair; the next beat (≤ 30 s)
  corrects it. W2-E may make it a single script if that matters for the deferral.
- **Team-roster channels have no row to hold a level.** A team member who reaches a
  channel through `channel_teams` (no `channel_members` row) gets 409 from `PUT
  /api/channels/{id}/notify`, as a task room does. They also get no `channel-message`
  rows, because `channel_member_ids` reads `channel_members` alone. Giving them levels
  needs a schema decision (a per-person table, not an inserted membership row, which would
  outlive their team membership). That decision is the coordinator's, and W2-P builds the
  rest without it.
- **Mark-unread is W2-H's through `reopen_notifications`**, never a bare `read_at = null`.
- **The GET seq**: see the live stream above. The newest 50 are not a read-through.

## Who owns what in wave 2

| Module or file | Owner |
|---|---|
| `talaria-notify` `lib`, `meta`, `href`, `brief`; this file | frozen |
| `talaria-notify` `classes`, `prefs`, `write`, `deliver`, `presence`, `defer`, `mail`, `retention`; `talaria-push` | W2-E |
| `talaria-notify` `inbox`; `talaria-realtime` | W2-H |
| `talaria-notify` `threads` | W2-P |
| `ui/src/lib/notify-wire.ts` | contract, then W2-R (fixes only) |
| `ui/src/lib/notify-status.svelte.ts` | W2-C (W2-R calls its setters) |

## The streams' own docs

- [Delivery engine](./delivery.md) (W2-E)
- [The live stream and the inbox API](./live-and-inbox.md) (W2-H)
- [Producers](./producers.md) (W2-P)
- [The web runtime](./web-runtime.md) (W2-R)
- [The notification center](./center.md) (W2-C)
