# API reference — admin

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

24 routes.

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
| [`/api/admin/google-client/login`](#apiadmingoogle-clientlogin) | PUT | `admin` |
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
| [`/api/admin/model-roles`](#apiadminmodel-roles) | PUT | `admin` |
| [`/api/admin/outreach`](#apiadminoutreach) | GET | `admin` |
| [`/api/admin/outreach`](#apiadminoutreach) | PUT | `admin` |
| [`/api/admin/password-accounts`](#apiadminpassword-accounts) | GET | `admin` |
| [`/api/admin/password-accounts`](#apiadminpassword-accounts) | POST | `admin` |
| [`/api/admin/password-accounts`](#apiadminpassword-accounts) | PUT | `admin` |
| [`/api/admin/password-accounts`](#apiadminpassword-accounts) | DELETE | `admin` |
| [`/api/admin/permissions`](#apiadminpermissions) | GET | `admin` |
| [`/api/admin/permissions`](#apiadminpermissions) | PUT | `admin` |
| [`/api/admin/platform-agents`](#apiadminplatform-agents) | GET | `admin` |
| [`/api/admin/platform-agents`](#apiadminplatform-agents) | PUT | `admin` |
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
| [`/api/admin/workspace-secrets`](#apiadminworkspace-secrets) | POST | `admin` |

## `/api/admin/apps`

Source: [`api/src/routes/admin/admin_apps.rs`](../../api/src/routes/admin/admin_apps.rs)

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

Source: [`api/src/routes/admin/admin_domains.rs`](../../api/src/routes/admin/admin_domains.rs)

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

Source: [`api/src/routes/admin/admin_email.rs`](../../api/src/routes/admin/admin_email.rs)

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

Source: [`api/src/routes/admin/admin_encryption.rs`](../../api/src/routes/admin/admin_encryption.rs)

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

Source: [`api/src/routes/admin/admin_google_client.rs`](../../api/src/routes/admin/admin_google_client.rs)

> The Google OAuth client — the credential the whole Google integration (login
> + workspace connect) runs on. Admins register it here instead of editing
> ui/.env; the secret is SEALED and never read back. Deliberately requireAdmin:
> this is an org credential, not a grantable surface.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{status, loginEnabled, loginPinnedByEnv, personalConnected, redirectUris}` | 200 + varies | — |
| PUT | `admin` | [body](#put-apiadmingoogle-client-body) | `{status, loginEnabled, loginPinnedByEnv}` | 200 + varies | audit |
| DELETE | `admin` | — | `{status, loginEnabled, loginPinnedByEnv}` | 200 + varies | audit |

### PUT `/api/admin/google-client` body

| field | schema | notes |
| :--- | :--- | :--- |
| `clientId` | `z.string().min(1).max(200)` |  |
| `clientSecret` | `z.string().max(400).nullable().optional()` |  |
| `hd` | `z.string().max(200).nullable().optional()` |  |

## `/api/admin/google-client/login`

Source: [`api/src/routes/admin/admin_google_client.rs`](../../api/src/routes/admin/admin_google_client.rs)

> The Google LOGIN switch — the policy half of the client credential
> (PUT /api/admin/google-client stores the credential; this decides whether the
> login screen offers it). Flipping it is an admin's deliberate, audit-logged
> act; AUTH_GOOGLE_ENABLED pinned in env still wins towards on. A client must
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apiadmingoogle-clientlogin-body) | `{loginEnabled}` | 200 | audit |

### PUT `/api/admin/google-client/login` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `z.boolean()` |  |

## `/api/admin/guardrails`

Source: [`api/src/routes/admin/admin_guardrails.rs`](../../api/src/routes/admin/admin_guardrails.rs)

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

Source: [`api/src/routes/admin/admin_instance.rs`](../../api/src/routes/admin/admin_instance.rs)

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

Source: [`api/src/routes/admin/admin_invites.rs`](../../api/src/routes/admin/admin_invites.rs)

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

Source: [`api/src/routes/admin/admin_judge.rs`](../../api/src/routes/admin/admin_judge.rs)

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

Source: [`api/src/routes/admin/admin_model_fitness.rs`](../../api/src/routes/admin/admin_model_fitness.rs)

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
| `reprobe` | `z.boolean().optional()` |  |
| `concurrency` | `z.number().int().min(1).max(8).optional()` |  |
| `retryFailed` | `z.boolean().optional()` |  |
| `supplement` | `z.boolean().optional()` |  |

### POST `/api/admin/model-fitness` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('stop')` |  |
| `model` | `z.string().max(200).nullish()` |  |

### POST `/api/admin/model-fitness` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('forget')` |  |
| `model` | `z.string().min(1).max(200)` |  |

### POST `/api/admin/model-fitness` body — variant 4

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('clear')` |  |
| `model` | `z.string().max(200).nullish()` |  |

## `/api/admin/model-roles`

Source: [`api/src/routes/admin/admin_model_roles.rs`](../../api/src/routes/admin/admin_model_roles.rs)

> Model Roles — which model handles each activity class. GET → the catalog of
> roles + current assignments + assignable models + fitness issues. PUT
> { role, model|null } → assign (null = back to auto). Admins only.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{roles, assignments, models, issues}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminmodel-roles-body) | `{assignments, issues}` | 200, 400 | audit |

### PUT `/api/admin/model-roles` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `z.enum(ROLES as [string,...string[]])` |  |
| `model` | `z.string().max(200).nullable().optional()` |  |
| `effort` | `z.string().min(1).max(24).nullable().optional()` |  |

## `/api/admin/outreach`

Source: [`api/src/routes/admin/admin_outreach.rs`](../../api/src/routes/admin/admin_outreach.rs)

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
| `proactiveAgents` | `z.array(z.string()).max(100)` |  |

## `/api/admin/password-accounts`

Source: [`api/src/routes/admin/admin_password_accounts.rs`](../../api/src/routes/admin/admin_password_accounts.rs)

> Admin console API for DB-backed password accounts (Admin → People).
> GET → the account list. POST → create an account. PUT → set/reset a
> password. DELETE → remove the account (the person stays). Admins only.
> Audit entries carry the email, never the password or its hash.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{accounts}` | 200 | — |
| POST | `admin` | [body](#post-apiadminpassword-accounts-body) | `{ok, userId}` | 200, 400, 409 | audit |
| PUT | `admin` | [body](#put-apiadminpassword-accounts-body) | `{ok}` | 200 + varies | audit |
| DELETE | `admin` | [body](#delete-apiadminpassword-accounts-body) | `{ok}` | 200, 404 | audit |

### POST `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `z.preprocess((v) => (typeof v === 'string' ? v.trim().toLowerCase() : v), z.string().email().max(200),)` |  |
| `password` | `z.string().min(8).max(1000)` |  |
| `name` | `z.string().max(200).optional()` |  |

### PUT `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |
| `password` | `z.string().min(8).max(1000)` |  |

### DELETE `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |

## `/api/admin/permissions`

Source: [`api/src/routes/admin/admin_permissions.rs`](../../api/src/routes/admin/admin_permissions.rs)

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

Source: [`api/src/routes/admin/admin_platform_agents.rs`](../../api/src/routes/admin/admin_platform_agents.rs)

> Platform sub-agents — Talaria's own workers (Muse, Distiller, Concluder, )
> and which model powers each. GET → registry + assignments + assignable
> models. PUT { id, model|null } → assign (null = back to auto). The Judge's
> pick lives in its own judge_config (shared with the Guard panel) — this
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{agents, models}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminplatform-agents-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/platform-agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `z.enum(IDS as [PlatformAgentId,...PlatformAgentId[]])` |  |
| `model` | `z.string().max(200).nullable().optional()` |  |
| `effort` | `z.string().min(1).max(24).nullable().optional()` |  |

## `/api/admin/rag`

Source: [`api/src/routes/admin/admin_rag.rs`](../../api/src/routes/admin/admin_rag.rs)

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
| `spaceBrain` | `z.object({ spaceId: Uuid, collectionId: Uuid.nullable() }).optional()` |  |

### POST `/api/admin/rag` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['reindex', 'backfill'])` |  |

### POST `/api/admin/rag` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `models` | `z.string().min(1).max(40)` |  |
| `key` | `z.string().max(500).nullish()` |  |

## `/api/admin/search`

Source: [`api/src/routes/admin/admin_search.rs`](../../api/src/routes/admin/admin_search.rs)

> LIVE WEB SEARCH — where it points, and whether it is actually answering.
>
> WHY THE REACHABILITY CHECK IS AN ADMIN SURFACE AND NOT A LOG LINE. When search
> is down, the way an operator currently finds out is that an agent tells
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{url, fromEnv, reachable, error}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsearch-body) | `{url, reachable, error}` | 200 | audit |

### PUT `/api/admin/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `url` | `z.string().max(300)` |  |

## `/api/admin/secrets`

Source: [`api/src/routes/admin/admin_secrets.rs`](../../api/src/routes/admin/admin_secrets.rs)

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

Source: [`api/src/routes/admin/admin_settings.rs`](../../api/src/routes/admin/admin_settings.rs)

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{auditRetentionDays, org, memberModels, llmBudgets, cronMinIntervalMinutes}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsettings-body) | `{ok}` | 200 | audit |

### PUT `/api/admin/settings` body

| field | schema | notes |
| :--- | :--- | :--- |
| `auditRetentionDays` | `z.number().int().min(0).max(3650).optional()` |  |
| `org` | `z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional()` |  |
| `memberModels` | `z.array(z.string().min(1).max(200)).max(200).optional()` |  |
| `llmBudgets` | `z.object({ windowHours: z.number().int().min(1).max(8760), org: budgetLimits.optional().default(null), perAgent: budgetLimits.optional().de…` |  |
| `cronMinIntervalMinutes` | `z.number().int().min(0).max(1440).optional()` |  |

## `/api/admin/storage`

Source: [`api/src/routes/admin/admin_storage.rs`](../../api/src/routes/admin/admin_storage.rs)

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

Source: [`api/src/routes/admin/admin_users.rs`](../../api/src/routes/admin/admin_users.rs)

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
| `assistantElevated` | `z.boolean().optional()` |  |

## `/api/admin/workspace-secrets`

Source: [`api/src/routes/admin/admin_workspace_secrets.rs`](../../api/src/routes/admin/admin_workspace_secrets.rs)

> WORKSPACE SECRETS — the credentials agents may USE without ever reading one.
>
> NOT `/api/admin/secrets`, WHICH IS A DIFFERENT THING. That route is the
> instance's own secret INVENTORY: provider keys, agent credentials, whether
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{held}` | 200 | — |
| POST | `admin` | [body](#post-apiadminworkspace-secrets-body) | `{secret}` | 200, 400, 404 | audit |

### POST `/api/admin/workspace-secrets` body — variant 1

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
| `allowedHosts` | `z.array(z.string().max(253)).max(30).optional()` |  |

### POST `/api/admin/workspace-secrets` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('grant')` |  |
| `name` | `z.string().max(40)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

### POST `/api/admin/workspace-secrets` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('revoke')` |  |
| `name` | `z.string().max(40)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

### POST `/api/admin/workspace-secrets` body — variant 4

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('delete')` |  |
| `name` | `z.string().max(40)` |  |

### POST `/api/admin/workspace-secrets` body — variant 5

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('folder-create')` |  |
| `name` | `z.string().min(1).max(60)` |  |

### POST `/api/admin/workspace-secrets` body — variant 6

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('folder-delete')` |  |
| `id` | `Uuid` |  |

### POST `/api/admin/workspace-secrets` body — variant 7

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('folder-grant')` |  |
| `id` | `Uuid` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |
| `on` | `z.boolean()` |  |

### POST `/api/admin/workspace-secrets` body — variant 8

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('file')` |  |
| `name` | `z.string().max(40)` |  |
| `folderId` | `Uuid.nullable()` |  |

