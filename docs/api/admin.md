# API reference — admin

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

24 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/admin/apps`](#apiadminapps) | GET | `session` + `view:/apps` |
| [`/api/admin/apps`](#apiadminapps) | POST | `admin` |
| [`/api/admin/apps`](#apiadminapps) | PUT | `admin` |
| [`/api/admin/apps`](#apiadminapps) | DELETE | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | GET | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | POST | `admin` |
| [`/api/admin/domains`](#apiadmindomains) | DELETE | `admin` |
| [`/api/admin/email`](#apiadminemail) | GET | `admin` |
| [`/api/admin/email`](#apiadminemail) | POST | `admin` |
| [`/api/admin/email`](#apiadminemail) | PUT | `admin` |
| [`/api/admin/encryption`](#apiadminencryption) | GET | `admin` |
| [`/api/admin/encryption`](#apiadminencryption) | POST | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | GET | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | PUT | `admin` |
| [`/api/admin/google-client`](#apiadmingoogle-client) | DELETE | `admin` |
| [`/api/admin/google-client/login`](#apiadmingoogle-clientlogin) | PUT | `admin` |
| [`/api/admin/guardrails`](#apiadminguardrails) | GET | `admin` |
| [`/api/admin/guardrails`](#apiadminguardrails) | PUT | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | GET | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | POST | `admin` |
| [`/api/admin/instance`](#apiadmininstance) | PUT | `admin` |
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
| [`/api/admin/rag`](#apiadminrag) | POST | `admin` |
| [`/api/admin/rag`](#apiadminrag) | PUT | `admin` |
| [`/api/admin/search`](#apiadminsearch) | GET | `admin` |
| [`/api/admin/search`](#apiadminsearch) | PUT | `admin` |
| [`/api/admin/secrets`](#apiadminsecrets) | GET | `admin` |
| [`/api/admin/secrets`](#apiadminsecrets) | DELETE | `admin` |
| [`/api/admin/settings`](#apiadminsettings) | GET | `admin` |
| [`/api/admin/settings`](#apiadminsettings) | PUT | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | GET | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | POST | `admin` |
| [`/api/admin/storage`](#apiadminstorage) | PUT | `admin` |
| [`/api/admin/updates`](#apiadminupdates) | GET | `admin` |
| [`/api/admin/updates`](#apiadminupdates) | POST | `admin` |
| [`/api/admin/updates`](#apiadminupdates) | PUT | `admin` |
| [`/api/admin/users`](#apiadminusers) | GET | `admin` |
| [`/api/admin/users`](#apiadminusers) | PUT | `admin` |
| [`/api/admin/workspace-secrets`](#apiadminworkspace-secrets) | GET | `admin` |
| [`/api/admin/workspace-secrets`](#apiadminworkspace-secrets) | POST | `admin` |

## `/api/admin/apps`

Source: [`api/src/routes/admin/admin_apps.rs`](../../api/src/routes/admin/admin_apps.rs)

> /api/admin/apps. App administration. GET → installed apps (+ ?catalog=1
> for the marketplace feed). Reads are open to anyone granted the /apps
> Manage view; mutations (enable/disable, install, uninstall, catalog
> source) stay admin-only — installing an app adds CODE to the deployment.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/apps` | — | `{apps, pending, catalog, catalogUrl}` | 200 | — |
| POST | `admin` | [body](#post-apiadminapps-body) | `{slug, pendingBuild}` | 200, 400 | audit |
| PUT | `admin` | [body](#put-apiadminapps-body) | `{ok, catalogUrl}` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadminapps-body) | `{ok}` | 200, 400 | audit |

### POST `/api/admin/apps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `string?(64)` |  |

### PUT `/api/admin/apps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `app` | `string(1)` |  |
| `enabled` | `bool` |  |

### DELETE `/api/admin/apps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `app` | `string(1)` | app: required, min 1 char; wipeData: optional boolean. |
| `wipeData` | `bool?` |  |

## `/api/admin/domains`

Source: [`api/src/routes/admin/admin_domains.rs`](../../api/src/routes/admin/admin_domains.rs)

> /api/admin/domains. Sign-up domains. GET → the list. POST { domain } → add
> (returns the TXT token to publish). POST { verifyId } → run the DNS check.
> DELETE { id } → remove (self-joins from it stop immediately). Admins only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{domains}` | 200 | — |
| POST | `admin` | [body](#post-apiadmindomains-body) | `{domain}` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadmindomains-body) | `{ok}` | 200, 400 | audit |

### POST `/api/admin/domains` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

### DELETE `/api/admin/domains` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `uuid` | just { id } — a uuid. |

## `/api/admin/email`

Source: [`api/src/routes/admin/admin_email.rs`](../../api/src/routes/admin/admin_email.rs)

> /api/admin/email. Transactional email config. GET → config with secrets
> MASKED (set-flags only). PUT → config patch (the write); POST { test: true }
> → send a test to the caller.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config}` | 200 | — |
| POST | `admin` | [body](#post-apiadminemail-body) | `{ok}` | 200, 400, 502 | — |
| PUT | `admin` | [body](#put-apiadminemail-body) | `{ok}` | 200, 400 | audit |

### POST `/api/admin/email` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

### PUT `/api/admin/email` body

| field | schema | notes |
| :--- | :--- | :--- |
| `provider` | `nullish` |  |
| `from` | `string?(200)` |  |

## `/api/admin/encryption`

Source: [`api/src/routes/admin/admin_encryption.rs`](../../api/src/routes/admin/admin_encryption.rs)

> /api/admin/encryption. Encryption status + one-click key rotation.
> Rotating re-generates the data key and re-encrypts every stored secret
> (provider keys, agent secrets, OAuth tokens) in a single pass — one
> action, no per-secret steps.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{keyVersion, rotatedAt, secretCount, rootSource, algorithm}` | 200 | — |
| POST | `admin` | [body](#post-apiadminencryption-body) | `{reencrypted, rootRewrapped}` | 200, 400, 500 | audit |

### POST `/api/admin/encryption` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/admin/google-client`

Source: [`api/src/routes/admin/admin_google_client.rs`](../../api/src/routes/admin/admin_google_client.rs)

> /api/admin/google-client. The Google OAuth client — the credential the
> whole Google integration (login + workspace connect) runs on. Admins
> register it here instead of editing env; the secret is SEALED and never
> read back. Deliberately requireAdmin: this is an org credential, not a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{status, loginEnabled, loginPinnedByEnv, personalConnected, redirectUris}` | 200 | — |
| PUT | `admin` | [body](#put-apiadmingoogle-client-body) | `{status, loginEnabled, loginPinnedByEnv}` | 200, 400 | audit |
| DELETE | `admin` | — | `{status, loginEnabled, loginPinnedByEnv}` | 200 | audit |

### PUT `/api/admin/google-client` body

| field | schema | notes |
| :--- | :--- | :--- |
| `clientId` | `string(1, 200)` |  |
| `clientSecret` | `string? nullish` |  |
| `hd` | `string? nullish` |  |

## `/api/admin/google-client/login`

Source: [`api/src/routes/admin/admin_google_client.rs`](../../api/src/routes/admin/admin_google_client.rs)

> /api/admin/google-client. The Google OAuth client — the credential the
> whole Google integration (login + workspace connect) runs on. Admins
> register it here instead of editing env; the secret is SEALED and never
> read back. Deliberately requireAdmin: this is an org credential, not a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apiadmingoogle-clientlogin-body) | `{loginEnabled}` | 200, 400 | audit |

### PUT `/api/admin/google-client/login` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `bool` |  |

## `/api/admin/guardrails`

Source: [`api/src/routes/admin/admin_guardrails.rs`](../../api/src/routes/admin/admin_guardrails.rs)

> /api/admin/guardrails. Confab guardrail config + observability (admin).
> GET → config + stats + recent findings. PUT → update config. The config
> is stored RAW (the five-key shape) and the GET reads defaults under
> stored: numbers pass through as written — never re-serialized from an f64.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config, stats, findings, rules}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminguardrails-body) | `{config}` | 200, 400 | audit |

### PUT `/api/admin/guardrails` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `enum(off|observe|annotate|strict)` |  |
| `minConfidence` | `number(0, 1)` | Validated only — the stored config passes the number through as the client wrote it (a stored 1 reads back as 1, not 1.0). |
| `coach` | `bool` |  |

## `/api/admin/instance`

Source: [`api/src/routes/admin/admin_instance.rs`](../../api/src/routes/admin/admin_instance.rs)

> /api/admin/instance. The instance's hosting domain and display name.
> GET → both configs. PUT { domain } sets the domain (unverified until the
> round trip passes), { domain: null } clears it; { companyName } sets the
> display name, { companyName: null } clears it — one PUT may carry either
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{instance, companyName}` | 200 | — |
| POST | `admin` | [body](#post-apiadmininstance-body) | `…` | 200, 400 | audit |
| PUT | `admin` | [body](#put-apiadmininstance-body) | `{instance, companyName}` | 200, 400 | audit |

### POST `/api/admin/instance` body

| field | schema | notes |
| :--- | :--- | :--- |
| `verify` | `literal(true)` |  |

### PUT `/api/admin/instance` body

| field | schema | notes |
| :--- | :--- | :--- |
| `companyName` | `string? nullable` |  |
| `domain` | `string? nullable(3, 253)` |  |

## `/api/admin/invites`

Source: [`api/src/routes/admin/admin_invites.rs`](../../api/src/routes/admin/admin_invites.rs)

> /api/admin/invites. Invites. GET → recent invites with state. POST
> { email } → create + send (re-invites re-issue with a fresh token).
> DELETE { id } → revoke.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{invites}` | 200 | — |
| POST | `admin` | [body](#post-apiadmininvites-body) | `…` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apiadmininvites-body) | `{ok}` | 200, 400 | audit |

### POST `/api/admin/invites` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `string(0, 200)` | email max 200 — the empty string is legal; create_invite itself answers it. |

### DELETE `/api/admin/invites` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `uuid` | { id }: type message first, then "Invalid UUID". |

## `/api/admin/judge`

Source: [`api/src/routes/admin/admin_judge.rs`](../../api/src/routes/admin/admin_judge.rs)

> /api/admin/judge. The automated QA judge config (admin). GET → current +
> available models. PUT → enable/disable + pick the judge model.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config, models}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminjudge-body) | `{config}` | 200, 400 | audit |

### PUT `/api/admin/judge` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `bool` |  |
| `model` | `string? nullish(200)` |  |
| `mode` | `enum(advisory|enforcing)?` |  |

## `/api/admin/model-fitness`

Source: [`api/src/routes/admin/admin_model_fitness.rs`](../../api/src/routes/admin/admin_model_fitness.rs)

> /api/admin/model-fitness. Admin → Models → Fitness, over HTTP. THIS FILE
> IS PLUMBING: the admin gate, the query string, the body parse, the audit
> line, the status code. Every decision — what a capability tag says across
> a pooled endpoint set, what a run will cost, what the archive keeps and
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200, 400 | — |
| POST | `admin` | [body](#post-apiadminmodel-fitness-body) | `{started, status, runs}` | 200, 400, 409 | audit |

### POST `/api/admin/model-fitness` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/admin/model-roles`

Source: [`api/src/routes/admin/admin_model_roles.rs`](../../api/src/routes/admin/admin_model_roles.rs)

> /api/admin/model-roles. Model Roles — which model handles each activity
> class. GET → the catalog of roles + current assignments + assignable
> models + fitness issues. PUT { role, model|null } → assign (null = back
> to auto). Admins only.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{roles, assignments, models, issues, efforts}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminmodel-roles-body) | `…` | 200, 400 | audit |

### PUT `/api/admin/model-roles` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `enum(research-recon|research-brief|research-expedition|utility|code-light|code-standard|code-heavy|vision|image-generation|embedding|reranker)` | Schema order: role, then model, then effort — an invalid role outranks an invalid model, which outranks an invalid effort. |
| `model` | `string? nullable(200)` |  |
| `effort` | `string? nullable` | Absent = leave the preference alone (a model-only save); null = clear. |

## `/api/admin/outreach`

Source: [`api/src/routes/admin/admin_outreach.rs`](../../api/src/routes/admin/admin_outreach.rs)

> /api/admin/outreach. GET → config + per-agent proactive flags + recent
> events. PUT → save both. Admin-only; the sweep itself stays off unless
> `enabled`.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config, agents, events}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminoutreach-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/outreach` body

| field | schema | notes |
| :--- | :--- | :--- |
| `enabled` | `bool` |  |
| `intervalMinutes` | `number(15, 1440)` |  |
| `dailyDmCap` | `number(1, 20)` |  |

## `/api/admin/password-accounts`

Source: [`api/src/routes/admin/admin_password_accounts.rs`](../../api/src/routes/admin/admin_password_accounts.rs)

> /api/admin/password-accounts. The admin console's API for DB-backed
> password accounts (Admin → People).
>   GET    → the account list.   POST → create an account.
>   PUT    → set/reset a password. DELETE → remove the account (the person
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{accounts}` | 200 | — |
| POST | `admin` | [body](#post-apiadminpassword-accounts-body) | `{ok, userId}` | 200, 400, 409 | audit |
| PUT | `admin` | [body](#put-apiadminpassword-accounts-body) | `{ok}` | 200, 400, 404, 409 | audit |
| DELETE | `admin` | [body](#delete-apiadminpassword-accounts-body) | `{ok}` | 200, 400, 404 | audit |

### POST `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |
| `password` | `string(8, 1000)` |  |
| `name` | `string?(200)` |  |

### PUT `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |
| `password` | `string(8, 1000)` |  |

### DELETE `/api/admin/password-accounts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |

## `/api/admin/permissions`

Source: [`api/src/routes/admin/admin_permissions.rs`](../../api/src/routes/admin/admin_permissions.rs)

> /api/admin/permissions. Fine-grained permissions admin. GET → the catalog
> + org member defaults + every user's overrides. PUT { userId, perm,
> allowed|null } → set/clear a per-user override (null = back to the org
> default). PUT { orgDefault: { perm, enabled|null } } → tune what plain
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{catalog, orgDefaults, overrides}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminpermissions-body) | `{overrides}` | 200, 400 | audit |

### PUT `/api/admin/permissions` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/admin/platform-agents`

Source: [`api/src/routes/admin/admin_platform_agents.rs`](../../api/src/routes/admin/admin_platform_agents.rs)

> /api/admin/platform-agents. Platform sub-agents — Talaria's own workers —
> and which model powers each. GET → registry + assignments + assignable
> models. PUT { id, model|null } → assign (null = back to auto). The Judge's
> pick lives in its own judge_config (shared with the Guard panel) — this
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{agents, assignments, models, efforts}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminplatform-agents-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/platform-agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `enum(…)` |  |
| `model` | `nullish` | Tri-state: absent leaves the assignment alone, null clears it, a string sets it — the PATCH distinction nullish_member exists for. |

## `/api/admin/rag`

Source: [`api/src/routes/admin/admin_rag.rs`](../../api/src/routes/admin/admin_rag.rs)

> /api/admin/rag. The retrieval console. GET → services health + both repair
> runs' projections + the upgrade status + reranker providers/config +
> KB-space brain bindings. PUT → reranker config and/or a space↔brain
> binding. POST → kick a repair run (detached), or { models, key? } → a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{health, backfill, upgrade, reindex, rerank, spaces}` | 200 | — |
| POST | `admin` | [body](#post-apiadminrag-body) | `{models}` | 200, 400 | audit |
| PUT | `admin` | [body](#put-apiadminrag-body) | `{rerank}` | 200, 400 | audit |

### POST `/api/admin/rag` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

### PUT `/api/admin/rag` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/admin/search`

Source: [`api/src/routes/admin/admin_search.rs`](../../api/src/routes/admin/admin_search.rs)

> /api/admin/search. LIVE WEB SEARCH — where it points, and whether it is
> actually answering.
>
> WHY THE REACHABILITY CHECK IS AN ADMIN SURFACE AND NOT A LOG LINE. When
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsearch-body) | `{url}` | 200, 400 | audit |

### PUT `/api/admin/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `url` | `string(0, 300)` | url max 300 — empty is a real instruction. |

## `/api/admin/secrets`

Source: [`api/src/routes/admin/admin_secrets.rs`](../../api/src/routes/admin/admin_secrets.rs)

> /api/admin/secrets. The secrets inventory. GET is a VIEW over the stores
> that own each value — presence, provenance and readability, never the
> value itself. DELETE clears one row's ciphertext, or every row that
> cannot be read.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200 | — |
| DELETE | `admin` | [body](#delete-apiadminsecrets-body) | `{cleared, failed}` | 200, 400, 404, 500 | audit |

### DELETE `/api/admin/secrets` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/admin/settings`

Source: [`api/src/routes/admin/admin_settings.rs`](../../api/src/routes/admin/admin_settings.rs)

> /api/admin/settings. App settings (admin). GET → current values. PUT →
> update. Grows as more app-wide settings land; audit retention is the
> first.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{windowHours, org, perAgent, agents}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminsettings-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/settings` body

| field | schema | notes |
| :--- | :--- | :--- |
| `auditRetentionDays` | `number?(0, 3650)` |  |
| `cronMinIntervalMinutes` | `number?(0, 1440)` |  |

## `/api/admin/storage`

Source: [`api/src/routes/admin/admin_storage.rs`](../../api/src/routes/admin/admin_storage.rs)

> /api/admin/storage. Object storage (uploads blob store) config. GET →
> config (secrets masked) + blob stats + migration/sync status + the
> built-in bucket's endpoint. PUT → save config. POST → connection tests,
> local→bucket migration, or a full sync to the replica.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{config, stats, migrate, sync, internal}` | 200 | — |
| POST | `admin` | [body](#post-apiadminstorage-body) | `{sync}` | 200, 400 | audit |
| PUT | `admin` | [body](#put-apiadminstorage-body) | `{config}` | 200, 400 | audit |

### POST `/api/admin/storage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(test|test-replica|migrate|sync)?` |  |

### PUT `/api/admin/storage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `enum(local|internal|s3)` | Body key order: mode, the Target spread, replica — a bad mode answers before any Target field can. |

## `/api/admin/updates`

Source: [`api/src/routes/admin/admin_updates.rs`](../../api/src/routes/admin/admin_updates.rs)

> /api/admin/updates — the update engine's panel surface and the fleet's
> machine surface. GET is the panel's whole read (and green's first read
> after a cutover: it runs the boot reconcile, so a run that landed while
> nobody was looking finishes the moment anyone looks). POST drives the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{mode, sentence, migrated, running, adoption, autoUpdate, machineKeySet, available, lastCheck, lastRun, history}` | 200 | — |
| POST | `admin` | [body](#post-apiadminupdates-body) | `…` | 200, 400, 409, 500, 502, 503 | audit |
| PUT | `admin` | [body](#put-apiadminupdates-body) | `{autoUpdate}` | 200, 400, 500 | audit |

### POST `/api/admin/updates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(check|apply|rollback|adopt|mint-key)?` |  |
| `edgePort` | `number?(1, 65535)` |  |

### PUT `/api/admin/updates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `autoUpdate` | `bool` |  |

## `/api/admin/users`

Source: [`api/src/routes/admin/admin_users.rs`](../../api/src/routes/admin/admin_users.rs)

> /api/admin/users. The people console. GET → every user with role, agent
> allow-list, view denials. PUT → the per-user levers, applied in order
> (role first, the assistant's elevation right behind it — a demotion
> collapses both).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{users}` | 200 | — |
| PUT | `admin` | [body](#put-apiadminusers-body) | `{ok}` | 200, 400 | audit |

### PUT `/api/admin/users` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |
| `role` | `enum(admin|member)?` |  |
| `canMintKeys` | `bool?` |  |
| `assistantElevated` | `bool?` |  |

## `/api/admin/workspace-secrets`

Source: [`api/src/routes/admin/admin_workspace_secrets.rs`](../../api/src/routes/admin/admin_workspace_secrets.rs)

> /api/admin/workspace-secrets.
>
> WORKSPACE SECRETS — the credentials agents may USE without ever reading one.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{secrets, folders}` | 200 | — |
| POST | `admin` | [body](#post-apiadminworkspace-secrets-body) | `{secret}` | 200, 400, 404 | audit |

### POST `/api/admin/workspace-secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `value` | name: the alphabet check ('lowercase letters, digits, - and _') sits before the max, like the entry key. |
| `title` | `string(1, 80)` |  |
| `kind` | `enum(vault|relay)?` |  |
| `note` | `string? nullish(400)` |  |
| `expiresAt` | `string? nullish(40)` |  |
| `uses` | `number? nullable(1, 1000)` | uses: int 1..1000, nullish. |
| `grantTo` | `string[]?(0, 120, 50)` |  |
| `allowedHosts` | `string[]?(0, 253, 30)` | Hosts this credential may be spent against. Empty/absent = unrestricted, which is what every secret predating the check has. |
| `name` | `string(0, 40)` |  |
| `agentModel` | `string(1, 120)` |  |
| `name` | `string(0, 40)` |  |
| `name` | `string(1, 60)` |  |
| `id` | `uuid` |  |
| `id` | `uuid` |  |
| `agentModel` | `string(1, 120)` |  |
| `on` | `bool` |  |
| `name` | `string(0, 40)` |  |
| `folderId` | `uuid? nullable` |  |

