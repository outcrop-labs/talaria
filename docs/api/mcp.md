# API reference — mcp

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

9 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/mcp`](#apimcp) | GET | `session` |
| [`/api/mcp/gw/{server}`](#apimcpgwserver) | POST | `agent` |
| [`/api/mcp/gw/{server}`](#apimcpgwserver) | GET | `agent` |
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

> MCP servers per agent: the agent's own config version PLUS the org
> registry's assignments (rendered in at deploy — marked 'managed' here so
> the agent UI reflects what the /mcp view attached). Non-admins get the
> roster + server NAMES only — internal URLs and tool filters map the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{agents}` | 200 | — |

## `/api/mcp/gw/{server}`

Source: [`ui/src/routes/api/mcp.gw.$server.ts`](../../ui/src/routes/api/mcp.gw.$server.ts)

> The MCP gateway — the registry's ENFORCEMENT point. Agents never see an
> upstream URL or credential: their configs point here, the agent's own
> credential identifies the caller (agent-auth), and the gateway
>   · forwards JSON-RPC to the upstream (org headers, or the acting user's
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | — | `…` | 200, 403, 502 + varies | SSE |
| GET | `agent` | — | `…` | 200, 403, 405, 502 + varies | SSE |

## `/api/mcp/icon`

Source: [`api/src/routes/mcp/mcp_icon.rs`](../../api/src/routes/mcp/mcp_icon.rs)

> FALLBACK marketplace icons: the publisher's favicon, proxied + cached
> server-side (warmed in bulk when library pages are served). Registry-
> declared icons hotlink directly from the client and never come through here.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 400, 404 | — |

## `/api/mcp/library`

Source: [`api/src/routes/mcp/mcp_library.rs`](../../api/src/routes/mcp/mcp_library.rs)

> GET ?q= → the MCP server library (the official registry, live, filtered to
> remote-capable servers). Backs the Add-server picker.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{servers}` | 200, 502 | — |

## `/api/mcp/oauth/callback`

Source: [`api/src/routes/mcp/mcp_oauth_callback.rs`](../../api/src/routes/mcp/mcp_oauth_callback.rs)

> The OAuth redirect target. No session requirement — identity was bound to
> the state row when the flow started; the state is single-use and expiring.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | — | audit |

## `/api/mcp/oauth/start`

Source: [`api/src/routes/mcp/mcp_oauth_start.rs`](../../api/src/routes/mcp/mcp_oauth_start.rs)

> GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
> scope=org (one shared connection) needs agents.manage; scope=me connects
> the signed-in user's own account on a per-user server.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200, 302, 400, 403, 404 | — |

## `/api/mcp/servers`

Source: [`api/src/routes/mcp/mcp_servers.rs`](../../api/src/routes/mcp/mcp_servers.rs)

> The org MCP registry. GET → servers + their assignments + user access
> (admin/agents.manage view). POST → register a server. Every mutation
> re-renders the fleet so configs pick the change up (Hermes re-reads on
> mtime — no restarts).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{servers}` | 200 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apimcpservers-body) | `{server}` | 200, 400 | audit |

### POST `/api/mcp/servers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase slug').max(60)` |  |
| `label` | `z.string().max(120).optional()` |  |
| `description` | `z.string().max(500).nullish()` |  |
| `url` | `z.string().url().max(500)` |  |
| `headers` | `z.record(z.string(), z.string().max(2000)).optional()` |  |
| `timeoutSecs` | `z.number().int().positive().max(3600).nullish()` |  |
| `authMode` | `z.enum(['org', 'per-user']).optional()` |  |
| `requiredHeaders` | `z.array(z.object({ name: z.string().max(120), description: z.string().max(500).nullish(), isSecret: z.boolean().optional(), placeholder: z.…` |  |

## `/api/mcp/servers/{id}`

Source: [`api/src/routes/mcp/mcp_servers_id.rs`](../../api/src/routes/mcp/mcp_servers_id.rs)

> One registry server: PUT patches config / assignment / user access / tool
> refresh in one idempotent surface; DELETE unregisters (assignments, user
> access, and connected accounts cascade). Fleet re-renders after mutations.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apimcpserversid-body) | `{ok}` | 200, 400, 404, 502 | audit |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200, 400, 404 | audit |

### PUT `/api/mcp/servers/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `label` | `z.string().max(120).optional()` |  |
| `description` | `z.string().max(500).nullish()` |  |
| `url` | `z.string().url().max(500).optional()` |  |
| `headers` | `z.record(z.string(), z.string().max(2000)).optional()` |  |
| `timeoutSecs` | `z.number().int().positive().max(3600).nullish()` |  |
| `enabled` | `z.boolean().optional()` |  |
| `allAgents` | `z.boolean().optional()` |  |
| `authMode` | `z.enum(['org', 'per-user']).optional()` |  |
| `refreshTools` | `z.boolean().optional()` |  |
| `assign` | `z.object({ agentModel: z.string().min(1).max(200), tools: z.array(z.string().max(120)).nullable() }).optional()` |  |
| `unassign` | `z.string().min(1).max(200).optional()` |  |
| `userAccess` | `z.object({ userId: Uuid, allowed: z.boolean().nullable(), tools: z.array(z.string().max(120)).nullable() }).optional()` |  |
| `oauthClient` | `z.object({ clientId: z.string().min(1).max(200), clientSecret: z.string().max(500).nullable() }).optional()` |  |

## `/api/mcp/test`

Source: [`api/src/routes/mcp/mcp_test.rs`](../../api/src/routes/mcp/mcp_test.rs)

> POST → probe an MCP server's reachability + auth state (admin only; it makes
> an outbound request to an admin-supplied URL).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apimcptest-body) | `…` | 200 | — |

### POST `/api/mcp/test` body

| field | schema | notes |
| :--- | :--- | :--- |
| `url` | `z.string().url().max(300)` |  |
| `agentSlug` | `z.string().max(80).optional()` |  |

