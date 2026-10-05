# API reference — account

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

17 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/auth/claim`](#apiauthclaim) | POST | `public` |
| [`/api/auth/google`](#apiauthgoogle) | GET | `public` |
| [`/api/auth/google/callback`](#apiauthgooglecallback) | GET | `public` |
| [`/api/auth/logout`](#apiauthlogout) | POST | `session` |
| [`/api/auth/password`](#apiauthpassword) | POST | `public` |
| [`/api/auth/providers`](#apiauthproviders) | GET | `public` |
| [`/api/auth/session`](#apiauthsession) | GET | `session` |
| [`/api/join`](#apijoin) | GET | `public` |
| [`/api/me`](#apime) | GET | `session` |
| [`/api/me`](#apime) | PUT | `session` |
| [`/api/me/assistant`](#apimeassistant) | GET | `session` |
| [`/api/me/assistant`](#apimeassistant) | POST | `session` |
| [`/api/me/assistant`](#apimeassistant) | PATCH | `session` |
| [`/api/me/events`](#apimeevents) | GET | `session` |
| [`/api/me/mcp`](#apimemcp) | GET | `session` |
| [`/api/me/mcp`](#apimemcp) | PUT | `session` |
| [`/api/me/presence`](#apimepresence) | PUT | `session` |
| [`/api/me/sent`](#apimesent) | GET | `session` |
| [`/api/me/threads`](#apimethreads) | GET | `session` |
| [`/api/users`](#apiusers) | GET | `dual` |
| [`/api/users/{id}/avatar`](#apiusersidavatar) | GET | `dual` |

## `/api/auth/claim`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_claim.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_claim.rs)

> POST /api/auth/claim { email, password, name? }. The FIRST admin. Offered
> only while the instance has zero admins (GET /api/auth/providers →
> claimable); claim's advisory lock closes the race, so a lost race is a 409,
> never a second admin. Reachable by whoever gets there first on a fresh
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `public` | [body](#post-apiauthclaim-body) | `{email, role, provider}` | 200, 400, 409, 429 | audit |

### POST `/api/auth/claim` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |
| `password` | `string(8, 1000)` |  |
| `name` | `string?(200)` |  |

## `/api/auth/google`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_google.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_google.rs)

> GET /api/auth/google. Begin the OAuth dance: set the one-shot state cookie
> and 302 to Google's consent screen. The state cookie is double-submit CSRF
> proof — random in, random back, compared constant-time at the callback.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 302, 400 | — |

## `/api/auth/google/callback`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_google_callback.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_google_callback.rs)

> GET /api/auth/google/callback. Verify the state cookie, exchange the code,
> mint the session, and land on the cockpit. Every failure bounces to /login
> with a machine-readable reason (the SPA renders each), extra params riding
> along where a door can name what to change.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{email, role, provider}` | 200, 302 | audit |

## `/api/auth/logout`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_logout.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_logout.rs)

> POST /api/auth/logout. Delete the Redis session and clear the cookie.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `…` | 200 | — |

## `/api/auth/password`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_password.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_password.rs)

