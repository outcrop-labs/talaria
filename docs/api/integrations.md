# API reference — integrations

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

21 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/integrations/google`](#apiintegrationsgoogle) | GET | `session` |
| [`/api/integrations/google`](#apiintegrationsgoogle) | DELETE | `session` |
| [`/api/integrations/google/agent/calendar`](#apiintegrationsgoogleagentcalendar) | GET | `agent` |
| [`/api/integrations/google/agent/calendar`](#apiintegrationsgoogleagentcalendar) | POST | `agent` |
| [`/api/integrations/google/agent/drive`](#apiintegrationsgoogleagentdrive) | GET | `agent` |
| [`/api/integrations/google/agent/gmail`](#apiintegrationsgoogleagentgmail) | GET | `agent` |
| [`/api/integrations/google/agent/gmail`](#apiintegrationsgoogleagentgmail) | POST | `agent` |
| [`/api/integrations/google/agent/gmail/{id}`](#apiintegrationsgoogleagentgmailid) | GET | `agent` |
| [`/api/integrations/google/agent/gmail/labels`](#apiintegrationsgoogleagentgmaillabels) | GET | `agent` |
| [`/api/integrations/google/agent/gmail/labels`](#apiintegrationsgoogleagentgmaillabels) | POST | `agent` |
| [`/api/integrations/google/agent/gmail/organize`](#apiintegrationsgoogleagentgmailorganize) | POST | `agent` |
| [`/api/integrations/google/calendar/events`](#apiintegrationsgooglecalendarevents) | GET | `session` |
| [`/api/integrations/google/calendar/events`](#apiintegrationsgooglecalendarevents) | POST | `session` |
| [`/api/integrations/google/callback`](#apiintegrationsgooglecallback) | GET | `public` |
| [`/api/integrations/google/connect`](#apiintegrationsgoogleconnect) | GET | `session` |
| [`/api/integrations/google/drive/files`](#apiintegrationsgoogledrivefiles) | GET | `session` |
| [`/api/integrations/google/drive/import`](#apiintegrationsgoogledriveimport) | POST | `session` |
| [`/api/integrations/google/gmail/messages`](#apiintegrationsgooglegmailmessages) | GET | `session` |
| [`/api/integrations/google/gmail/send`](#apiintegrationsgooglegmailsend) | POST | `session` |
| [`/api/integrations/google/org`](#apiintegrationsgoogleorg) | GET | `admin` |
| [`/api/integrations/google/org`](#apiintegrationsgoogleorg) | PUT | `admin` |
| [`/api/integrations/google/org`](#apiintegrationsgoogleorg) | DELETE | `admin` |
| [`/api/integrations/google/org/callback`](#apiintegrationsgoogleorgcallback) | GET | `public` |
| [`/api/integrations/google/org/connect`](#apiintegrationsgoogleorgconnect) | GET | `session` |
| [`/api/integrations/google/org/health`](#apiintegrationsgoogleorghealth) | GET | `admin` |
| [`/api/integrations/google/org/provision`](#apiintegrationsgoogleorgprovision) | GET | `admin` |
| [`/api/integrations/google/org/provision`](#apiintegrationsgoogleorgprovision) | POST | `admin` |
| [`/api/integrations/google/pending`](#apiintegrationsgooglepending) | GET | `session` |
| [`/api/integrations/google/pending/{id}`](#apiintegrationsgooglependingid) | POST | `session` |

## `/api/integrations/google`

Source: [`ui/src/routes/api/integrations/google.ts`](../../ui/src/routes/api/integrations/google.ts)

> GET  → this user's Google connection status (never exposes tokens)
> DELETE → disconnect (revoke + forget)

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{available}` | 200 | — |
| DELETE | `session` | — | `{ok}` | 200 | — |

## `/api/integrations/google/agent/calendar`

Source: [`ui/src/routes/api/integrations/google.agent.calendar.ts`](../../ui/src/routes/api/integrations/google.agent.calendar.ts)

