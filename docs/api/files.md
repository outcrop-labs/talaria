# API reference — files

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

13 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/agent-media/{model}`](#apiagent-mediamodel) | GET | `session` |
| [`/api/agent-media/{model}/save`](#apiagent-mediamodelsave) | POST | `dual` |
| [`/api/artifact-folders`](#apiartifact-folders) | GET | `session` |
| [`/api/artifact-folders`](#apiartifact-folders) | POST | `session` + `perm:artifacts.create` |
| [`/api/artifact-folders/{id}`](#apiartifact-foldersid) | GET | `session` |
| [`/api/artifact-folders/{id}`](#apiartifact-foldersid) | PUT | `session` + `perm:artifacts.create` `perm:artifacts.publish` |
| [`/api/artifact-folders/{id}`](#apiartifact-foldersid) | DELETE | `session` + `perm:artifacts.create` |
| [`/api/artifacts`](#apiartifacts) | GET | `dual` |
| [`/api/artifacts`](#apiartifacts) | POST | `dual` |
| [`/api/artifacts/{id}`](#apiartifactsid) | GET | `dual` |
| [`/api/artifacts/{id}`](#apiartifactsid) | PUT | `dual` |
| [`/api/artifacts/{id}`](#apiartifactsid) | DELETE | `session` |
| [`/api/artifacts/{id}/export/google`](#apiartifactsidexportgoogle) | POST | `dual` |
| [`/api/artifacts/{id}/links`](#apiartifactsidlinks) | POST | `session` |
| [`/api/artifacts/{id}/links`](#apiartifactsidlinks) | DELETE | `session` |
| [`/api/artifacts/for`](#apiartifactsfor) | GET | `session` |
| [`/api/artifacts/public/{slug}`](#apiartifactspublicslug) | GET | `public` |
| [`/api/artifacts/public/{slug}/download`](#apiartifactspublicslugdownload) | GET | `public` |
| [`/api/uploads`](#apiuploads) | POST | `session` + `perm:files.upload` |
| [`/api/uploads/{id}`](#apiuploadsid) | GET | `dual` |

## `/api/agent-media/{model}`

Source: [`api/src/routes/files/agent_media_model.rs`](../../api/src/routes/files/agent_media_model.rs)

> /api/agent-media/{model}.
> GET ?path=/opt/data/ → stream an image out of the agent's container, so
> media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
> Access + path/type guardrails live in agent_media.rs.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403, 500 | — |

## `/api/agent-media/{model}/save`

Source: [`api/src/routes/files/agent_media_model_save.rs`](../../api/src/routes/files/agent_media_model_save.rs)

> /api/agent-media/{model}/save.
> POST { path, title?, folderId? | folder? } → copy an image out of the
> agent's container into a durable FILE artifact (uploads-backed), optionally
> straight into a folder. For science. And company meme folders. Callable by
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apiagent-mediamodelsave-body) | `{artifact}` | 200, 400, 403, 500 | — |

### POST `/api/agent-media/{model}/save` body

| field | schema | notes |
| :--- | :--- | :--- |
| `path` | `string(1, 1000)` |  |
| `title` | `trimmed_optional` |  |
| `folderId` | `uuid?` |  |
| `folder` | `trimmed_optional` |  |

## `/api/artifact-folders`

Source: [`api/src/routes/files/artifact_folders.rs`](../../api/src/routes/files/artifact_folders.rs)

> /api/artifact-folders.
> Artifact folders. GET → the ones you can read. POST → create one you own.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{folders}` | 200 | — |
| POST | `session` + `perm:artifacts.create` | [body](#post-apiartifact-folders-body) | `{folder}` | 200, 400 | — |

### POST `/api/artifact-folders` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 80)` |  |
| `parentId` | `uuid? nullable` |  |
| `visibility` | `enum(private|org|public)?` |  |

## `/api/artifact-folders/{id}`

Source: [`api/src/routes/files/artifact_folders_id.rs`](../../api/src/routes/files/artifact_folders_id.rs)

> /api/artifact-folders/{id}. One artifact folder. GET → the
> folder + its grants (what the Share dialog reads). PUT → rename / icon /
> reparent / re-share. DELETE → remove (its artifacts and child folders fall
> back to the root).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{folder, editors}` | 200, 403, 404 | — |
| PUT | `session` + `perm:artifacts.create` `perm:artifacts.publish` | [body](#put-apiartifact-foldersid-body) | `{folder}` | 200, 400, 403, 404 | — |
| DELETE | `session` + `perm:artifacts.create` | — | `{ok}` | 200, 403 | — |

### PUT `/api/artifact-folders/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 80)` |  |
| `icon` | `string? nullable(16)` |  |
| `parentId` | `uuid? nullable` |  |
| `visibility` | `enum(private|org|public)?` |  |
| `editPolicy` | `enum(owner|org|restricted)?` |  |

## `/api/artifacts`

Source: [`api/src/routes/files/artifacts.rs`](../../api/src/routes/files/artifacts.rs)

> /api/artifacts. The artifact LIST
> (what the Files browser opens on) and CREATE. Read is gated exactly like
> the KB list beside it: org/public visible to all, private only to owner
> and grants. Creation differs by caller: a PERSONAL assistant's output
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{artifacts}` | 200 | — |
| POST | `dual` | [body](#post-apiartifacts-body) | `{artifact}` | 200, 400 | audit |

### POST `/api/artifacts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `enum(doc|sheet|microsite|file)?` |  |
| `title` | `string?(200)` |  |
| `body` | `string?(2000)` |  |
| `visibility` | `enum(private|org|public)?` |  |

## `/api/artifacts/{id}`

Source: [`api/src/routes/files/artifacts_id.rs`](../../api/src/routes/files/artifacts_id.rs)

> /api/artifacts/{id}. One artifact: read/edit gated by its audience,
> sharing owner-only, agents (by key) only edit content when granted the
> Editor role — a personal assistant READS its owner's artifacts the way it
> reads their docs (can_read_agent's owner arm), and edit stays grant-only.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{artifact, editors}` | 200, 403, 404 | — |
| PUT | `dual` | [body](#put-apiartifactsid-body) | `{artifact, editors}` | 200, 400, 403, 404 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PUT `/api/artifacts/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string?(200)` |  |
| `body` | `string?(2000)` |  |
| `icon` | `string? nullable(16)` |  |
| `storageRef` | `uuid? nullable` |  |
| `contentType` | `string? nullable(200)` |  |
| `folderId` | `uuid? nullable` |  |
| `visibility` | `enum(private|org|public)?` |  |
| `editPolicy` | `enum(owner|org|restricted)?` |  |
| `official` | `bool?` |  |
| `ragRouting` | `string?(60)` |  |

## `/api/artifacts/{id}/export/google`

Source: [`api/src/routes/files/artifacts_id_export_google.rs`](../../api/src/routes/files/artifacts_id_export_google.rs)

> /api/artifacts/{id}/export/google. Mirror an artifact into Google Drive.
>
> Whose Drive it lands in depends on the caller (per-user OAuth):
>   human            → their own connected Drive
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | — | `{file}` | 200, 400, 403, 404, 409, 502 | audit |

## `/api/artifacts/{id}/links`

Source: [`api/src/routes/files/artifacts_id_links.rs`](../../api/src/routes/files/artifacts_id_links.rs)

> /api/artifacts/{id}/links.
> Attach / detach an artifact to/from a target (KB doc, ticket, channel).
> The caller must be able to read the artifact — a human-only surface, so the
> gate runs before the body is even parsed: a bad body on a link you can't
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiartifactsidlinks-body) | `{ok}` | 200, 400, 403, 404 | — |
| DELETE | `session` | [body](#delete-apiartifactsidlinks-body) | `{ok}` | 200, 400, 403, 404 | — |

### POST `/api/artifacts/{id}/links` body

| field | schema | notes |
| :--- | :--- | :--- |
| `targetType` | `string(1, 40)` |  |
| `targetId` | `string(1, 200)` |  |

### DELETE `/api/artifacts/{id}/links` body

| field | schema | notes |
| :--- | :--- | :--- |
| `targetType` | `string(1, 40)` |  |
| `targetId` | `string(1, 200)` |  |

## `/api/artifacts/for`

Source: [`api/src/routes/files/artifacts_for.rs`](../../api/src/routes/files/artifacts_for.rs)

> /api/artifacts/for.
> Artifacts attached to a given target (e.g. a KB doc), filtered to the ones
> the caller can read. GET /api/artifacts/for?targetType=kb-doc&targetId=<id>
> — a static path that must win over /api/artifacts/{id} in the router.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{artifacts}` | 200 | — |

## `/api/artifacts/public/{slug}`

Source: [`api/src/routes/files/artifacts_public_slug.rs`](../../api/src/routes/files/artifacts_public_slug.rs)

> /api/artifacts/public/{slug}.
> Public artifact read — NO AUTH. Only artifacts set to 'public' resolve, and
> the response is a deliberate subset: never the id, the owner, the folder,
> the routing, or anything else the full artifact carries.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{artifact}` | 200, 404 | — |

## `/api/artifacts/public/{slug}/download`

Source: [`api/src/routes/files/artifacts_public_slug_download.rs`](../../api/src/routes/files/artifacts_public_slug_download.rs)

> /api/artifacts/public/{slug}/download.
> Public download for a public *file* artifact — NO AUTH. Serves the stored
> bytes; only resolves
> when the artifact is public and points at an upload. The inline/download
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 404 | — |

## `/api/uploads`

Source: [`api/src/routes/files/uploads.rs`](../../api/src/routes/files/uploads.rs)

> /api/uploads.
> POST (multipart/form-data, field "file") → store an attachment, return its
> metadata. Any signed-in user with the upload perm may upload; the file is
> served back from /api/uploads/{id}. The body is read through
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:files.upload` | — | `…` | 200, 400, 413 | — |

## `/api/uploads/{id}`

Source: [`api/src/routes/files/uploads_id.rs`](../../api/src/routes/files/uploads_id.rs)

> /api/uploads/{id}.
> GET → serve an attachment's bytes: signed-in users, or fleet agents (agent
> key) pulling ticket/chat attachments they were handed. The inline/download
> decision lives in serve_upload — one allowlist, no route widens it on its
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | — | — |

