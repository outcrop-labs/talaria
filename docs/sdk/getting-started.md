# Getting started

An app is a self-contained TypeScript codebase this instance compiles and renders as
native platform surfaces — same design system, same router, same session. Not an iframe, not
webhooks. Authors write against `@talaria/sdk`; they never write Rust.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.ts         UI surfaces — default-exports defineApp({...})
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — default-exports defineAppServer({...})
  mcp.ts         optional agent tools — default-exports defineAppMcp({...})
  harnesses/     optional activity harnesses — one defineHarness per file
```

Scaffold one with `bun talaria app new <slug>`, or drop a directory under `apps/` (or install
from **Manage → Apps**). Enable it in **Manage → Apps**.

## The manifest

```json
{
  "name": "Contacts",
  "icon": "☏",
  "version": "0.1.0",
  "description": "Lightweight CRM — people, companies, stages, notes.",
  "surfaces": { "work": "Contacts", "manage": "Contacts data", "settings": "Contacts" }
}
```

Surfaces are opt-in; ship any combination:

| surface | where it appears | access |
| :--- | :--- | :--- |
| `work` | Work section of the nav → `/x/<slug>` | admins; members need an explicit grant |
| `manage` | Manage section → `/x/<slug>/manage` | admins; members need an explicit grant |
| `settings` | a Settings tab (for people with access) | follows the work-view grant |

Apps are **explicit-grant**: enabling one gives members nothing until an admin adds its views
per person in **Admin → People** — the same checklist that governs core Manage views. The app
API gateway enforces the grant server-side.

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
  import { EmptyState, SkeletonRows, useAppQuery } from '@talaria/sdk'

  const things = useAppQuery<{ things: Array<{ id: string; data: { name: string } }> }>(
    'my-app',
    'things',
  )
</script>

{#if things.isLoading}
  <SkeletonRows />
{:else if !things.data?.things.length}
  <EmptyState title="No things yet" />
{:else}
  {#each things.data.things as t (t.id)}
    <div>{t.data.name}</div>
  {/each}
{/if}
```

That already reads like the platform because it is the platform: the kit, the query, the
session, the theme.

## Ten minutes to a real app

1. `bun talaria app new <slug>` — writes `apps/<slug>/` (work surface, server, MCP starter).
2. Replace the surface with yours ([ui-kit.md](./ui-kit.md), [client.md](./client.md)).
3. Grow the API ([server.md](./server.md)) — or call platform APIs from the client and skip
   a server entirely.
4. Optional: give agents tools ([mcp.md](./mcp.md)) or model calls
   ([harnesses.md](./harnesses.md)).
5. Enable in **Manage → Apps**; grant views in **Admin → People**.

Publishing and the marketplace: [APPS.md](../APPS.md).


