# API reference — secrets

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

> The sealed-secrets vault. GET → the secrets the caller can see
> (metadata only — values never leave the vault); POST → seal a new one.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{secrets}` | 200 | — |
| POST | `session` | [body](#post-apisecrets-body) | `{secret}` | 200, 500 | audit |
| PATCH | `session` | [body](#patch-apisecrets-body) | `{secret}` | 200, 403 | audit |
| DELETE | `session` | [body](#delete-apisecrets-body) | `{ok}` | 200, 403, 404 | audit |

### POST `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().min(1).max(80)` |  |
| `entries` | `z.array(Entry).min(1).max(20)` |  |
| `note` | `z.string().max(400).nullish()` |  |
| `folderId` | `Uuid.nullish()` |  |
| `readers` | `z.array(Uuid).max(50).optional()` |  |
| `grantTo` | `z.array(z.string().max(120)).max(50).optional()` |  |
| `allowedHosts` | `z.array(z.string().max(253)).max(30).optional()` |  |
| `expiresAt` | `z.string().max(40).nullish()` |  |

### PATCH `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().max(80)` |  |
| `folderId` | `Uuid.nullable()` |  |

### DELETE `/api/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().max(80)` |  |

## `/api/secrets/folders`

Source: [`api/src/routes/secrets/secrets_folders.rs`](../../api/src/routes/secrets/secrets_folders.rs)

> Secret folders: list / create / rename / delete. Folder membership
> gates what GET /api/secrets shows.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{folders}` | 200 | — |
| POST | `session` | [body](#post-apisecretsfolders-body) | `{folder}` | 200, 403 | audit |

### POST `/api/secrets/folders` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('create')` |  |
| `name` | `z.string().min(1).max(60)` |  |

### POST `/api/secrets/folders` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('rename')` |  |
| `id` | `Uuid` |  |
| `name` | `z.string().min(1).max(60)` |  |

### POST `/api/secrets/folders` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('delete')` |  |
| `id` | `Uuid` |  |

### POST `/api/secrets/folders` body — variant 4

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('share')` |  |
| `id` | `Uuid` |  |
| `on` | `z.boolean()` |  |
| `userId` | `Uuid.optional()` |  |
| `agentModel` | `z.string().max(120).optional()` |  |

## `/api/secrets/git-credential`

Source: [`api/src/routes/secrets/secrets_git_credential.rs`](../../api/src/routes/secrets/secrets_git_credential.rs)

> THE SANDBOX'S WAY IN — where a handle could not otherwise reach.
>
> A handle substitutes at the MCP gateway, which covers every tool call an agent
> makes THROUGH Talaria. It does not cover the shell inside a workbench sandbox:
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apisecretsgit-credential-body) | `{username, password}` | 200, 400, 404 | — |

### POST `/api/secrets/git-credential` body

| field | schema | notes |
| :--- | :--- | :--- |
| `host` | `z.string().min(1).max(253)` |  |
| `protocol` | `z.string().max(20).optional()` |  |
| `path` | `z.string().max(400).optional()` |  |

## `/api/secrets/relay`

Source: [`api/src/routes/secrets/secrets_relay.rs`](../../api/src/routes/secrets/secrets_relay.rs)

> HAND AN AGENT A CREDENTIAL, MID-CONVERSATION, WITHOUT PUTTING IT IN THE CHAT.
>
> The paste this exists to prevent is the ordinary one: somebody needs their
> agent to do a thing that takes a token, so they type the token into the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsrelay-body) | `{handle, name, label, expiresAt}` | 200, 403, 500 | audit |

### POST `/api/secrets/relay` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentModel` | `z.string().min(1).max(120)` |  |
| `label` | `z.string().min(1).max(60)` |  |
| `value` | `z.string().min(1).max(20_000)` |  |
| `note` | `z.string().max(400).nullish()` |  |
| `allowedHosts` | `z.array(z.string().max(253)).max(10).optional()` |  |

## `/api/secrets/reveal`

Source: [`api/src/routes/secrets/secrets_reveal.rs`](../../api/src/routes/secrets/secrets_reveal.rs)

> THE ONE ROUTE IN THIS FEATURE THAT RETURNS A CREDENTIAL.
>
> Everything else — the admin panel, the relay mint, the listing above — is
> built so that a value has nowhere to come back through. This is the deliberate
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsreveal-body) | `{value}` | 200 | — |

### POST `/api/secrets/reveal` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().max(80)` |  |
| `key` | `z.string().max(40)` |  |

## `/api/secrets/share`

Source: [`api/src/routes/secrets/secrets_share.rs`](../../api/src/routes/secrets/secrets_share.rs)

> Share a sealed secret with a person (share / unshare) or grant an
> agent access to it (grant / revoke). Values stay sealed; shares gate
> visibility.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apisecretsshare-body) | `{secret}` | 200, 403, 404 | audit |

### POST `/api/secrets/share` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('share')` |  |
| `name` | `z.string().max(80)` |  |
| `userId` | `Uuid` |  |

### POST `/api/secrets/share` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('unshare')` |  |
| `name` | `z.string().max(80)` |  |
| `userId` | `Uuid` |  |

### POST `/api/secrets/share` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('grant')` |  |
| `name` | `z.string().max(80)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

### POST `/api/secrets/share` body — variant 4

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.literal('revoke')` |  |
| `name` | `z.string().max(80)` |  |
| `agentModel` | `z.string().min(1).max(120)` |  |

