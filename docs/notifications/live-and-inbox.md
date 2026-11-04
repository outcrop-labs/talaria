# The live stream and the inbox API

How a person's notifications reach their open pages and devices, how a stream that dropped
resumes without losing anything, how a stream ends when its session does, and the inbox
read/write routes the notification center is built on. Owned by W2-H. The interfaces it
builds on (I-10 seq, I-16 read-sync, I-19 the SSE wire) are in [the contract](./contract.md);
the user-facing story is [`../NOTIFICATIONS.md`](../NOTIFICATIONS.md).

## The hub: one subscriber connection per process

Every event stream (`GET /api/me/events`, `/api/boards/{id}/events`,
`/api/channels/{id}/events`, `/api/runs/{id}/events` and the run-watch tail) reads from
the process's one Redis subscriber, `talaria-realtime`'s hub. Wave 1 opened a Redis
connection per stream (SE-7); with the hub, 50 user streams and 20 board and channel
streams are one connection in `CLIENT LIST`.

- A topic is `SUBSCRIBE`d while at least one stream wants it and `UNSUBSCRIBE`d when the
  last one leaves (counted per topic). Closing a stream releases its topic at once, so
  `PUBSUB NUMSUB` drops to 0 as soon as the client goes.
- Each topic fans out in-process over a broadcast channel of 256 messages. A stream that
  falls further behind than that has lagged, and it ends; its client reconnects and resumes
  by seq.
- One driver task owns the connection. It sends one channel per `SUBSCRIBE` (a
  multi-channel command answers once per channel, and the client matches replies to
  requests in order), `PING`s every 30 s so a half-open socket is noticed, and bounds each
  command at 5 s.
- A subscriber is answered only once Redis holds its topic, so a read the route makes after
  subscribing cannot miss a publish made after it. The wait is bounded (3 s): when Redis is
  slow or the hub is reconnecting, the stream goes ahead, and a user stream is sent
  `resync` (`complete: false`) the moment its topic is confirmed, because whatever was
  published before that never reached it.
- When the connection drops, the driver reconnects at once, then backs off from 100 ms,
  doubling to 5 s, resubscribes every live topic, and sends every **user** stream
  `{"type":"resync","seq":N,"complete":false}` (N is the person's seq now). Whatever was
  published in the gap is gone from pub/sub; the client's `?since` read recovers it.
- If the driver itself ends (a panic, or the runtime it ran on going away), the hub closes:
  its streams end, their clients reconnect, and the next subscribe starts a fresh hub.
- The publish side is unchanged: publishes ride the shared `ConnectionManager`.

## The person's stream (I-19)

`GET /api/me/events` sends, in order:

```
: connected
retry: 3000

data: {"type":"presence","looking":"tab-…","active":null,"clients":2}

id: 41
data: {"type":"notification","notificationId":"…","seq":41}      ← the replay, one per row

data: {"type":"resync","seq":47,"complete":true}

…live frames; a notification keeps its "id: <seq>"
```

- **The presence frame** is sent on every open: the frame is otherwise published only
  when the winner changes, and a stream that just opened has seen none.
- **Resuming.** `?since=<seq>`, or `Last-Event-ID: <seq>` (an `EventSource` sends it by
  itself on a reconnect; the query wins). The route subscribes first, then reads, in one
  statement, the person's seq N and every row with `seq > since` in seq order (at most
  101). Up to 100 rows: one `notification` frame per row, each with `id: <seq>`, then
  `{"type":"resync","seq":N,"complete":true}`. Over 100: only
  `{"type":"resync","seq":N,"complete":false}`, and the client reads
  `GET /api/notifications?since=` itself. A value that is not a whole number is ignored and
  the stream opens fresh.
- **The answered seq is the server's, even when it is lower than `since`.** A client that
  asks from a seq past the person's (the database was restored from a backup, or a dev
  stack was reset) gets no rows and `{"type":"resync","seq":N,"complete":true}` with `N`
  below what it sent; `GET ?since=` answers the same `seq`. A client must then take `N` as
  its record, not keep the larger value, or every row up to its old seq would never count
  as new.