> Agent-facing calendar. A personal assistant acts as its owner; a general fleet
> agent acts on the shared ORG calendar.
> GET  → read upcoming events (free)
> POST → DRAFT an event; queued for approval (the owner, or an admin for org).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{error, message}` | 200, 409 | — |
| POST | `agent` | [body](#post-apiintegrationsgoogleagentcalendar-body) | `{pending, message}` | 200 + varies | — |

### POST `/api/integrations/google/agent/calendar` body

| field | schema | notes |
| :--- | :--- | :--- |
| `summary` | `z.string().min(1).max(500)` |  |
| `description` | `z.string().max(8000).optional()` |  |
| `location` | `z.string().max(500).optional()` |  |
| `start` | `z.string().min(4)` |  |
| `end` | `z.string().min(4)` |  |
| `allDay` | `z.boolean().optional()` |  |
| `attendees` | `z.array(z.string().email()).max(50).optional()` |  |

## `/api/integrations/google/agent/drive`

Source: [`ui/src/routes/api/integrations/google.agent.drive.ts`](../../ui/src/routes/api/integrations/google.agent.drive.ts)

> GET /api/integrations/google/agent/drive?q= → find files in the Drive the
> calling agent acts for (its owner's, or the shared org Drive). Read-only:
> finding and handing back a link. Creating files stays on
> export_to_google_doc; nothing here writes.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{error, message}` | 200, 409 | — |

## `/api/integrations/google/agent/gmail`

Source: [`ui/src/routes/api/integrations/google.agent.gmail.ts`](../../ui/src/routes/api/integrations/google.agent.gmail.ts)

> Agent-facing Gmail. A personal assistant acts as its owner; a general fleet
> agent acts on the shared ORG mailbox.
> GET  → read recent mail (free)
> POST → DRAFT an email; queued for approval (the owner, or an admin for org).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{error, message}` | 200, 409 | — |
| POST | `agent` | [body](#post-apiintegrationsgoogleagentgmail-body) | `{pending, message}` | 200 + varies | — |

### POST `/api/integrations/google/agent/gmail` body

| field | schema | notes |
| :--- | :--- | :--- |
| `to` | `z.string().min(3).max(500)` |  |
| `subject` | `z.string().max(500).default('')` |  |
| `body` | `z.string().max(50_000).default('')` |  |
| `cc` | `z.string().max(500).optional()` |  |
| `bcc` | `z.string().max(500).optional()` |  |

## `/api/integrations/google/agent/gmail/{id}`

Source: [`ui/src/routes/api/integrations/google.agent.gmail.$id.ts`](../../ui/src/routes/api/integrations/google.agent.gmail.$id.ts)

> GET /api/integrations/google/agent/gmail/$id → one FULL message (headers +
> plain-text body) for the calling agent. The listing tool hands out ids and
> snippets; this is the read an actual answer needs — summarize the thread,
> quote the question, draft the informed reply. Reads are free (confirm-sends
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{error, message}` | 200, 400, 409 | — |

## `/api/integrations/google/agent/gmail/labels`

Source: [`ui/src/routes/api/integrations/google.agent.gmail.labels.ts`](../../ui/src/routes/api/integrations/google.agent.gmail.labels.ts)

