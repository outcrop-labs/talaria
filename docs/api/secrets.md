# API reference — secrets

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

6 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/secrets`](#apisecrets) | GET | `session` |
| [`/api/secrets`](#apisecrets) | POST | `session` |
| [`/api/secrets`](#apisecrets) | PATCH | `session` |
| [`/api/secrets`](#apisecrets) | DELETE | `session` |
| [`/api/secrets/folders`](#apisecretsfolders) | GET | `session` |
| [`/api/secrets/folders`](#apisecretsfolders) | POST | `session` |
| [`/api/secrets/git-credential`](#apisecretsgit-credential) | POST | `agent` |
| [`/api/secrets/relay`](#apisecretsrelay) | POST | `session` |
| [`/api/secrets/reveal`](#apisecretsreveal) | POST | `session` |
| [`/api/secrets/share`](#apisecretsshare) | POST | `session` |

## `/api/secrets`

Source: [`api/src/routes/secrets/secrets.rs`](../../api/src/routes/secrets/secrets.rs)

> /api/secrets.
>
> WORKING SECRETS — the ones a PERSON needs back. Not admin, and that is the
> entire reason this exists: somebody wiring up a staging integration has a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{secrets}` | 200 | — |
| POST | `session` | [body](#post-apisecrets-body) | `{secret}` | 200, 400, 500 | audit |
| PATCH | `session` | [body](#patch-apisecrets-body) | `{secret}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apisecrets-body) | `{ok}` | 200, 400, 403, 404 | audit |

### POST `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string(1, 80)` |  |
| `note` | `string? nullish(400)` |  |
| `folderId` | `uuid?` |  |
| `readers` | `uuid[]?(50)` |  |
| `grantTo` | `string[]?(0, 120, 50)` |  |
| `allowedHosts` | `string[]?(0, 253, 30)` |  |
| `expiresAt` | `string? nullish(40)` |  |
| `key` | `value` |  |
| `label` | `string(1, 60)` |  |
| `value` | `string(1, 20000)` |  |

### PATCH `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(0, 80)` | name is max-only; folderId is REQUIRED but may be null. |
| `folderId` | `uuid? nullable` |  |

### DELETE `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(0, 80)` |  |

## `/api/secrets/folders`

Source: [`api/src/routes/secrets/secrets_folders.rs`](../../api/src/routes/secrets/secrets_folders.rs)

> /api/secrets/folders.
>
> The Secrets view's own organisation, not the Files browser's. Sharing a
> folder is the point, not a bonus: a set somebody is actively working on
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{folders}` | 200 | — |
| POST | `session` | [body](#post-apisecretsfolders-body) | `{agentModel}` | 200, 400, 403 | audit |

### POST `/api/secrets/folders` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/secrets/git-credential`

Source: [`api/src/routes/secrets/secrets_git_credential.rs`](../../api/src/routes/secrets/secrets_git_credential.rs)

> /api/secrets/git-credential.
>
> THE SANDBOX'S WAY IN — where a handle could not otherwise reach. A handle
> substitutes at the MCP gateway, which covers every tool call an agent
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apisecretsgit-credential-body) | `{username, password}` | 200, 400, 404 | — |

### POST `/api/secrets/git-credential` body

| field | schema | notes |
| :--- | :--- | :--- |
| `host` | `string(1, 253)` |  |
| `protocol` | `string?(20)` |  |
| `path` | `string?(400)` |  |

## `/api/secrets/relay`

Source: [`api/src/routes/secrets/secrets_relay.rs`](../../api/src/routes/secrets/secrets_relay.rs)

> /api/secrets/relay.
>
> HAND AN AGENT A CREDENTIAL, MID-CONVERSATION, WITHOUT PUTTING IT IN THE
> CHAT. The paste this exists to prevent is the ordinary one: somebody needs
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsrelay-body) | `{handle, name, label, expiresAt}` | 200, 400, 403, 500 | audit |

### POST `/api/secrets/relay` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentModel` | `string(1, 120)` |  |
| `label` | `string(1, 60)` |  |
| `value` | `string(1, 20000)` |  |
| `note` | `string? nullish(400)` |  |
| `allowedHosts` | `string[]?(0, 253, 10)` | Optional: pin the one-shot to the host it is for — the only bound that survives the agent being talked into spending it elsewhere. |

## `/api/secrets/reveal`

Source: [`api/src/routes/secrets/secrets_reveal.rs`](../../api/src/routes/secrets/secrets_reveal.rs)

> /api/secrets/reveal.
>
> THE ONE ROUTE IN THIS FEATURE THAT RETURNS A CREDENTIAL. Everything else
> is built so a value has nowhere to come back through; this deliberate
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsreveal-body) | `{value}` | 200, 400, 403, 404, 409 | — |

### POST `/api/secrets/reveal` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(0, 80)` |  |
| `key` | `string(0, 40)` |  |

## `/api/secrets/share`

Source: [`api/src/routes/secrets/secrets_share.rs`](../../api/src/routes/secrets/secrets_share.rs)

> /api/secrets/share.
>
> Sharing a working secret — and the two audiences mean two different
> things. A PERSON gets a READER grant: they can reveal it, copy it, paste
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsshare-body) | `…` | 200, 400, 403, 404 | audit |

### POST `/api/secrets/share` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