- **Each row exactly once.** A write that committed before the replay's snapshot is in the
  replay, and its live frame (published after its commit, so after the subscribe) has a
  seq ≤ N and is dropped. A write after the snapshot has a seq > N and arrives live. A row
  that coalesced after `since` is replayed once, with its newest seq.
- **The replay is every row after `since`**: read, unread and Done alike. The question is
  "what happened after S", and the client decides what to show.
- Frames that are not `notification` carry no `id:`. A browser keeps the last `id:` it saw
  across those frames, so `Last-Event-ID` stays the newest notification seq.

### The session ends the stream (SE-16)

Three doors, each sending `{"type":"session","state":"ended"}` and closing the stream:

- **Logout.** `POST /api/auth/logout` deletes the session, then publishes its hash on
  `session:revoked`. Every stream that session opened, on every api process, ends at once.
  The person's other sessions keep streaming.
- **Before every live frame**, the stream re-reads its session. Nothing is delivered to a
  session that is gone, however it went: deleted, expired, or logged out on a process that
  could not publish.
- **Every 20 s**, a quiet stream re-reads it anyway, so the 7-day expiry or a deleted key
  ends it within one tick.

A session read that fails (Redis unreachable) is not an ending: the stream carries on, and
the next read decides.

### Shutdown

On SIGTERM the api calls `end_streams()`: every event stream ends, and the hub closes,
which also ends the streams a route built on the hub by hand (the run-watch tail had no
shutdown hook of its own). Each client reconnects after its `retry:`, with its seq. The
HTTP drain then waits at most 5 s for any other in-flight response, the run drivers (≤ 5 s)
and the scheduler (≤ 30 s for a job in flight) drain after it, and the pool gets 1 s to
take its connections back. Each phase logs how long it took
(`[shutdown] http drained in …`, `run drivers drained in …`, `scheduler stopped in …`,
`complete …`), so a stop that lingers names what it waited for.

The pool bound is the one that mattered. With 20 streams open, a stop took 11–35 s in 3 of
10 tries while every stream had ended in 15 ms and no run or job was in flight: the pool's
`close` was waiting for a query in a detached boot task (the builtin MCP rows' seed, an
insert held behind a slow commit). Past the bound that connection drops with the process,
as the container's own kill would drop it.

## The inbox API

### `GET /api/notifications`

With no parameters it is wave 1's read: the newest 50 rows, Done ones included, every
wave-1 key in order. Parameters, all optional:

| Parameter | Meaning |
|---|---|
| `view=foryou\|all\|done` | For you = Needs you + Direct, not Done. All = everything not Done. Done = archived rows, the most recently archived first |
| `unread=1` | unread rows only |
| `category=<list>` | `needs_you`, `direct`, `following`, `workspace`, comma-separated |
| `class=<list>` | class ids from `classes[]` |
| `kind=<list>` | raw kinds, at most 50 |
| `limit=1..100` | the page (default 50) |
| `before=<cursor>` | the next page: the previous answer's `next` |
| `since=<seq>` | instead of a page: every row with `seq > since`, in seq order, at most 200. The view parameters are ignored |

Lists are OR'd within a parameter and AND'd across parameters. A kind the class registry
does not name belongs to the fallback class (`work_complete`, a Following class), so a
`category=following` or `class=work_complete` filter includes it. Unknown parameters are
ignored; a bad value is a 400 (`unknown category`, `unknown notification class`,
`limit must be a whole number from 1 to 100`, `invalid cursor` (also for a cursor whose
time is outside years 1 to 9999), `since must be a whole number`, the view's enum
message).

The answer is every key the GET always served, plus:

- `next`: the following page's cursor, or null at the end. Pages are keyset, not offset:
  `(order key, id) < cursor`, newest first. A row filed while someone pages lands above the
  first page and never shifts the next one. The order key is `created_at` (a re-alert moves
  it up), or `archived_at` in Done. The cursor is opaque: inside, it holds the order key to
  the microsecond (the column's own precision, so two rows in one millisecond can never
  fall between pages) and the row id.
- `more`: rows remain past this answer. For a `since` read, `more: false` means the client
  has read through `seq` (I-20); with `more: true` it reads again from the last row's seq.
