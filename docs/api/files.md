# API reference — files

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
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
| [`/api/artifacts/{id}`](#apiartifactsid) | GET | `dual` |
| [`/api/artifacts/{id}`](#apiartifactsid) | PUT | `dual` |
| [`/api/artifacts/{id}/export/google`](#apiartifactsidexportgoogle) | POST | `dual` |
| [`/api/artifacts/{id}/links`](#apiartifactsidlinks) | POST | `session` |
| [`/api/artifacts/{id}/links`](#apiartifactsidlinks) | DELETE | `session` |
| [`/api/artifacts/for`](#apiartifactsfor) | GET | `session` |
| [`/api/artifacts/public/{slug}`](#apiartifactspublicslug) | GET | `public` |
| [`/api/artifacts/public/{slug}/download`](#apiartifactspublicslugdownload) | GET | `public` |
| [`/api/uploads`](#apiuploads) | POST | `session` + `perm:files.upload` |
| [`/api/uploads/{id}`](#apiuploadsid) | GET | `dual` |

## `/api/agent-media/{model}`

Source: [`ui/src/routes/api/agent-media.$model.ts`](../../ui/src/routes/api/agent-media.$model.ts)

> GET ?path=/opt/data/ → stream an image out of the agent's container, so
> media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
> Access + path/type guardrails live in server/agent-media.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 + varies | — |

## `/api/agent-media/{model}/save`

Source: [`ui/src/routes/api/agent-media.$model.save.ts`](../../ui/src/routes/api/agent-media.$model.save.ts)

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

## `/api/artifact-folders`

Source: [`ui/src/routes/api/artifact-folders.ts`](../../ui/src/routes/api/artifact-folders.ts)

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

## `/api/artifact-folders/{id}`

Source: [`ui/src/routes/api/artifact-folders.$id.ts`](../../ui/src/routes/api/artifact-folders.$id.ts)

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

Source: [`ui/src/routes/api/artifacts.ts`](../../ui/src/routes/api/artifacts.ts)

> Artifacts the caller can read. POST creates one (owned by the caller, or —
> for an agent over MCP — org-visible and editable by that agent).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | [body](#get-apiartifacts-body) | `{artifact}` | 200 | audit |

### GET `/api/artifacts` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `z.enum(['doc', 'sheet', 'microsite', 'file']).optional()` |  |
| `title` | `z.string().max(200).optional()` |  |
| `body` | `z.string().max(2_000_000).optional()` |  |

## `/api/artifacts/{id}`

Source: [`ui/src/routes/api/artifacts.$id.ts`](../../ui/src/routes/api/artifacts.$id.ts)

> One artifact. Read/edit gated by its audience; sharing owner-only; agents
> (by key) only edit content when granted the Editor role.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 403, 404 | — |
| PUT | `dual` | [body](#put-apiartifactsid-body) | `{artifact, editors}` | 200, 400, 403, 404 | audit |

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

## `/api/artifacts/{id}/export/google`

Source: [`ui/src/routes/api/artifacts.$id.export.google.ts`](../../ui/src/routes/api/artifacts.$id.export.google.ts)

> POST /api/artifacts/$id/export/google — mirror an artifact into Google Drive.
>
> Whose Drive it lands in depends on the caller (per-user OAuth):
>   human            → their own connected Drive
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | — | `{error, message}` | 200, 400, 403, 404, 409, 502 | audit |

## `/api/artifacts/{id}/links`

Source: [`ui/src/routes/api/artifacts.$id.links.ts`](../../ui/src/routes/api/artifacts.$id.links.ts)

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

Source: [`ui/src/routes/api/artifacts.for.ts`](../../ui/src/routes/api/artifacts.for.ts)

> Artifacts attached to a given target (e.g. a KB doc), filtered to the ones
> the caller can read.  GET /api/artifacts/for?targetType=kb-doc&targetId=<id>

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{artifacts}` | 200 | — |

## `/api/artifacts/public/{slug}`

Source: [`ui/src/routes/api/artifacts.public.$slug.ts`](../../ui/src/routes/api/artifacts.public.$slug.ts)

> Public artifact read — no auth. Only artifacts set to 'public' resolve.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{artifact}` | 200, 404 | — |

## `/api/artifacts/public/{slug}/download`

Source: [`ui/src/routes/api/artifacts.public.$slug.download.ts`](../../ui/src/routes/api/artifacts.public.$slug.download.ts)

> Public download for a public *file* artifact — no auth. Serves the stored
> bytes; only resolves when the artifact is public and points at an upload.
> The inline/download decision lives in serveUpload (server/uploads.ts) — this
> route is UNAUTHENTICATED, so it especially may not widen that allowlist.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `…` | 200, 404 | — |

## `/api/uploads`

Source: [`ui/src/routes/api/uploads.ts`](../../ui/src/routes/api/uploads.ts)

> POST (multipart/form-data, field "file") → store an attachment, return its
> metadata. Any signed-in user may upload; the file is served back from
> /api/uploads/:id.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:files.upload` | — | `…` | 200, 400 | — |

## `/api/uploads/{id}`

Source: [`ui/src/routes/api/uploads.$id.ts`](../../ui/src/routes/api/uploads.$id.ts)

> GET → serve an attachment's bytes: signed-in users, or fleet agents (agent
> key) pulling ticket/chat attachments they were handed. The inline/download
> decision lives in serveUpload (server/uploads.ts) — one allowlist, no route
> widens it on its own.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 404 | — |

