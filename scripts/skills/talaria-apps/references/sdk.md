# The SDK, for an app in its own repo

The exact signatures are the kit's declarations: `types/src/sdk/index.d.ts` (the client),
`server.d.ts` and `testing.d.ts` under the kit's root (`talaria-app version` names the kit;
`$(dirname "$(readlink -f "$(command -v talaria-app)")")/..` is where it is). Read them before
you invent an import. This page is the map. Model calls: [harnesses.md](harnesses.md).

## The files

```
talaria.json       the manifest — the one required file
app.ts             defineApp({ work?, manage?, settings? }) — only when talaria.json declares a surface
*.svelte           one component per surface, plus whatever else you need
server.ts          defineAppServer({ fetch?, triggers? }) — /api/apps/<slug>/*, and the triggers
mcp.ts             defineAppMcp({ tools }) — tools the org's agents can call
harnesses/*.ts     one defineHarness per file — model calls, scored in Admin → Models → Fitness
harnesses/recorded/<file>.json   replies `talaria-app eval` replays offline
*.test.ts          your tests; `talaria-app check` runs them
```

The repo's root is the app's root. No `package.json`: the host installs no dependencies.

## talaria.json

| key | rule |
| :--- | :--- |
| `name` | Required. A non-empty display name, at most 120 characters |
| `slug` | The slug the app installs as: `^[a-z0-9][a-z0-9-]{0,63}$`, not `layout`, `hooks` or `meta`. Install uses it, refuses a taken one, and refuses an admin's different one. Without it the slug is the repo's name minus `talaria-app-`. Declare it: the one you passed `request_repo` as `appSlug` |
| `icon` | A glyph or emoji, at most 16 characters. Default `⬡` |
| `version` | Your own version, shown in Manage → Apps. Updates follow git commits, not this |
| `description` | At most 2000 characters |
| `surfaces` | `{ work?, manage?, settings? }`, each the surface's title. Declaring any requires `app.ts`; declaring none makes a server- or MCP-only app |
| `sdk` | The `@talaria/sdk` range you wrote against, npm range grammar (`"^1.0"`). A host outside it refuses the install and lists an installed app as broken, naming the version it needs. Declare it |
| `triggers` | `[{ id, cron }]` or `[{ id, event }]` — what runs your server with nobody calling ([below](#triggers)) |

Any other key is ignored. The full rules are the kit's `schema/talaria.schema.json`, and
`talaria-app check` holds the file to the validator install uses: it says
`installs as "<slug>"` and, with `sdk`, which SDK version this host serves.

## Surfaces

| surface | where | who reaches it |
| :--- | :--- | :--- |
| `work` | a dock tile → `/x/<slug>`, and every route below it | admins, and members granted the work view |
| `manage` | behind the dock's gear → `/x/<slug>/manage`, and every route below it | admins, and members granted the manage view |
| `settings` | a Settings tab: one page, no routes below it | follows the manage grant |

Apps are explicit-grant: enabling one gives members nothing until an admin grants its views. A
surface fills the pane: `h-full overflow-y-auto p-8` around a centred `mx-auto w-full max-w-3xl`
column, as the skeleton does. Use the kit's components, never a second design system. A
destructive action is quiet (`DangerLink`, `confirm()`), never a big red button.

## The client — `@talaria/sdk` (app.ts, *.svelte)

| You need | Reach for |
|---|---|
| Which app, which surface | `useApp()` → `{ slug, surface, manifest, rest, query }` (read it live, don't destructure) |
| Your own server | `useAppApi()` → `get/post/put/patch/del('items')`; `useAppQuery<T>('items')`, or ``useAppQuery(() => `items/${id}`)`` for a path that changes; `useAppInvalidate()` then `invalidate('items')` after a write |
| A read's three answers | `<QueryState {query} isEmpty={…}>` with `skeleton`, `empty` and `children(data)` snippets — a failed read shows its error and a Retry, never "No items yet" |
| A failed write | `catch (e) { await alert({ title, message: errorMessage(e) }) }` |
| Routes below a surface | `useAppRoute()` (`rest`, `segments`, `query`, `match('items/:id')`), `appHref('items/42')`, `appNavigate(path, { replace })`; `{ surface: 'manage' }` crosses to the other surface |
| The platform API | `api('/api/boards')` as the signed-in person; a non-2xx rejects with an `HttpError` |
| The person | `useMe()`, `useIsAdmin(user)`, `useHasPerm('boards.create')` |
| Live updates | `useUserEvents` (ids only, then read) |
| Components | `Button`, `IconButton`, `Input`, `Textarea`, `Select`, `Combobox`, `Checkbox`, `Toggle`, `AddRow`, `RenameField`, `Panel`, `ViewHeader`, `SectionHeader`, `Tabs`, `Segmented`, `Modal`, `Disclosure`, `StatCard`, `LibraryPane`, `RecordTitle`, `SortHeader`, `Chip`, `StatusDot`, `EmptyState`, `Avatar`, `Markdown`, `Popover`, `SkeletonRows`, `DropdownMenu`, `DangerLink`, `confirm` / `alert` / `prompt` |
| Motion | `fade`, `fly`, `scale`, `slide`, `flip`, with `QUICK`, `POP`, `PANEL`, `LIST` — from the SDK |
| Icons | `@lucide/svelte` |

The hooks read Svelte context: call them during a component's init, never in a plain `.ts`
module. Never spell the slug: the hooks default to the surrounding surface's app.

## The server — `@talaria/sdk/server` (server.ts)

```ts
import { defineAppServer, json, parseBody, z } from '@talaria/sdk/server'

const Body = z.object({ title: z.string().trim().min(1, 'title is required') })

export default defineAppServer({
  async fetch(request, ctx) {
    const [head, id] = ctx.segments
    if (head === 'items' && id === undefined && request.method === 'POST') {
      const body = await parseBody(request, Body)
      if (body instanceof Response) return body // the standard 400
      return json({ item: await ctx.store.insert('items', body) })
    }
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

| `ctx.` | what it is |
| :--- | :--- |
| `user` | The signed-in person, already authenticated |
| `access` | `{ admin, work, manage, settings }`: the surfaces they hold. The host admits anyone holding any one, so a manage-only route checks `ctx.access.manage` itself, on `ctx.segments[0]` |
| `perms`, `can(perm)` | Their platform permissions |
| `app`, `segments`, `path`, `url` | Your slug; the sub-path decoded once as segments (route on these); the same joined; the URL |
| `origin` | Where people reach the instance, for absolute links |
| `store` | Your document store (below) |
| `api(path, init?)` | The platform API as this person: `/api/…` only, never `/api/apps`, `/api/app-host` or `/api/internal`. A non-2xx throws `HttpError` with the server's sentence |
| `secret(name)` | A workspace secret an admin gave this app (`talaria-app://<slug>` on its allowed hosts), or null. Each read is audited |
| `signal` | Aborts when the caller disconnects. Pass it to long work |
| `agents`, `harness` | Drive an agent, run a harness: [harnesses.md](harnesses.md) |

Errors are `json({ error: '…' }, { status })`; a write answers `{ ok: true }` or the created
object. `parseBody` returns the data or the 400 `Response`: return it, don't throw it.

**The store.** `ctx.store` is your app's own collections of JSON documents, in its own Postgres.
`list(collection, { limit?, offset?, newestFirst? })`, `get(collection, id)` (null for an id that
isn't one), `insert(collection, data)`, `update(collection, id, patch)` (a shallow merge),
`remove(collection, id)` (a boolean), `count(collection)`, `wipe()`. A document is
`{ id, data, createdAt, updatedAt }`; ids are UUIDs the store mints.

## Triggers

Declare them in `talaria.json` and answer each by id in `server.ts`:

```json
{ "name": "Lead scorer", "triggers": [
  { "id": "nightly", "cron": "0 3 * * *" },
  { "id": "on-ticket", "event": "ticket.created" }
] }
```

```ts
export default defineAppServer({
  triggers: {
    async nightly(ctx) {
      const leads = await ctx.store.list('leads')
      return `scored ${leads.length} leads` // the run's line in Manage → Apps
    },
    async 'on-ticket'(ctx) {
      if (ctx.cause.kind !== 'event' || ctx.cause.event.name !== 'ticket.created') return
      await ctx.store.insert('seen', { ticketId: ctx.cause.event.ticketId })
    },
  },
})
```

- **`cron`** is five fields read in UTC (minute, hour, day of month, month, day of week: `*`, a
  number, `a-b`, `*/s`, `a-b/s`, `a/s`, comma lists), or `@hourly`, `@daily`, `@weekly`,
  `@monthly`, `@yearly`. Numbers only.
- **`event`** is `ticket.created` (`ticketId`, `boardId`) or `ticket.moved` (adds `from`, `to`
  and `closed`). **An event carries ids, never content.** Store the id; read the ticket when a
  person opens your app.
- **A trigger runs as the app, with no user.** Its `ctx` has `app`, `trigger`, `runId`, `cause`,
  `store`, `secret`, `origin`, `signal` (a 10-minute deadline) and `harness` (which may pin no
  agent). It has no `user` and no `api`, and `ctx.agents.turn` refuses.
- A throw, a missed deadline or a trigger with no handler is a failed run in Manage → Apps. It
  never turns the app off. Only an enabled app with a ready build fires.

## Agent tools — `mcp.ts`

```ts
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'notes_search', // prefixed: tool names are global
      description: 'Search notes by text. Returns [{ id, title }].',
      inputSchema: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] },
      async handler(args, ctx) {
        const q = String(args.query).toLowerCase()
        const rows = await ctx.store.list<{ title: string }>('notes', { limit: 500 })
        return rows.filter((r) => r.data.title.toLowerCase().includes(q)).map((r) => ({ id: r.id, title: r.data.title }))
      },
    },
  ],
})
```

- **A tool is checked when the app loads; one that fails is left out, never the app.** A name
  (letters, digits, `_`, `-`; at most 200; unique), a description, and an `inputSchema` that
  describes an object (JSON Schema, not zod; at most 32 KiB). `outputSchema` is optional.
  `talaria-app build` names every tool it kept or left out.
- **`inputSchema` is enforced before your handler runs.** A bad shape never reaches you.
- **The handler's `ctx`:** `app`, `caller` (`{ name, id, proven }` — proven only on the agent's
  own key; decide nothing that matters on an unproven caller), `actingUserId` (the person a
  personal assistant acts for, else null), `store`, `signal` (the server's deadline, 120 s by
  default), `agents` (the calling agent only), `harness`.
- **Results:** a string is one text block; any other value is its JSON; a throw is a tool error
  whose text the model reads; `toolResult({ content, structuredContent, isError })` is the
  whole MCP result. Keep results small.
- **Governance is Manage → MCP**, like any server: an admin decides which agents may call it,
  and every call is recorded (never its arguments). A changed catalog restarts the agents that
  carry it.
- **An argument may carry a resolved credential** (an agent's `«secret:…»` handle, resolved on
  the way in). Use it for the call. Never `ctx.store.insert(…, args)`, never log it, never echo
  it in a thrown message.

## Tests — `*.test.ts`

`talaria-app check` runs every `*.test.ts` under vitest, with `@talaria/sdk/testing`:

```ts
import { expect, test } from 'vitest'
import { callAppTool, memoryStore } from '@talaria/sdk/testing'
import mcp from './mcp'

test('search finds a stored note', async () => {
  const store = memoryStore()
  await store.insert('notes', { title: 'Supplier call' })
  const r = await callAppTool(mcp, 'notes_search', { query: 'supplier' }, { store })
  expect(r.isError).toBe(false)
  expect(r.text).toContain('Supplier call')
})
```

`callAppTool(mcp, name, args, ctx?)` runs a tool the way the gateway does (the load checks,
the `inputSchema` check, the deadline, the result's shaping) and answers what the agent would
receive. `memoryStore()` is a store in memory. `runHarnessOffline` runs a harness against
scripted replies ([harnesses.md](harnesses.md#test-it)). The client SDK does not run under
these tests; test your server and tools.