> The label half of inbox organizing. Gmail's folders ARE labels: INBOX and
> UNREAD are system labels a message carries, and "filing" mail means applying
> and removing them (see the organize route for the mutations).
> GET  → every label (read)
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{error, message}` | 200, 409 | — |
| POST | `agent` | [body](#post-apiintegrationsgoogleagentgmaillabels-body) | `{error, message}` | 200, 409 | — |

### POST `/api/integrations/google/agent/gmail/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(60)` |  |

## `/api/integrations/google/agent/gmail/organize`

Source: [`ui/src/routes/api/integrations/google.agent.gmail.organize.ts`](../../ui/src/routes/api/integrations/google.agent.gmail.organize.ts)

> POST → file/archive/read messages by label. THE HITL LINE, stated once
> because it is the one judgment this route makes: sends and invites leave the
> building under the owner's identity and queue for approval; filing, archiving
> and mark-read stay INSIDE the mailbox and are reversible, so they apply
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiintegrationsgoogleagentgmailorganize-body) | `{error, message}` | 200, 400, 409 | — |

### POST `/api/integrations/google/agent/gmail/organize` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `z.array(z.string().min(1).max(128)).min(1).max(100)` |  |
| `addLabels` | `z.array(z.string().min(1).max(120)).max(10).optional()` |  |
| `removeLabels` | `z.array(z.string().min(1).max(120)).max(10).optional()` |  |

## `/api/integrations/google/calendar/events`

Source: [`ui/src/routes/api/integrations/google.calendar.events.ts`](../../ui/src/routes/api/integrations/google.calendar.events.ts)

> GET  → upcoming events on the user's primary calendar
> POST → create an event

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{events}` | 200 | — |
| POST | `session` | [body](#post-apiintegrationsgooglecalendarevents-body) | `{event}` | 200 | — |

### POST `/api/integrations/google/calendar/events` body

| field | schema | notes |
| :--- | :--- | :--- |
| `summary` | `z.string().min(1).max(500)` |  |
| `description` | `z.string().max(8000).optional()` |  |
| `location` | `z.string().max(500).optional()` |  |
| `start` | `z.string().min(4)` |  |
| `end` | `z.string().min(4)` |  |
| `allDay` | `z.boolean().optional()` |  |
| `attendees` | `z.array(z.string().email()).max(50).optional()` |  |

## `/api/integrations/google/callback`

Source: [`ui/src/routes/api/integrations/google.callback.ts`](../../ui/src/routes/api/integrations/google.callback.ts)

> GET /api/integrations/google/callback → verify state, exchange the code for an
> offline refresh token, and store the connection for the signed-in user.
> The flow itself (gate, state, exchange, bounce-back) is the shared connect
> body; what THIS route adds is the meaning — the tokens are THIS user's, and
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | — | — |

## `/api/integrations/google/connect`

Source: [`ui/src/routes/api/integrations/google.connect.ts`](../../ui/src/routes/api/integrations/google.connect.ts)

> GET /api/integrations/google/connect → begin the offline-access consent dance.
> Requires an authenticated session (we bind the connection to this user).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 302, 400 | — |

## `/api/integrations/google/drive/files`

Source: [`ui/src/routes/api/integrations/google.drive.files.ts`](../../ui/src/routes/api/integrations/google.drive.files.ts)

> GET /api/integrations/google/drive/files?q= → browse/search the user's Drive.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{error, message}` | 200, 409, 502 | — |

## `/api/integrations/google/drive/import`

Source: [`ui/src/routes/api/integrations/google.drive.import.ts`](../../ui/src/routes/api/integrations/google.drive.import.ts)

> POST /api/integrations/google/drive/import { fileId } → pull a Drive file in
> as a new artifact owned by the caller (Doc→doc, Sheet→sheet, else→file).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiintegrationsgoogledriveimport-body) | `{artifact}` | 200, 409, 413, 502 | — |

### POST `/api/integrations/google/drive/import` body

| field | schema | notes |
| :--- | :--- | :--- |
| `fileId` | `z.string().min(1)` |  |

## `/api/integrations/google/gmail/messages`

Source: [`ui/src/routes/api/integrations/google.gmail.messages.ts`](../../ui/src/routes/api/integrations/google.gmail.messages.ts)

