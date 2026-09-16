# SDK cheat sheet

Two entry points. Apps install nothing — these resolve from the host.

## `@talaria/sdk` — client (`app.ts`, `*.svelte`)

```ts
import { defineApp } from '@talaria/sdk'
import { Button, EmptyState, Input, Chip, SkeletonRows, confirm, api, appApi, useAppQuery, useAppInvalidate, useMe, useIsAdmin } from '@talaria/sdk'
```

- `defineApp({ work?, manage?, settings? })` — default-export from `app.ts`
- `appApi(slug)` → `{ get, post, put, patch, del }` against `/api/apps/<slug>/`
- `useAppQuery(slug, path | (() => path))` — svelte-query; options are a function
- `useAppInvalidate(slug)` — call at init; returned fn is safe anywhere
- `api(path)` — any platform route, as the signed-in user; throws on non-2xx
- Icons: `@lucide/svelte`. Motion: `fade`, `fly`, `QUICK` from the SDK, not `svelte/transition`

## `@talaria/sdk/server` — server (`server.ts`, `mcp.ts`)

```ts
import { defineAppServer, defineAppMcp, json, parseBody, z } from '@talaria/sdk/server'
```

`ctx` on the server: `user`, `app`, `path`, `url`, `store`.
`ctx` on MCP: `app`, `agent`, `store`.

Store: `list`, `get`, `insert`, `update` (shallow merge), `remove`, `count`, `wipe`.
An `AppDoc` is `{ id, data, createdAt, updatedAt }`.

Errors: `json({ error: '…' }, { status })`. Mutations: `{ ok: true }` or the created object.
`parseBody` returns `Response` on 400 — return it, do not throw.

MCP tool `name` is globally unique — prefix with the slug. `inputSchema` is JSON Schema, not zod.

## Anatomy

```
apps/<slug>/
  talaria.json   name, icon, version, description, surfaces
  app.ts         defineApp({ work, manage?, settings? })
  Work.svelte    pane-filling surface
  server.ts      defineAppServer — /api/apps/<slug>/*
  mcp.ts         defineAppMcp — agent tools
```

Surfaces opt in via `talaria.json`. Work → `/x/<slug>`. Manage → `/x/<slug>/manage`. Settings is a Settings tab.
