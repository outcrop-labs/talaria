# API reference — account

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

13 routes.

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
| [`/api/users`](#apiusers) | GET | `dual` |

## `/api/auth/claim`

Source: [`api/src/routes/auth_claim.rs`](../../api/src/routes/auth_claim.rs)

> POST /api/auth/claim { email, password, name? } → the FIRST admin.
>
> Offered only while the instance has zero admins (GET /api/auth/providers →
> claimable); claimAdmin's advisory lock closes the race, so a lost race is a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `public` | [body](#post-apiauthclaim-body) | `{ok, user}` | 200, 409, 429 | audit |

### POST `/api/auth/claim` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `z.preprocess((v) => (typeof v === 'string' ? v.trim().toLowerCase() : v), z.string().email().max(200),)` |  |
| `password` | `z.string().min(8).max(1000)` |  |
| `name` | `z.string().max(200).optional()` |  |

## `/api/auth/google`

Source: [`api/src/routes/auth_google.rs`](../../api/src/routes/auth_google.rs)

> GET /api/auth/google → begin the OAuth dance: set a signed state cookie and
> 302 to Google's consent screen.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200, 302, 400 | — |

## `/api/auth/google/callback`

Source: [`api/src/routes/auth_google_callback.rs`](../../api/src/routes/auth_google_callback.rs)

> GET /api/auth/google/callback → verify state, exchange the code, mint the
> session, and land on the cockpit.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200, 302 | audit |

## `/api/auth/logout`

Source: [`api/src/routes/auth_logout.rs`](../../api/src/routes/auth_logout.rs)

> POST /api/auth/logout → delete the Redis session + clear the cookie.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{ok}` | 200 | — |

## `/api/auth/password`

Source: [`api/src/routes/auth_password.rs`](../../api/src/routes/auth_password.rs)

> POST /api/auth/password { username, password } → sets the session cookie.
> Credentials live in user_password_credentials (Admin → People); the provider
> exists while any account does. No allow-list applies here — an account was
> admitted by an admin when it was created; login checks the stored hash only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `public` | [body](#post-apiauthpassword-body) | `{ok}` | 200, 400, 401, 429 | — |

### POST `/api/auth/password` body

| field | schema | notes |
| :--- | :--- | :--- |
| `username` | `z.string().min(1).max(200)` |  |
| `password` | `z.string().min(1).max(1000)` |  |

## `/api/auth/providers`

Source: [`api/src/routes/auth_providers.rs`](../../api/src/routes/auth_providers.rs)

> GET /api/auth/providers → the providers the login screen should render, and
> whether the instance is still UNCLAIMED. Everything is computed live:
>   • google — the Admin UI login toggle (or the AUTH_GOOGLE_ENABLED pin) AND
>     a resolvable client (Admin UI record or env);
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{claimable, configured}` | 200 | — |

## `/api/auth/session`

Source: [`api/src/routes/auth_session.rs`](../../api/src/routes/auth_session.rs)

> GET /api/auth/session → the current user + their denied views + effective
> permissions (read from the DB each time, so an admin's access change
> applies without re-login).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{deniedViews}` | 200 | — |

## `/api/join`

Source: [`api/src/routes/join.rs`](../../api/src/routes/join.rs)

> Public invite lookup for the /join page: token → who's invited, by whom,
> to which org. Rate-limited; expired/revoked/accepted tokens read as gone.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200, 400, 404, 429 | — |

## `/api/me`

Source: [`api/src/routes/me.rs`](../../api/src/routes/me.rs)

> The signed-in user's profile. GET → preferences (preferred model, preferred
> effort, timezone). PUT { name?, preferredModel?, preferredEffort?,
> timezone? } → update display name (users row + live session), the model
> powering their AI drafting (null clears → server default), their
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{preferredModel, preferredEffort, timezone}` | 200 | — |
| PUT | `session` | [body](#put-apime-body) | `{user}` | 200, 400, 403 | — |

### PUT `/api/me` body

Body schema `z.object({ name: z.string().min(1).max(80).optional(), preferredModel: z.string().min(1).max(200).nullable().optional(), preferredEffort: z…` is not an object literal in the route file — see the route source.

## `/api/me/assistant`

Source: [`api/src/routes/me_assistant.rs`](../../api/src/routes/me_assistant.rs)

> The signed-in user's personal assistant. GET → theirs (or null), with
> personality + live status. POST → create + start one, optionally named/
> personalized (idempotent: returns the existing one, re-enabling if retired).
> PATCH → owner-scoped rename / personality edit. Any signed-in user; every
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{assistant}` | 200 | — |
| POST | `session` | [body](#post-apimeassistant-body) | `…` | 200, 400 | — |
| PATCH | `session` | [body](#patch-apimeassistant-body) | `…` | 200, 400 | — |

### POST `/api/me/assistant` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `Name.optional()` |  |
| `handle` | `Handle.optional()` |  |
| `personality` | `z.string().max(4000).optional()` |  |

### PATCH `/api/me/assistant` body

Body schema `z.object({ name: Name.optional(), handle: Handle.optional(), personality: z.string().max(4000).optional(), model: z.string().trim().min(1).…` is not an object literal in the route file — see the route source.

## `/api/me/events`

Source: [`api/src/routes/me_events.rs`](../../api/src/routes/me_events.rs)

> GET /api/me/events → SSE stream of THIS person's own firehose: their runs
> changing state, their notifications landing, their brief being appended to.
>
> The per-device attach point. A run started on a laptop and parked on a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | SSE |

## `/api/me/mcp`

Source: [`api/src/routes/me_mcp.rs`](../../api/src/routes/me_mcp.rs)

> Connected accounts (Settings → Connections): per-user MCP servers and
> whether YOU have connected yours. PUT { serverId, headers } connects
> (headers sealed at rest — e.g. { Authorization: "Bearer <your token>" });
> headers null disconnects. Your assistant only carries a per-user server
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{servers}` | 200 | — |
| PUT | `session` | [body](#put-apimemcp-body) | `{ok}` | 200 | audit |

### PUT `/api/me/mcp` body

| field | schema | notes |
| :--- | :--- | :--- |
| `serverId` | `Uuid` |  |
| `headers` | `z.record(z.string(), z.string().max(4000)).nullable()` |  |

## `/api/users`

Source: [`api/src/routes/users.rs`](../../api/src/routes/users.rs)

> GET /api/users → everyone who has signed in (id, email, name). Powers the
> people pickers (board sharing, teams, channels). Any signed-in user — and
> agents (fleet key): they need the directory to resolve "email Priya" or
> "add Priya to the board" into an address.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{users}` | 200 | — |

