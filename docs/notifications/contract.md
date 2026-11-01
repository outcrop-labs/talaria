# The notification contract (wave 2)

The interfaces every wave-2 notification stream builds against. It is landed by one pull
request (the "contract"): every schema change, the single write path, the shared Rust
types and wire shapes, the route surfaces, the TypeScript types, and the test and doc
scaffolding. With the contract alone, every wave-1 behaviour is identical: new columns are
written, new wire fields are served, and nothing consumes them yet.

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

The `ADD COLUMN`s were metadata-only: `users`, `channel_members` and `push_subscriptions`
kept their exact on-disk size, and the `notifications` heap grew only by the rows the
writer inserted during the run. The ones whose server time reached ~200 ms were waiting
for the writer's open transaction to commit, not rewriting anything. Each new partial
index is 8 KB on this volume (no row has a seq, a key, an entity or a due time yet). The
host was under IO pressure throughout (`/proc/pressure/io` some avg10 18–32 % during the
runs), and run 2 overlapped a checkpoint: its 8.7 s worst writer latency came 11 s after
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
   seq, `updated_at`, and `created_at = now()` for a `Realert` coalesce (a `Quiet` one
   keeps it). The update applies only to a row that is still open; a row read in between
   is retried as a fresh insert.

After commit: `{type:"notification", notificationId, seq}`, the brief nudge, and
`deliver::after_write` — in the contract, wave 1's tail moved: push unless the class's
route is email-only (read through prefs v2), or the producer said `quiet`/`skip_push`;
mail through the in-memory outbox as before. A row's `alert` is `feed` when the producer
muted it or the class's Desktop channel is off, else `banner`.

Reading or archiving never takes a seq. A read row leaves the open-group index, so the
next write files a fresh row. `refresh_notification` keeps its signature and meaning, and
now takes a seq too.

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
has rows until W2-P files their kinds. `scripts/check-invariants.mjs` fails a writer
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
