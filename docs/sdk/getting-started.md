# Getting started

An app is a self-contained TypeScript codebase this instance compiles and renders as
native platform surfaces — same design system, same router, same session. Not an iframe, not
webhooks. Authors write against `@talaria/sdk`; they never write Rust.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.ts         UI surfaces — default-exports defineApp({...}); only when talaria.json declares one
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — default-exports defineAppServer({...}) → /api/apps/<slug>/*
  mcp.ts         optional agent tools — default-exports defineAppMcp({...})
  harnesses/     optional activity harnesses — one defineHarness per file, run from server.ts
```

Scaffold one with `bun talaria app new <slug>`, or drop a directory under `apps/` (or install
from **Manage → Apps**). Enable it in **Manage → Apps**. An app with no UI (a server or agent
tools only) declares no surfaces and ships no `app.ts` ([server.md](./server.md#an-app-with-no-ui)).
A harness runs only when your `server.ts` imports it and calls `runHarness`; nothing loads
`harnesses/` on its own ([harnesses.md](./harnesses.md)).

## The manifest

```json
{
  "name": "Contacts",
  "icon": "☏",
  "version": "0.1.0",
  "description": "Lightweight CRM — people, companies, stages, notes.",
  "surfaces": { "work": "Contacts", "manage": "Contacts data", "settings": "Contacts" },
  "sdk": "^1.0"
}
```

`name` is the one required key. The full shape, with every rule the host enforces, is
[`talaria.schema.json`](./talaria.schema.json); a manifest that breaks a rule is refused at
install with the reason, and an installed one is listed as broken. Two keys matter for an app
that is installed and updated from git:

- `slug` — the slug the app installs as, when it must not depend on the repository's name
  (its own code or docs name `/x/<slug>`). Install uses it and refuses a taken or reserved one.
  Leave it out and the host derives the slug from the repository; surfaces never hard-code it
  either way ([client.md](./client.md)).
- `sdk` — the `@talaria/sdk` range the app was written against (npm range syntax: `^1.0`,
  `>=1.2 <2`). A host outside the range refuses to install the app, and lists an installed one
  as broken after a host upgrade, instead of failing in a build.

Surfaces are opt-in; ship any combination:

| surface | where it appears | access |
| :--- | :--- | :--- |
| `work` | a tile in the top dock's Apps group → `/x/<slug>`, and every route below it | admins; members need an explicit grant |
| `manage` | the manage area behind the dock's gear: the `/manage` overview and its sidebar → `/x/<slug>/manage`, and every route below it | admins; members need an explicit grant |
| `settings` | a Settings tab (for people with access); one page, no routes below it | follows the manage grant |

Apps are **explicit-grant**: enabling one gives members nothing until an admin adds its views
per person in **Admin → People**, or per team in **Manage → Teams** — the same checklist that
governs core Manage views. The work and manage surfaces are separate grants, and each covers the
routes below it: a member with only the work grant reaches `/x/<slug>/items/42` and never
`/x/<slug>/manage/…`. An app that ships settings without a manage surface is still granted
through `/x/<slug>/manage`. The app API gateway admits anyone holding any of the app's surfaces
and tells your server which in `ctx.access`, so a route only the manage surface uses must check
`ctx.access.manage` itself ([server.md](./server.md#grants)). Routes below a surface:
[client.md](./client.md#routes-inside-your-app).

## Declare the surfaces

```ts
// apps/<slug>/app.ts
import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'
import Settings from './Settings.svelte'

export default defineApp({ work: Work, settings: Settings })
```

## First surface

```svelte
<!-- apps/<slug>/Work.svelte -->
<script lang="ts">
  import { EmptyState, QueryState, SkeletonRows, useAppQuery } from '@talaria/sdk'

  type Thing = { id: string; data: { name: string } }

  // GET /api/apps/<slug>/things — the slug is the surface's own, never spelled here.
  const things = useAppQuery<{ things: Thing[] }>('things')
</script>

<QueryState query={things} isEmpty={(d) => d.things.length === 0} errorTitle="Could not load the things">
  {#snippet skeleton()}<SkeletonRows />{/snippet}
  {#snippet empty()}<EmptyState title="No things yet" />{/snippet}
  {#snippet children(data)}
    {#each data.things as t (t.id)}
      <div>{t.data.name}</div>
    {/each}
  {/snippet}
</QueryState>
```

`QueryState` gives the read its three honest answers: your skeleton while it loads, the
failure (with a Retry) when the server could not answer, and the data — or your empty state
only when the server really answered with nothing. A 500 never reads as "No things yet".

That already reads like the platform because it is the platform: the kit, the query, the
session, the theme.

## Ten minutes to a real app

1. `bun talaria app new <slug>` — writes `apps/<slug>/` (a work surface with a list and a record
   page below it, a server, an MCP starter).
2. Replace the surface with yours ([ui-kit.md](./ui-kit.md), [client.md](./client.md)).
3. Grow the API ([server.md](./server.md)) — or call platform APIs from the client and skip
   a server entirely.
4. Optional: give agents tools ([mcp.md](./mcp.md)) or model calls
   ([harnesses.md](./harnesses.md)).
5. Enable in **Manage → Apps**; grant views in **Admin → People** (or to a team in
   **Manage → Teams**).

Publishing and the marketplace: [APPS.md](../APPS.md).