- `seq` is read in the same statement as the rows (I-10).
- `unread` and `unreadForYou` come from one read and both leave Done rows out, so they
  never disagree (N3). `/api/unreads`' `notifications` arm is the same "all unread" count
  (O-10), and its `research` arm leaves Done rows out too.

### `PUT /api/notifications`

A selector and at most one action:

| Member | Meaning |
|---|---|
| `ids: uuid[] ≤ 200` | these rows |
| `href: string` | every row pointing at this place |
| `entity: {type, id}` | every row about this thing (`task`, `channel`, `conversation`, `approval`, `mcp_server`, `plan`, `research_run`, `kb_doc`) |
| *(none, or empty ones)* | every row |
| `archive: true` | Done: sets `archived_at`, and `read_at` if it was unread (Done also marks read, O-11) |
| `unarchive: true` | back from Done; stays read. Needs a selector |
| `unread: true` | unread again (read rows that are not Done), through `reopen_notifications`. Needs a selector |
| *(no action)* | mark read (wave 1) |

The selector precedence is ids, then href, then entity. Every selector reaches only the
caller's own rows; the entity read maps to `clear_entity`, never `resolve_entity` (C24).
The answer is `{ok, read, archived, unread, unarchived}`, each the number of rows that
changed that way.

Every path publishes the read-sync event (I-16). A read or an archive names its rows in
`readIds` (an archive counts as read: every client closes the banners it posted for those
rows and drops them from its views), with `all: true` when there was no selector. An
unarchive or a mark-unread publishes the counts (no ids; nothing was read), and every page
refetches on it.

Because Done marks read, every reader that counts or lists unread rows (the bell, the Home
Inbox, the daily brief, the rail badges) drops a Done row with no change of its own.

### Reads, reopens and the deliveries

A row can carry a deferred push and a pending mail (docs/notifications/delivery.md). Reading
and reopening treat them this way:

- **A read cancels the deferred push.** Every read in `inbox.rs` (ids, href, all, Done, the
  entity clears) and the Home Inbox's mark-read clear `push_due_at`, unless the push already
  went. A read leaves the mail's due time alone: the batcher never mails a read row, and the
  due time is how a reopen knows a mail was pending.
- **A reopen never brings the push back.** `reopen_notifications` clears `push_due_at` too,
  which also covers a row read by a path outside these files. Before this, a row read inside
  its 60 s deferral and marked unread was pushed at its old due time.
- **A reopen re-arms a pending mail from now.** The new due time is the one a write filed
  now would get: the class's own delay (15 minutes for Needs you), or else the person's
  frequency (15 minutes, an hour, or the next 09:00 on their clock). Their clock is their
  profile's zone, else the workspace's (the brief's `timeZone`, else `TZ`, else UTC), as the
  engine's write reads it. With `off`, no mail is armed. Before this, a reopened row was
  mailed at its old due time, which was often already past, so the mail went within a minute
  of the reopen. The batcher still checks DND when it sends.
- **A row already mailed stays mailed,** because a reopen is not new activity. A row the
  batcher gave up on stays given up, and a row that never had a mail gets none.

### The Home Inbox

The Home Inbox's mark-read of a notification publishes the read-sync event with that row's
id, so the bell drops in every tab. Its undo, which makes the row unread again, publishes
the counts.

### Features

`inbox::FEATURES`: `since`, `inbox-v2`, `sse-replay`, `archive`. Clients enable the UI on a
feature, never on a version number.

## Verifying

- `cargo test -p talaria-realtime` covers the hub's counting, the reconnect marker, a
  lagging stream, the replay/live dedupe, the `id:` framing, and the three session doors.
- `api/tests/it/notify_hub_live.rs` runs against a real Postgres and Redis: one subscriber
  connection for 70 streams, `NUMSUB` 0 after they close, `CLIENT KILL` and the resync, a
  write racing the replay arriving exactly once over 20 runs, the 100-row cap, and the
  route's replay (query and `Last-Event-ID`), logout and a deleted key.
- `api/tests/it/notifications_reads.rs` covers the views, paging, filters against a SQL
  oracle, Done and back, mark unread, the entity selector, and what a read and a reopen do to
  the deferred push and the pending mail, including a person with no zone of their own in a
  workspace on another clock.