> GET /api/integrations/google/gmail/messages?q= → recent mail (metadata only).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{messages}` | 200 | — |

## `/api/integrations/google/gmail/send`

Source: [`ui/src/routes/api/integrations/google.gmail.send.ts`](../../ui/src/routes/api/integrations/google.gmail.send.ts)

> POST /api/integrations/google/gmail/send → send a plain-text email as the user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiintegrationsgooglegmailsend-body) | `{sent}` | 200 | — |

### POST `/api/integrations/google/gmail/send` body

| field | schema | notes |
| :--- | :--- | :--- |
| `to` | `z.string().min(3).max(500)` |  |
| `subject` | `z.string().max(500).default('')` |  |
| `body` | `z.string().max(50_000).default('')` |  |
| `cc` | `z.string().max(500).optional()` |  |
| `bcc` | `z.string().max(500).optional()` |  |

## `/api/integrations/google/org`

Source: [`ui/src/routes/api/integrations/google.org.ts`](../../ui/src/routes/api/integrations/google.org.ts)

> The shared org Google connection (admin-managed). General fleet agents act as
> this identity for Drive/Docs/Calendar/Gmail.
> GET → status + targets · PUT → save build targets · DELETE → disconnect

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{available}` | 200 | — |
| PUT | `admin` | [body](#put-apiintegrationsgoogleorg-body) | `{ok}` | 200 | — |
| DELETE | `admin` | — | `{ok}` | 200 | — |

### PUT `/api/integrations/google/org` body

| field | schema | notes |
| :--- | :--- | :--- |
| `driveFolderId` | `z.string().max(200).nullish()` |  |
| `calendarId` | `z.string().max(300).nullish()` |  |
| `sendAs` | `z.string().max(300).nullish()` |  |

## `/api/integrations/google/org/callback`

Source: [`ui/src/routes/api/integrations/google.org.callback.ts`](../../ui/src/routes/api/integrations/google.org.callback.ts)

> GET /api/integrations/google/org/callback → store the SHARED org connection.
> The shared connect body plus the org's two differences: only an admin may
> tie the org's containers to a Google account, and the landing page is the
> admin panel whose googleOrg flash reads the status param.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | — | — |

## `/api/integrations/google/org/connect`

Source: [`ui/src/routes/api/integrations/google.org.connect.ts`](../../ui/src/routes/api/integrations/google.org.connect.ts)

> GET /api/integrations/google/org/connect → admin begins connecting the shared
> org Google account (offline access). Wider scopes than the per-user flow:
> the org account is the one that provisions the shared calendar + Drive.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 302, 400, 403 | — |

## `/api/integrations/google/org/health`

Source: [`ui/src/routes/api/integrations/google.org.health.ts`](../../ui/src/routes/api/integrations/google.org.health.ts)

> GET /api/integrations/google/org/health → live probe of Drive / Calendar /
> Gmail with the org connection's token. Admin-only, and deliberately a
> separate route from the org status read: it makes three real Google calls
> and must only run when an admin asks for it, not on every panel load.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{checkedAt, results}` | 200 | — |

## `/api/integrations/google/org/provision`

Source: [`ui/src/routes/api/integrations/google.org.provision.ts`](../../ui/src/routes/api/integrations/google.org.provision.ts)

> The org workspace provisioning surface (admin). GET → what the panel draws:
> scope readiness, the provisioned container ids, and every agent's effective
> send address. POST → run the requested provisions, per-item outcomes back.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{calendarId, sharedDriveId, agents}` | 200 | — |
| POST | `admin` | [body](#post-apiintegrationsgoogleorgprovision-body) | `…` | 200, 400 | — |

### POST `/api/integrations/google/org/provision` body

| field | schema | notes |
| :--- | :--- | :--- |
| `calendar` | `z.boolean().optional()` |  |
| `drive` | `z.boolean().optional()` |  |

## `/api/integrations/google/pending`

Source: [`ui/src/routes/api/integrations/google.pending.ts`](../../ui/src/routes/api/integrations/google.pending.ts)

> GET /api/integrations/google/pending → the caller's agent-drafted actions
> awaiting their approval (send email / create event).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{pending}` | 200 | — |

## `/api/integrations/google/pending/{id}`

Source: [`ui/src/routes/api/integrations/google.pending.$id.ts`](../../ui/src/routes/api/integrations/google.pending.$id.ts)

> POST /api/integrations/google/pending/$id { decision } → approve (executes as
> the owner) or reject an agent-drafted action.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiintegrationsgooglependingid-body) | `{error, message}` | 200, 403, 404, 409, 502 + varies | — |

### POST `/api/integrations/google/pending/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `decision` | `z.enum(['approve', 'reject'])` |  |

