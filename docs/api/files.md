# API reference — files

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

> GET ?path=/opt/data/ → stream an image out of the agent's container, so
> media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
> Access + path/type guardrails live in server/agent-media.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 + varies | — |

## `/api/agent-media/{model}/save`

Source: [`api/src/routes/files/agent_media_model_save.rs`](../../api/src/routes/files/agent_media_model_save.rs)

> POST { path, title?, folderId? | folder? } → copy an image out of the
> agent's container into a durable FILE artifact (uploads-backed), optionally
> straight into a folder. For science. And company meme folders. Callable by
> humans (session; any agent they may use) AND by the agent itself over the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apiagent-mediamodelsave-body) | `{artifact}` | 200, 403 + varies | — |

### POST `/api/agent-media/{model}/save` body

| field | schema | notes |
| :--- | :--- | :--- |
| `path` | `z.string().min(1).max(1000)` |  |
| `title` | `z.string().trim().max(200).optional()` |  |
| `folderId` | `Uuid.nullish()` |  |
| `folder` | `z.string().trim().max(120).optional()` |  |

## `/api/artifact-folders`

Source: [`api/src/routes/files/artifact_folders.rs`](../../api/src/routes/files/artifact_folders.rs)

> Artifact folders. GET → the ones you can read. POST → create one you own.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |
| POST | `session` + `perm:artifacts.create` | [body](#post-apiartifact-folders-body) | `{folder}` | 200 | — |

### POST `/api/artifact-folders` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80)` |  |
| `parentId` | `Uuid.nullish()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |

## `/api/artifact-folders/{id}`

Source: [`api/src/routes/files/artifact_folders_id.rs`](../../api/src/routes/files/artifact_folders_id.rs)

> One artifact folder. GET → the folder + its grants (what the Share dialog
> reads). PUT → rename / icon / reparent / re-share. DELETE → remove (its
> artifacts and child folders fall back to the root).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403, 404 | — |
| PUT | `session` + `perm:artifacts.create` `perm:artifacts.publish` | [body](#put-apiartifact-foldersid-body) | `{folder}` | 200, 400, 403, 404 | — |
| DELETE | `session` + `perm:artifacts.create` | — | `{ok}` | 200, 403 | — |

### PUT `/api/artifact-folders/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80).optional()` |  |
| `icon` | `z.string().max(16).nullish()` |  |
| `parentId` | `Uuid.nullish()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |
| `editPolicy` | `z.enum(['owner', 'org', 'restricted']).optional()` |  |
| `editors` | `z.array(z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1), role: z.enum(['viewer', 'editor']) })).optiona…` |  |

## `/api/artifacts`

Source: [`api/src/routes/files/artifacts.rs`](../../api/src/routes/files/artifacts.rs)

