# API reference — admin

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

22 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/admin/apps`](#apiadminapps) | GET | `session` + `view:/apps` |
| [`/api/admin/apps`](#apiadminapps) | PUT | `admin` |
| [`/api/admin/apps`](#apiadminapps) | POST | `admin` |
| [`/api/admin/apps`](#apiadminapps) | DELETE | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | GET | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | POST | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | DELETE | `admin` |
| [`/api/admin/email`](#apiadminemail) | GET | `admin` |
| [`/api/admin/email`](#apiadminemail) | PUT | `admin` |
| [`/api/admin/email`](#apiadminemail) | POST | `admin` |
| [`/api/admin/encryption`](#apiadminencryption) | GET | `admin` |
| [`/api/admin/encryption`](#apiadminencryption) | POST | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | GET | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | PUT | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | DELETE | `admin` |
| [`/api/admin/guardrails`](#apiadminguardrails) | GET | `admin` |
| [`/api/admin/guardrails`](#apiadminguardrails) | PUT | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | GET | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | PUT | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | POST | `admin` |
| [`/api/admin/invites`](#apiadmininvites) | GET | `admin` |
| [`/api/admin/invites`](#apiadmininvites) | POST | `admin` |
| [`/api/admin/invites`](#apiadmininvites) | DELETE | `admin` |
| [`/api/admin/judge`](#apiadminjudge) | GET | `admin` |
| [`/api/admin/judge`](#apiadminjudge) | PUT | `admin` |
| [`/api/admin/model-fitness`](#apiadminmodel-fitness) | GET | `admin` |
| [`/api/admin/model-fitness`](#apiadminmodel-fitness) | POST | `admin` |
| [`/api/admin/model-roles`](#apiadminmodel-roles) | GET | `admin` |
| [`/api/admin/outreach`](#apiadminoutreach) | GET | `admin` |
| [`/api/admin/outreach`](#apiadminoutreach) | PUT | `admin` |
| [`/api/admin/permissions`](#apiadminpermissions) | GET | `admin` |
| [`/api/admin/permissions`](#apiadminpermissions) | PUT | `admin` |
| [`/api/admin/platform-agents`](#apiadminplatform-agents) | GET | `admin` |
| [`/api/admin/rag`](#apiadminrag) | GET | `admin` |
| [`/api/admin/rag`](#apiadminrag) | PUT | `admin` |
| [`/api/admin/rag`](#apiadminrag) | POST | `admin` |
| [`/api/admin/search`](#apiadminsearch) | GET | `admin` |
| [`/api/admin/search`](#apiadminsearch) | PUT | `admin` |
| [`/api/admin/secrets`](#apiadminsecrets) | GET | `admin` |
| [`/api/admin/secrets`](#apiadminsecrets) | DELETE | `admin` |
| [`/api/admin/settings`](#apiadminsettings) | GET | `admin` |
| [`/api/admin/settings`](#apiadminsettings) | PUT | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | GET | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | PUT | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | POST | `admin` |
| [`/api/admin/update`](#apiadminupdate) | GET | `admin` |
| [`/api/admin/update`](#apiadminupdate) | POST | `admin` |
| [`/api/admin/update`](#apiadminupdate) | PUT | `admin` |
| [`/api/admin/users`](#apiadminusers) | GET | `admin` |
| [`/api/admin/users`](#apiadminusers) | PUT | `admin` |
| [`/api/admin/workspace-secrets`](#apiadminworkspace-secrets) | GET | `admin` |

## `/api/admin/apps`

Source: [`ui/src/routes/api/admin.apps.ts`](../../ui/src/routes/api/admin.apps.ts)

> App administration. GET → installed apps (+ ?catalog=1 for the marketplace
> feed). Reads are open to anyone granted the /apps Manage view; mutations
> (enable/disable, install, uninstall, catalog source) stay admin-only —
> installing an app adds CODE to the deployment.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/apps` | — | `{catalogUrl}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminapps-body) | `{ok}` | 200, 400 | audit |
| POST | `admin` | [body](#post-apiadminapps-body) | `…` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadminapps-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/apps` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `app` | `z.string().min(1)` |  |
| `enabled` | `z.boolean()` |  |

### PUT `/api/admin/apps` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `catalogUrl` | `z.string().url().nullable()` |  |

### POST `/api/admin/apps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `installUrl` | `z.string().url()` |  |
| `slug` | `z.string().min(1).max(64).optional()` |  |

### DELETE `/api/admin/apps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `app` | `z.string().min(1)` |  |
| `wipeData` | `z.boolean().optional()` |  |

## `/api/admin/domains`

Source: [`ui/src/routes/api/admin.domains.ts`](../../ui/src/routes/api/admin.domains.ts)

> Sign-up domains. GET → the list. POST { domain } → add (returns the TXT
> token to publish). POST { verifyId } → run the DNS check. DELETE { id } →
> remove (self-joins from it stop immediately). Admins only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{domains}` | 200 | — |
| POST | `admin` | [body](#post-apiadmindomains-body) | `…` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadmindomains-body) | `{ok}` | 200 | audit |

### POST `/api/admin/domains` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `domain` | `z.string().min(3).max(253)` |  |

### POST `/api/admin/domains` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `verifyId` | `Uuid` |  |

### DELETE `/api/admin/domains` body

Body schema `IdBody` is not an object literal in the route file — see the route source.

## `/api/admin/email`

Source: [`ui/src/routes/api/admin.email.ts`](../../ui/src/routes/api/admin.email.ts)

> Transactional email config. GET → config with secrets MASKED (set-flags
> only). POST → patch config; { test: true } → send a test to the caller.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminemail-body) | `{ok}` | 200 | audit |
| POST | `admin` | [body](#post-apiadminemail-body) | `{ok}` | 200, 400, 502 | — |

### PUT `/api/admin/email` body

| field | schema | notes |
| :--- | :--- | :--- |
| `provider` | `z.enum(['smtp', 'resend']).nullable().optional()` |  |
| `from` | `z.string().max(200).optional()` |  |
| `smtp` | `z.object({ host: z.string().max(200).optional(), port: z.number().int().min(1).max(65535).optional(), secure: z.boolean().optional(), user:…` |  |
| `resend` | `z.object({ apiKey: z.string().max(200).nullable().optional() }).optional()` |  |

### POST `/api/admin/email` body

| field | schema | notes |
| :--- | :--- | :--- |
| `test` | `z.literal(true)` |  |

## `/api/admin/encryption`

Source: [`ui/src/routes/api/admin.encryption.ts`](../../ui/src/routes/api/admin.encryption.ts)

> Encryption status + one-click key rotation. Rotating re-generates the data key
> and re-encrypts every stored secret (provider keys, agent secrets, OAuth
> tokens) in a single pass — one action, no per-secret steps.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{keyVersion, rotatedAt, secretCount, algorithm}` | 200 | — |
| POST | `admin` | [body](#post-apiadminencryption-body) | `{ok}` | 200, 500 | audit |

### POST `/api/admin/encryption` body

Body schema `z.object({ newRootSecret: z.string().min(16, 'new root secret must be at least 16 chars').max(400).optional() }).nullable()` is not an object literal in the route file — see the route source.

## `/api/admin/google-client`

Source: [`ui/src/routes/api/admin.google-client.ts`](../../ui/src/routes/api/admin.google-client.ts)

> The Google OAuth client — the credential the whole Google integration (login
> + workspace connect) runs on. Admins register it here instead of editing
> ui/.env; the secret is SEALED and never read back. Deliberately requireAdmin:
> this is an org credential, not a grantable surface.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{status, loginEnabled, redirectUris}` | 200 + varies | — |
| PUT | `admin` | [body](#put-apiadmingoogle-client-body) | `{status, loginEnabled}` | 200 + varies | audit |
| DELETE | `admin` | — | `{status, loginEnabled}` | 200 + varies | audit |

### PUT `/api/admin/google-client` body

| field | schema | notes |
| :--- | :--- | :--- |
| `clientId` | `z.string().min(1).max(200)` |  |

## `/api/admin/guardrails`

Source: [`ui/src/routes/api/admin.guardrails.ts`](../../ui/src/routes/api/admin.guardrails.ts)

> Confab guardrail config + observability (admin). GET → config + stats + recent
> findings. PUT → update config.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{rules}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminguardrails-body) | `{config}` | 200 | audit |

### PUT `/api/admin/guardrails` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `z.enum(['off', 'observe', 'annotate', 'strict'])` |  |
| `checks` | `z.record(z.string(), z.boolean())` |  |
| `minConfidence` | `z.number().min(0).max(1)` |  |
| `policedHosts` | `z.array(z.string().max(200)).max(100)` |  |
| `coach` | `z.boolean().default(false)` |  |

## `/api/admin/instance`

Source: [`ui/src/routes/api/admin.instance.ts`](../../ui/src/routes/api/admin.instance.ts)

> The instance's hosting domain. GET → current config. PUT { domain } → set
> (unverified until the round trip passes); { domain: null } clears.
> POST { verify: true } → run the self-fetch (the action). Admins only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{instance}` | 200 | — |
| PUT | `admin` | [body](#put-apiadmininstance-body) | `…` | 200, 400 | audit |
| POST | `admin` | [body](#post-apiadmininstance-body) | `…` | 200 | audit |

### PUT `/api/admin/instance` body

| field | schema | notes |
| :--- | :--- | :--- |
| `domain` | `z.string().min(3).max(253).nullable()` |  |

### POST `/api/admin/instance` body

| field | schema | notes |
| :--- | :--- | :--- |
| `verify` | `z.literal(true)` |  |

## `/api/admin/invites`

Source: [`ui/src/routes/api/admin.invites.ts`](../../ui/src/routes/api/admin.invites.ts)

> Invites. GET → recent invites with state. POST { email } → create + send
> (re-invites re-issue with a fresh token). DELETE { id } → revoke.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{invites}` | 200 | — |
| POST | `admin` | [body](#post-apiadmininvites-body) | `…` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadmininvites-body) | `{ok}` | 200 | audit |

### POST `/api/admin/invites` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `z.string().max(200)` |  |

### DELETE `/api/admin/invites` body

Body schema `IdBody` is not an object literal in the route file — see the route source.

## `/api/admin/judge`

Source: [`ui/src/routes/api/admin.judge.ts`](../../ui/src/routes/api/admin.judge.ts)

> The automated QA judge config (admin). GET → current + available models.
> PUT → enable/disable + pick the judge model.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{models}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminjudge-body) | `…` | 200 | audit |

### PUT `/api/admin/judge` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `z.boolean()` |  |
| `model` | `z.string().max(200).nullish()` |  |
| `mode` | `z.enum(['advisory', 'enforcing']).optional()` |  |

## `/api/admin/model-fitness`

Source: [`ui/src/routes/api/admin.model-fitness.ts`](../../ui/src/routes/api/admin.model-fitness.ts)

> GET  ?view=matrix (default) → slots + models + capability facts + cells + runs
>      ?view=capabilities     → models + facts only (the model pickers)
>      ?view=detail&model=    → one archived report + production telemetry
>      ?view=value            → price against performance, over the measured workload
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200, 400 | — |
| POST | `admin` | [body](#post-apiadminmodel-fitness-body) | `{models, report}` | 200, 400, 409 + varies | audit |

### POST `/api/admin/model-fitness` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('start')` |  |
| `model` | `z.string().min(1).max(200)` |  |
| `tiers` | `z.array(z.enum(['probes', 'evals', 'adversarial'])).min(1)` |  |
| `adversaryModel` | `z.string().max(200).nullish()` |  |
| `only` | `z.array(z.string().max(120)).max(64).optional()` |  |
| `restart` | `z.boolean().optional()` |  |

### POST `/api/admin/model-fitness` body — variant 2

Schema not a literal object in the route file (// A model stops ONE run; omitted stops every run in flight. z.object({ action: z.literal('stop'), model: z.string().max(200).nullish() })) — see the route source.

### POST `/api/admin/model-fitness` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('forget')` |  |
| `model` | `z.string().min(1).max(200)` |  |

### POST `/api/admin/model-fitness` body — variant 4

Schema not a literal object in the route file (// CLEAR is not FORGET. Forget drops what a model CAN DO (probe facts, paid // for once and true until the id is re-pointed); this drops wh…) — see the route source.

### POST `/api/admin/model-fitness` body — variant 5

Schema not a literal object in the route file (// so a candidate can be swept again from nothing. `model: null` clears every // tested candidate. z.object({ action: z.literal('clear'), m…) — see the route source.

## `/api/admin/model-roles`

Source: [`ui/src/routes/api/admin.model-roles.ts`](../../ui/src/routes/api/admin.model-roles.ts)

> Model Roles — which model handles each activity class. GET → the catalog of
> roles + current assignments + assignable models + fitness issues. PUT
> { role, model|null } → assign (null = back to auto). Admins only.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | [body](#get-apiadminmodel-roles-body) | `{roles, assignments, models, issues}` | 200, 400 | audit |

### GET `/api/admin/model-roles` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `z.enum(ROLES as [string,...string[]])` |  |
| `model` | `z.string().max(200).nullable().optional()` |  |

## `/api/admin/outreach`

Source: [`ui/src/routes/api/admin.outreach.ts`](../../ui/src/routes/api/admin.outreach.ts)

> GET → config + per-agent proactive flags + recent events. PUT → save both.
> Admin-only; the sweep itself stays off unless `enabled`.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config, events}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminoutreach-body) | `{ok}` | 200 | audit |

### PUT `/api/admin/outreach` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `z.boolean()` |  |
| `intervalMinutes` | `z.number().int().min(15).max(24 * 60)` |  |
| `dailyDmCap` | `z.number().int().min(1).max(20)` |  |

## `/api/admin/permissions`

Source: [`ui/src/routes/api/admin.permissions.ts`](../../ui/src/routes/api/admin.permissions.ts)

> Fine-grained permissions admin. GET → the catalog + org member defaults +
> every user's overrides. PUT { userId, perm, allowed|null } → set/clear a
> per-user override (null = back to the org default). PUT { orgDefault:
> { perm, enabled|null } } → tune what plain members can do out of the box
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{catalog, orgDefaults}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminpermissions-body) | `{orgDefaults}` | 200 | audit |

### PUT `/api/admin/permissions` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |
| `perm` | `z.enum(PERM_IDS as [Perm,...Perm[]])` |  |
| `allowed` | `z.boolean().nullable()` |  |

### PUT `/api/admin/permissions` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `orgDefault` | `z.object({ perm: z.enum(PERM_IDS as [Perm,...Perm[]]), enabled: z.boolean().nullable() })` |  |

## `/api/admin/platform-agents`

Source: [`ui/src/routes/api/admin.platform-agents.ts`](../../ui/src/routes/api/admin.platform-agents.ts)

> Platform sub-agents — Talaria's own workers (Muse, Distiller, Concluder, )
> and which model powers each. GET → registry + assignments + assignable
> models. PUT { id, model|null } → assign (null = back to auto). The Judge's
> pick lives in its own judge_config (shared with the Guard panel) — this
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | [body](#get-apiadminplatform-agents-body) | `{agents, models}` | 200, 400 | audit |

### GET `/api/admin/platform-agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `z.enum(IDS as [PlatformAgentId,...PlatformAgentId[]])` |  |
| `model` | `z.string().max(200).nullable().optional()` |  |

## `/api/admin/rag`

Source: [`ui/src/routes/api/admin.rag.ts`](../../ui/src/routes/api/admin.rag.ts)

> Admin → Retrieval. GET → services health + backfill status + reranker
> providers/config + KB-space brain bindings. PUT → reranker config and/or a
> space↔brain binding. POST → kick a full backfill (detached), or
> { models, key? } → live model catalog for the picker.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{health, backfill, upgrade, reindex, rerank}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminrag-body) | `{rerank}` | 200 | audit |
| POST | `admin` | [body](#post-apiadminrag-body) | `{models}` | 200 | audit |

### PUT `/api/admin/rag` body

| field | schema | notes |
| :--- | :--- | :--- |
| `reranker` | `z.object({ provider: z.enum(PROVIDER_IDS).optional(), url: z.string().max(500).nullish(), model: z.string().max(200).nullish(), apiKey: z.s…` |  |

### POST `/api/admin/rag` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['reindex', 'backfill'])` |  |

### POST `/api/admin/rag` body — variant 2

Schema not a literal object in the route file (// Model catalog for the picker. The candidate API key travels in a POST // body — NEVER a query string) — see the route source.

### POST `/api/admin/rag` body — variant 3

Schema not a literal object in the route file (where it would land in access/proxy logs. z.object({ models: z.string().min(1).max(40), key: z.string().max(500).nullish() })) — see the route source.

## `/api/admin/search`

Source: [`ui/src/routes/api/admin.search.ts`](../../ui/src/routes/api/admin.search.ts)

> LIVE WEB SEARCH — where it points, and whether it is actually answering.
>
> WHY THE REACHABILITY CHECK IS AN ADMIN SURFACE AND NOT A LOG LINE. When search
> is down, the way an operator currently finds out is that an agent tells
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{url, reachable, error}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsearch-body) | `{url, reachable, error}` | 200, 400 | audit |

### PUT `/api/admin/search` body

| field | schema | notes |
| :--- | :--- | :--- |

## `/api/admin/secrets`

Source: [`ui/src/routes/api/admin.secrets.ts`](../../ui/src/routes/api/admin.secrets.ts)

> The secrets inventory. GET is a VIEW over the stores that own each value —
> presence, provenance and readability, never the value itself. DELETE clears
> one row's ciphertext, or every row that cannot be read.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200 | — |
| DELETE | `admin` | [body](#delete-apiadminsecrets-body) | `{ok}` | 200, 404, 500 | audit |

### DELETE `/api/admin/secrets` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `z.string().min(1).max(200)` |  |

### DELETE `/api/admin/secrets` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `unreadable` | `z.literal(true)` |  |

## `/api/admin/settings`

Source: [`ui/src/routes/api/admin.settings.ts`](../../ui/src/routes/api/admin.settings.ts)

> App settings (admin). GET → current values. PUT → update. Grows as more
> app-wide settings land; audit retention is the first.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{auditRetentionDays, org, memberModels}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsettings-body) | `{ok}` | 200 | audit |

### PUT `/api/admin/settings` body

| field | schema | notes |
| :--- | :--- | :--- |
| `auditRetentionDays` | `z.number().int().min(0).max(3650).optional()` |  |
| `org` | `z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional()` |  |

## `/api/admin/storage`

Source: [`ui/src/routes/api/admin.storage.ts`](../../ui/src/routes/api/admin.storage.ts)

> Object storage (uploads blob store) config. GET → config (secrets masked) +
> blob location stats + migration/sync status + the built-in bucket's endpoint.
> PUT → save config. POST → connection tests, local→bucket migration, or a
> full sync to the replica.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{internal}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminstorage-body) | `{config}` | 200 | audit |
| POST | `admin` | [body](#post-apiadminstorage-body) | `{migrate}` | 200, 400 | audit |

### PUT `/api/admin/storage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `z.enum(['local', 'internal', 's3'])` |  |
| `replica` | `z.object({ enabled: z.boolean(),...Target })` |  |

### POST `/api/admin/storage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['test', 'test-replica', 'migrate', 'sync']).optional()` |  |

## `/api/admin/update`

Source: [`ui/src/routes/api/admin.update.ts`](../../ui/src/routes/api/admin.update.ts)

> In-app updates (admin). GET reads the panel's world (mode, current commit,
> last check, last run, auto-update switch). POST runs an action: `check`
> fetches the remote and compares, `apply` starts the whole pull/build/
> restart sequence. PUT flips the auto-update switch, which is off until
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{mode, current, autoUpdate, lastCheck, lastRun, history}` | 200 | — |
| POST | `admin` | [body](#post-apiadminupdate-body) | `{started}` | 200, 400 | audit |
| PUT | `admin` | [body](#put-apiadminupdate-body) | `{autoUpdate}` | 200 | audit |

### POST `/api/admin/update` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['check', 'apply'])` |  |

### PUT `/api/admin/update` body

| field | schema | notes |
| :--- | :--- | :--- |
| `autoUpdate` | `z.boolean()` |  |

## `/api/admin/users`

Source: [`ui/src/routes/api/admin.users.ts`](../../ui/src/routes/api/admin.users.ts)

> Admin console API. GET → all users with roles + agent allow-lists.
> PUT { userId, role? , agentModels? } → update either. Admins only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{users}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminusers-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/users` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |
| `role` | `z.enum(['admin', 'member']).optional()` |  |
| `agentModels` | `z.array(z.string().max(200)).max(100).optional()` |  |
| `canMintKeys` | `z.boolean().optional()` |  |
| `deniedViews` | `z.array(z.string().max(60)).max(40).optional()` |  |
| `allowedManageViews` | `z.array(z.string().max(60)).max(10).optional()` |  |

## `/api/admin/workspace-secrets`

Source: [`ui/src/routes/api/admin.workspace-secrets.ts`](../../ui/src/routes/api/admin.workspace-secrets.ts)

> WORKSPACE SECRETS — the credentials agents may USE without ever reading one.
>
> NOT `/api/admin/secrets`, WHICH IS A DIFFERENT THING. That route is the
> instance's own secret INVENTORY: provider keys, agent credentials, whether
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | [body](#get-apiadminworkspace-secrets-body) | `{held}` | 200, 400, 404 | audit |

### GET `/api/admin/workspace-secrets` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('create')` |  |
| `name` | `z.string().regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase letters, digits, - and _').max(40)` |  |
| `title` | `z.string().min(1).max(80)` |  |
| `entries` | `z.array(Entry).min(1).max(20)` |  |
| `kind` | `z.enum(['vault', 'relay']).optional()` |  |
| `note` | `z.string().max(400).nullish()` |  |
| `expiresAt` | `z.string().max(40).nullish()` |  |
| `uses` | `z.number().int().min(1).max(1000).nullish()` |  |
| `grantTo` | `z.array(z.string().max(120)).max(50).optional()` |  |

### GET `/api/admin/workspace-secrets` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('grant')` |  |
| `name` | `z.string().max(40)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

### GET `/api/admin/workspace-secrets` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('revoke')` |  |
| `name` | `z.string().max(40)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

### GET `/api/admin/workspace-secrets` body — variant 4

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('delete')` |  |
| `name` | `z.string().max(40)` |  |

### GET `/api/admin/workspace-secrets` body — variant 5

Schema not a literal object in the route file (// Folders) — see the route source.

### GET `/api/admin/workspace-secrets` body — variant 6

Schema not a literal object in the route file (for grouping credentials and granting a whole set to an agent at // once — the same argument that made folder sharing worth building for pe…) — see the route source.

### GET `/api/admin/workspace-secrets` body — variant 7

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('folder-delete')` |  |
| `id` | `Uuid` |  |

### GET `/api/admin/workspace-secrets` body — variant 8

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('folder-grant')` |  |
| `id` | `Uuid` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |
| `on` | `z.boolean()` |  |

### GET `/api/admin/workspace-secrets` body — variant 9

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('file')` |  |
| `name` | `z.string().max(40)` |  |
| `folderId` | `Uuid.nullable()` |  |

