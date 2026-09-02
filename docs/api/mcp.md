# API reference — mcp

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

9 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/mcp`](#apimcp) | GET | `session` |
| [`/api/mcp/gw/{server}`](#apimcpgwserver) | GET | `agent` |
| [`/api/mcp/gw/{server}`](#apimcpgwserver) | POST | `agent` |
| [`/api/mcp/icon`](#apimcpicon) | GET | `session` |
| [`/api/mcp/library`](#apimcplibrary) | GET | `session` + `perm:agents.manage` |
| [`/api/mcp/oauth/callback`](#apimcpoauthcallback) | GET | `public` |
| [`/api/mcp/oauth/start`](#apimcpoauthstart) | GET | `session` + `perm:agents.manage` |
| [`/api/mcp/servers`](#apimcpservers) | GET | `session` + `perm:agents.manage` |
| [`/api/mcp/servers`](#apimcpservers) | POST | `session` + `perm:agents.manage` |
| [`/api/mcp/servers/{id}`](#apimcpserversid) | PUT | `session` + `perm:agents.manage` |
| [`/api/mcp/servers/{id}`](#apimcpserversid) | DELETE | `session` + `perm:agents.manage` |
| [`/api/mcp/test`](#apimcptest) | POST | `admin` |

## `/api/mcp`

Source: [`api/src/routes/mcp/mcp.rs`](../../api/src/routes/mcp/mcp.rs)

> /api/mcp.
> GET → MCP servers per agent: the agent's own config version PLUS the org
> registry's assignments (rendered in at deploy — marked 'managed' here so
> the agent UI reflects what the /mcp view attached). Non-admins get the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{name, url, timeout, extras}` | 200 | — |

## `/api/mcp/gw/{server}`

Source: [`api/src/routes/mcp/mcp_gw_server.rs`](../../api/src/routes/mcp/mcp_gw_server.rs)

> /api/mcp/gw/{server}.
> The MCP gateway — the registry's ENFORCEMENT point. Agents never see an
> upstream URL or credential: their configs point here, the agent's own
> credential identifies the caller (agent-auth), and the gateway
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `…` | 200, 403, 405, 502 | SSE |
| POST | `agent` | — | `…` | 200, 403, 502 | SSE |

## `/api/mcp/icon`

Source: [`api/src/routes/mcp/mcp_icon.rs`](../../api/src/routes/mcp/mcp_icon.rs)

> /api/mcp/icon.
> FALLBACK marketplace icons: the publisher's favicon, proxied + cached
> server-side (warmed in bulk when library pages are served). Registry-
> declared icons hotlink directly from the client and never come through
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 400, 404 | — |

## `/api/mcp/library`

Source: [`api/src/routes/mcp/mcp_library.rs`](../../api/src/routes/mcp/mcp_library.rs)

> /api/mcp/library.
> GET ?q= → the MCP server library (the official registry, live, filtered to
> remote-capable servers). Backs the Add-server picker. BOTH arms answer
> `{servers}` — featured shelf and search alike; the picker indexes
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{servers}` | 200 | — |

## `/api/mcp/oauth/callback`

Source: [`api/src/routes/mcp/mcp_oauth_callback.rs`](../../api/src/routes/mcp/mcp_oauth_callback.rs)

> /api/mcp/oauth/callback.
> The OAuth redirect target. No session requirement — identity was bound to
> the state row when the flow started; the state is single-use and expiring.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200 | audit |

## `/api/mcp/oauth/start`

Source: [`api/src/routes/mcp/mcp_oauth_start.rs`](../../api/src/routes/mcp/mcp_oauth_start.rs)

> /api/mcp/oauth/start.
> GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
> scope=org (one shared connection) needs agents.manage; scope=me connects
> the signed-in user's own account on a per-user server.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 302, 400, 403, 404 | — |

## `/api/mcp/servers`

Source: [`api/src/routes/mcp/mcp_servers.rs`](../../api/src/routes/mcp/mcp_servers.rs)

> /api/mcp/servers.
> The org MCP registry. GET → servers + their assignments + user access
> (admin/agents.manage view). POST → register a server. Every mutation
> re-renders the fleet so configs pick the change up (Hermes re-reads on
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{servers}` | 200 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apimcpservers-body) | `{server}` | 200, 400 | audit |

### POST `/api/mcp/servers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `slug()` | The zod body, checked in schema order. |
| `label` | `string?(120)` |  |
| `description` | `string? nullish(500)` |  |
| `url` | `url` |  |
| `headers` | `optional_headers` |  |
| `timeoutSecs` | `int? nullish (>0)` |  |
| `authMode` | `enum(org|per-user)?` |  |

## `/api/mcp/servers/{id}`

Source: [`api/src/routes/mcp/mcp_servers_id.rs`](../../api/src/routes/mcp/mcp_servers_id.rs)

> /api/mcp/servers/{id}.
> One registry server: PUT patches config / assignment / user access / tool
> refresh in one idempotent surface; DELETE unregisters (assignments, user
> access, and connected accounts cascade). Fleet re-renders after mutations.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apimcpserversid-body) | `…` | 200, 400, 404, 502 | audit |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200, 400, 404 | audit |

### PUT `/api/mcp/servers/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `label` | `string?(120)` |  |
| `description` | `string?(500)` |  |
| `url` | `url?` |  |
| `headers` | `optional_headers` |  |
| `timeoutSecs` | `number? nullable()` |  |
| `enabled` | `bool?` |  |
| `allAgents` | `bool?` |  |
| `authMode` | `enum(org|per-user)?` |  |
| `refreshTools` | `bool?` |  |
| `assign` | `assign` |  |
| `unassign` | `optional_string_member_max` |  |
| `userAccess` | `user_access` |  |
| `oauthClient` | `oauth_client` |  |

## `/api/mcp/test`

Source: [`api/src/routes/mcp/mcp_test.rs`](../../api/src/routes/mcp/mcp_test.rs)

> /api/mcp/test.
> POST → probe an MCP server's reachability + auth state (admin only; it
> makes an outbound request to an admin-supplied URL).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apimcptest-body) | `{state, detail}` | 200, 400 | — |

### POST `/api/mcp/test` body

| field | schema | notes |
| :--- | :--- | :--- |
| `url` | `url` |  |
| `agentSlug` | `string?(80)` |  |