> Artifacts the caller can read. POST creates one (owned by the caller, or —
> for an agent over MCP — org-visible and editable by that agent).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200 | — |
| POST | `dual` | [body](#post-apiartifacts-body) | `{artifact}` | 200 | audit |

### POST `/api/artifacts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `z.enum(['doc', 'sheet', 'microsite', 'file']).optional()` |  |
| `title` | `z.string().max(200).optional()` |  |
| `body` | `z.string().max(2_000_000).optional()` |  |
| `folder` | `z.string().trim().max(120).optional()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |

## `/api/artifacts/{id}`

Source: [`api/src/routes/files/artifacts_id.rs`](../../api/src/routes/files/artifacts_id.rs)

> One artifact. Read/edit gated by its audience; sharing owner-only; agents
> (by key) only edit content when granted the Editor role.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 403, 404 | — |
| PUT | `dual` | [body](#put-apiartifactsid-body) | `{artifact, editors}` | 200, 400, 403, 404 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PUT `/api/artifacts/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().max(200).optional()` |  |
| `body` | `z.string().max(2_000_000).optional()` |  |
| `icon` | `z.string().max(16).nullish()` |  |
| `storageRef` | `Uuid.nullish()` |  |
| `contentType` | `z.string().max(200).nullish()` |  |
| `folderId` | `Uuid.nullish()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |
| `editPolicy` | `z.enum(['owner', 'org', 'restricted']).optional()` |  |
| `editors` | `z.array(Editor).max(200).optional()` |  |
| `official` | `z.boolean().optional()` |  |
| `ragRouting` | `z.string().max(60).optional()` |  |

## `/api/artifacts/{id}/export/google`

Source: [`api/src/routes/files/artifacts_id_export_google.rs`](../../api/src/routes/files/artifacts_id_export_google.rs)

> POST /api/artifacts/$id/export/google — mirror an artifact into Google Drive.
>
> Whose Drive it lands in depends on the caller (per-user OAuth):
>   human            → their own connected Drive
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | — | `{error, message}` | 200, 400, 403, 404, 409, 502 | audit |

## `/api/artifacts/{id}/links`

Source: [`api/src/routes/files/artifacts_id_links.rs`](../../api/src/routes/files/artifacts_id_links.rs)

> Attach / detach an artifact to/from a target (KB doc, ticket, channel, ).
> The caller must be able to read the artifact.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiartifactsidlinks-body) | `{ok}` | 200, 403, 404 | — |
| DELETE | `session` | [body](#delete-apiartifactsidlinks-body) | `{ok}` | 200, 403, 404 | — |

### POST `/api/artifacts/{id}/links` body

| field | schema | notes |
| :--- | :--- | :--- |
| `targetType` | `z.string().min(1).max(40)` |  |
| `targetId` | `z.string().min(1).max(200)` |  |

### DELETE `/api/artifacts/{id}/links` body

| field | schema | notes |
| :--- | :--- | :--- |
| `targetType` | `z.string().min(1).max(40)` |  |
| `targetId` | `z.string().min(1).max(200)` |  |

## `/api/artifacts/for`

Source: [`api/src/routes/files/artifacts_for.rs`](../../api/src/routes/files/artifacts_for.rs)

> Artifacts attached to a given target (e.g. a KB doc), filtered to the ones
> the caller can read.  GET /api/artifacts/for?targetType=kb-doc&targetId=<id>

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{artifacts}` | 200 | — |

## `/api/artifacts/public/{slug}`

Source: [`api/src/routes/files/artifacts_public_slug.rs`](../../api/src/routes/files/artifacts_public_slug.rs)

> Public artifact read — no auth. Only artifacts set to 'public' resolve.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{artifact}` | 200, 404 | — |

## `/api/artifacts/public/{slug}/download`

Source: [`api/src/routes/files/artifacts_public_slug_download.rs`](../../api/src/routes/files/artifacts_public_slug_download.rs)

> Public download for a public *file* artifact — no auth. Serves the stored
> bytes; only resolves when the artifact is public and points at an upload.
> The inline/download decision lives in serveUpload (server/uploads.ts) — this
> route is UNAUTHENTICATED, so it especially may not widen that allowlist.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200, 404 | — |

## `/api/uploads`

Source: [`api/src/routes/files/uploads.rs`](../../api/src/routes/files/uploads.rs)

> POST (multipart/form-data, field "file") → store an attachment, return its
> metadata. Any signed-in user may upload; the file is served back from
> /api/uploads/:id. The body is read through readUploadForm — an oversized
> upload is refused before it is buffered (413), not discovered after.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:files.upload` | — | `…` | 200, 400 + varies | — |

## `/api/uploads/{id}`

Source: [`api/src/routes/files/uploads_id.rs`](../../api/src/routes/files/uploads_id.rs)

> GET → serve an attachment's bytes: signed-in users, or fleet agents (agent
> key) pulling ticket/chat attachments they were handed. The inline/download
> decision lives in serveUpload (server/uploads.ts) — one allowlist, no route
> widens it on its own.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 404 | — |

