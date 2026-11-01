# The notification center

The bell and `/notifications`, the category × channel matrix, Do Not Disturb (pause and
working hours), and the status panel: the parts of notifications a person sees and sets.
Owned by W2-C. The interfaces it builds on are in [the contract](./contract.md); the
user-facing story and troubleshooting are in [`../NOTIFICATIONS.md`](../NOTIFICATIONS.md).

## What it reads and writes

| Surface | Reads | Writes |
|---|---|---|
| The bell | `GET /api/notifications` (the newest 50, `unreadForYou`, `classes`, `dnd`, `features`); with `inbox-v2`, `?view=foryou` or `?view=all` (30 rows) while open | `PUT {ids}` (open, read), `PUT {ids, archive}` (Done), `PUT {ids, unread}`, `PATCH {dnd}` (pause, Resume) |
| `/notifications` | `?view=foryou\|all\|done`, `unread`, `category`, `kind`, `limit`, `before` (W2-H), paged on `next` | the same `PUT`s, plus `{ids, unarchive}` (Move to inbox) |
| The matrix | `classes`, `channels`, `delivery.emailEnabled`, `emailFrequency` | `PATCH {channels}`, `PATCH {emailFrequency}`; the digest and the admin email switch as before |
| DND | `dnd` (stored and evaluated: `active`, `until`, `reason`, `timeZone`), the profile's zone (`GET /api/me`) | `PATCH {dnd: {pausedUntil \| schedule \| allowNeedsYou}}` |
| The status panel | `GET /api/push/devices`, `GET /api/me/presence`, `notify-status` (W2-R's setters), the shell bridge | `POST /api/notifications/test`, `DELETE /api/push/devices/{id}`, the shell's test |

Every list read sits under the query key `['notifications', …]`, so the live stream's
invalidation of the bell refreshes the page's views too.

## The rules it keeps

- **The badge counts For you** (O-10): `unreadForYou`, Needs you plus Direct. Following
  shows in All without badging. An api older than the contract has no such field, and the
  badge counts every unread row, as wave 1 did.
- **For you is `category` in {`needs_you`, `direct`}.** A row without a category (an api
  older than the contract) is For you.
- **Grouping is consecutive rows about one entity.** A run of rows with the same
  `entity.type` and `entity.id` goes under one collapsible header; a row about something
  else in between ends the run, so a group never pulls an older row above newer ones. The
  header is "\<name\> · N updates": a ticket key every row carries (`TAL-112`), else the
  entity type's word ("Ticket", "Channel").
- **The count** is the coalesced row's `count` (W2-P), worded by kind: "3 messages",
  "Moved 4 times", "Down 2 times"; "N updates" otherwise.
- **The reason** is the row's `reason`, else the registry's reason for its kind.
- **Opening a row** is wave 1's one door (`openNotification`): route in place, mark read.
- **Done also marks read** (O-11, the server's rule). Move to inbox brings a row back read.
- **Undo.** Each Done and Move to inbox can be taken back for a few seconds: the bell's
  "Marked done · Undo" line, and on the page the bar under the header or `z`. Undoing a
  Done puts the rows back exactly: in the inbox, and unread again where they were unread.
- **Bulk writes** go in slices of 200 ids, the most one `PUT /api/notifications` takes, so
  `Shift+E` over a long scrolled list marks every row done.
- **Rows line up.** A row with a person behind it shows their avatar, an agent's row the
  agent glyph, and any other row its category's glyph, so every title starts at one place.
  The row's actions show on hover or focus, and always on a screen with no hover.
- **The degrade rule (I-23).** Without `inbox-v2` there is no Done, no Mark unread, no Done
  view and no paging; For you and All slice the newest 50, and the page's filters run on
  those 50 in the browser. Nothing else changes.

## The matrix

Under the `sm` breakpoint each row's label takes its own line and the four channels sit
under it, each with its name; from `sm` up it is one row with column heads.

Its rows are `classes[]`, grouped by category in the order Needs you, Direct, Following,
Workspace (a category the client does not know goes last, under its id). Members are not
served admin-only classes, so today a member's matrix has 9 rows and an admin's 11, with
no client code knowing why. The cells are `channels[class]`. In-app is always checked and
cannot be changed. Email is disabled, with the instance switch's explanation, while
`delivery.emailEnabled` is false. "Reset" writes the registry's defaults as explicit bools
(the `PATCH` has no unset); it shows only on a row that is not at its defaults.

The server rewrites each class's legacy route from the matrix (I-13), so `prefs` stays in
step: email and a screen → `both`, email alone → `email`, no email → `in_app`.

There is no client-side class list. `ui/src/lib/notify-classes.ts` keeps only the legacy
types (the row's wave-1 shape, the route, the digest, the instance switch), and
`scripts/check-invariants.mjs` fails any non-test file under `ui/src` that holds three or
more `id: '<class id>'` entries. Its allowlist is empty.

## Do Not Disturb

- **Pause presets**: 30 minutes, 1 hour, 2 hours, until tomorrow 9:00, until I resume
  (`FOREVER`), and until a chosen time. In the account menu and in the bell's header,
  inline (both are popovers already, and a nested popover would close its host).
- **The zone** (O-19): the profile's time zone, else the browser's. "Until tomorrow 9:00"
  is 09:00 on the next calendar day there, computed in `notify-dnd.ts` and stored as the
  UTC instant; the label names the zone when it differs from the browser's.
- **Working hours** (`NotifySchedule.svelte`): days, a start and an end (`start > end`
  crosses midnight), and "Let Needs you through". The server evaluates them in the profile's
  zone, else UTC; the editor says so when the profile has no zone.
- **What is shown**: a pause is read from `pausedUntil` itself, so it shows the moment it
  is saved and goes the moment it ends; a schedule trusts the server's `active` and `until`
  until that `until` passes. The bell's icon becomes a crossed-out bell while either holds.
- **Enforcement** is W2-E's (`dnd-enforced`) and the clients' (W2-R, W2-D). Until the api
  serves `dnd-enforced`, Settings says that a pause is saved and shown but that push and
  email may still arrive.
- **Saves are confirmed in the Do Not Disturb panel itself**, not in the matrix's panel
  above it.

## The status panel

Five sections in Settings → Notifications:

1. **This browser**: the permission (live: `watchPermission`), the service worker, the push
   subscription (the push service's host), whether the server has this browser on file (a
   device from this session), the blocked and insecure steps (`notify-help.ts`), and "Test
   this browser" (a local banner through the registration; no server).
2. **Your devices**: label, "This browser", registered and last seen, the last push's status
   and the server-filtered error, the failure streak, Send test and Remove. The test's
   answer: "Delivered to the push service in N ms…", the push service's refusal, "Server
   test not available on this instance yet." (501, before W2-E), or "Try again in N min"
   (429, past 5 in 10 minutes, O-20).
3. **Live connection**: the event stream's state and this tab's role, from `notify-status`
   (W2-R calls its setters; until it does the panel says it is not reported), and where
   alerts go now by the presence winner (I-17): this browser, Talaria Desktop, another
   browser or tab, or nowhere (no device is active). The sentence under it says only what
   this instance does: that the device being looked at banners and the others stay quiet
   when the web runtime reports (W2-R), and that push waits while a device is in use only
   when the api serves `presence-gated-push` (W2-E); before that, it says every alert with
   Push on is pushed to every registered browser at once.
4. **Talaria Desktop** (in the shell only): the shell's notification service and its test,
   through the bridge (`desktop_notification_status`, `desktop_notification_test`), with a
   link to Settings → Desktop.
5. **Pause**: what DND is doing now, with Resume.

## The keyboard (`/notifications`)

`notify-keys.ts` is pure; the page wires it. The first row is focused on load.

| Keys | Does |
|---|---|
| `j` / `↓`, `k` / `↑` | next, previous (clamped, no wrap) |
| `Enter` / `o` | open |
| `e` | Done the selection, else the focused row (in Done: Move to inbox) |
| `u` | read when any target is unread, else unread |
| `x` | select or unselect |
| `Shift+E` | Done the selection, else every visible row |
| `z` | undo the last Done or Move to inbox |
| `?` | the shortcut sheet |
| `Esc` | clear the selection |

A key typed into a text field is the field's (a checkbox is not one: after a click on a
row's select box the keys still work); a chord with Ctrl, Cmd or Alt is the browser's; a
key typed while the focus is outside the page (the bell's panel, the account menu) is not
the page's. Letters are read case-blind and the bulk Done needs a real Shift, so Caps Lock
neither kills the keyboard nor turns `e` into "Done everything shown".
Nearing the end of what is loaded, `j` fetches the next page.

## Files

- `ui/src/components/app/NotificationBell.svelte`, `UserMenu.svelte` (the pause entry)
- `ui/src/components/notify/NotificationRow.svelte`, `NotificationList.svelte`,
  `NotifyFilters.svelte`, `PauseMenu.svelte`
- `ui/src/routes/app/Notifications.svelte`; `ui/src/route-table.ts` (`/notifications`)
- `ui/src/routes/app/settings/NotificationsSection.svelte`, `NotifyMatrix.svelte`,
  `NotifySchedule.svelte`, `NotifyStatusPanel.svelte`, `NotifyDevices.svelte`,
  `BrowserNotifications.svelte`
- `ui/src/lib/notify-center.ts`, `notify-matrix.ts`, `notify-dnd.ts`, `notify-keys.ts`,
  each with its test; `notify-classes.ts` (the list retired)
- `ui/e2e/notify-center-ui.spec.ts`: the bell, a 390 px window, the matrix, DND, the status
  panel, and (with `inbox-v2`) Done, undo, filters, paging, the keyboard (Caps Lock and a
  focused select box included) and a `Shift+E` over 230 rows

## Verification

- vitest: `notify-dnd.test.ts` (presets including the DST days of New York, London and
  Auckland, a spring-forward gap, the custom time, the zone rule), `notify-matrix.test.ts`
  (rows are the registry for a member and an admin, the overlay, reset, the legacy
  derivation), `notify-keys.test.ts`, `notify-center.test.ts` (views, badge, grouping,
  labels, filters, devices, the alert target).
- `ui/e2e/notify-center-ui.spec.ts` in Chromium and Firefox against a running stack; CI's
  e2e job runs it in Chromium.