> POST /api/auth/password { username, password }. Sets the session cookie.
> Credentials live in user_password_credentials (Admin → People); the provider
> exists while any account does. No allow-list applies here — an account was
> admitted by an admin when it was created; login checks the stored hash only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `public` | [body](#post-apiauthpassword-body) | `…` | 200, 400, 401, 429 | — |

### POST `/api/auth/password` body

| field | schema | notes |
| :--- | :--- | :--- |
| `username` | `string(1, 200)` |  |
| `password` | `string(1, 1000)` |  |

## `/api/auth/providers`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_providers.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_providers.rs)

> GET /api/auth/providers. The login screen asks one question: what doors
> exist on this instance? Answered live on every call, so flipping a toggle
> or creating the first password account changes the screen without a
> restart:
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200 | — |

## `/api/auth/session`

Source: [`api/crates/talaria-routes-integrations/src/account/auth_session.rs`](../../api/crates/talaria-routes-integrations/src/account/auth_session.rs)

> GET /api/auth/session. The current user + their denied views + effective
> permissions, read from the DB each time so an admin's access change applies
> without re-login. The user's face — effective picture, status emoji and
> text — is read fresh from the users row the same way, so a photo or status
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/join`

Source: [`api/crates/talaria-routes-integrations/src/account/join.rs`](../../api/crates/talaria-routes-integrations/src/account/join.rs)

> /api/join.
>
> Public invite lookup for the /join page: token → who's invited, by whom, to
> which org. Expired/revoked/accepted tokens read as gone.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{invite}` | 200, 400, 404, 429 | — |

## `/api/me`

Source: [`api/crates/talaria-routes-integrations/src/account/me.rs`](../../api/crates/talaria-routes-integrations/src/account/me.rs)

> /api/me. The signed-in person's own profile: GET reads the three preference
> columns, PUT edits display name (users row + the live session, so the SPA's
> corner never waits for a re-login), preferred model (the member gate runs
> HERE, not just in the picker), platform-default reasoning effort, IANA
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{preferredModel, preferredEffort, timezone}` | 200 | — |
| PUT | `session` | [body](#put-apime-body) | `{user}` | 200, 400, 403, 404 | — |

### PUT `/api/me` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(80)` |  |
| `preferredModel` | `string? nullable` |  |
| `preferredEffort` | `string? nullable` |  |
| `timezone` | `string? nullable` |  |
| `avatarUploadId` | `uuid? nullable` |  |
| `statusEmoji` | `string? nullable` |  |
| `statusText` | `string? nullable(100)` |  |

## `/api/me/assistant`

Source: [`api/crates/talaria-routes-integrations/src/account/me_assistant.rs`](../../api/crates/talaria-routes-integrations/src/account/me_assistant.rs)

> /api/me/assistant. The signed-in user's personal assistant. GET → theirs
> (or null), with personality + live status. POST → create + start one,
> optionally named/personalized (idempotent: returns the existing one,
> re-enabling if retired). PATCH → owner-scoped rename / personality edit.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{assistant}` | 200 | — |
| POST | `session` | [body](#post-apimeassistant-body) | `{assistant}` | 200, 400 | — |
| PATCH | `session` | [body](#patch-apimeassistant-body) | `{assistant}` | 200, 400 | — |

### POST `/api/me/assistant` body

| field | schema | notes |
| :--- | :--- | :--- |
| `personality` | `string?(4000)` |  |

### PATCH `/api/me/assistant` body

| field | schema | notes |
| :--- | :--- | :--- |
| `personality` | `string?(4000)` |  |

## `/api/me/events`

Source: [`api/crates/talaria-routes-integrations/src/account/me_events.rs`](../../api/crates/talaria-routes-integrations/src/account/me_events.rs)

> GET /api/me/events → SSE stream of THIS person's own firehose: their runs
> changing state, their notifications landing, their brief being appended to.
>
> The per-device attach point. A run started on a laptop and parked on a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | SSE |

## `/api/me/mcp`

Source: [`api/crates/talaria-routes-integrations/src/account/me_mcp.rs`](../../api/crates/talaria-routes-integrations/src/account/me_mcp.rs)

> /api/me/mcp. Connected accounts (Settings → Connections): per-user MCP
> servers and whether YOU have connected yours. PUT { serverId, headers }
> connects (headers sealed at rest — e.g. { Authorization: "Bearer <your
> token>" }); headers null disconnects. Your assistant only carries a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{servers}` | 200 | — |
| PUT | `session` | [body](#put-apimemcp-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/me/mcp` body

| field | schema | notes |
| :--- | :--- | :--- |
| `serverId` | `uuid` |  |

## `/api/me/presence`

Source: [`api/crates/talaria-routes-integrations/src/account/me_presence.rs`](../../api/crates/talaria-routes-integrations/src/account/me_presence.rs)

> PUT /api/me/presence. The signed-in person's heartbeat: the client calls it
> every 30s while a Talaria tab is visible, and it sets
> `user:presence:{id}` with a 90s TTL. `GET /api/users` reads those keys to
> mark people online. Answers { ok: true }.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` | — | `{ok}` | 200 | — |

## `/api/me/sent`

Source: [`api/crates/talaria-routes-comms/src/comms/me_sent.rs`](../../api/crates/talaria-routes-comms/src/comms/me_sent.rs)

> GET /api/me/sent. The Comms sidebar's "Drafts & sent" view, sent half (drafts
> live in the browser): the viewer's own latest 50 messages, newest first —
> channel and DM messages in channels they are still in, and their turns in
> their agent DMs. Never anyone else's message. Answers
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{messages}` | 200 | — |

## `/api/me/threads`

Source: [`api/crates/talaria-routes-comms/src/comms/me_threads.rs`](../../api/crates/talaria-routes-comms/src/comms/me_threads.rs)

> GET /api/me/threads. The Comms sidebar's Threads view: every thread root
> with at least one reply, in a channel the viewer is in right now, that the
> viewer started or replied in — newest reply first, at most 50. Answers
> { threads: [{ channelId, channelName, channelKind, root, lastAt }] }, where
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{threads}` | 200 | — |

## `/api/users`

Source: [`api/crates/talaria-routes-integrations/src/account/users.rs`](../../api/crates/talaria-routes-integrations/src/account/users.rs)

> GET /api/users. Everyone who has signed in, for the people pickers and the
> Comms sidebar: id, email, name, the effective picture (an uploaded photo,
> else the sign-in provider's), status emoji + text, and `online` — whether
> they pinged `PUT /api/me/presence` within the last 90s. Any signed-in user —
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200 | — |

## `/api/users/{id}/avatar`

Source: [`api/crates/talaria-routes-integrations/src/account/users_id_avatar.rs`](../../api/crates/talaria-routes-integrations/src/account/users_id_avatar.rs)

> GET /api/users/{id}/avatar. A person's uploaded profile photo, for any
> signed-in member of the org and for agents (their tak_ key or the fleet
> key) — the one place an upload is readable outside the conversation-scoped
> `can_access_upload`. It is narrow on purpose: the upload id comes from the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | — | — |

