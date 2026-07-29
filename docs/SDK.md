# @talaria/sdk — the app developer surface

Everything a [Talaria app](./APPS.md) imports. Two entry points:

- `@talaria/sdk` — client: UI kit, session + data hooks, fetch helpers. Used in `app.tsx`.
- `@talaria/sdk/server` — server: request context, document store, MCP tool definitions. Used in
  `server.ts` and `mcp.ts`.

Both resolve from the host at build time — an app never installs them. Shared dependencies (`react`,
`react-dom`, `@tanstack/react-query`, `@tanstack/react-router`, `lucide-react`) also resolve from the
host, so exactly one copy of each exists in the deployment. Import `react` freely; import anything
else through the SDK.

## Defining an app

```tsx
// apps/<slug>/app.tsx
import { defineApp } from '@talaria/sdk'

export default defineApp({
  work: WorkView,        // Work nav → /x/<slug>
  manage: ManageView,    // Manage nav → /x/<slug>/manage
  settings: SettingsPanel, // a tab on Settings
})
```

All three surfaces are optional; labels and identity come from `talaria.json` (see
[APPS.md](./APPS.md)). Access is explicit-grant: members see nothing until an admin allows the app's
views per person (Admin → People).

## Client — `@talaria/sdk`

### UI kit (Mercury design system)

The same primitives the platform is built from; visual parity with core surfaces is automatic.
See [UI-CONVENTIONS.md](./UI-CONVENTIONS.md) for when to use what.

| Group | Exports |
|---|---|
| Actions | `Button` (+`buttonClasses`, variants incl. `danger-outline`, `accent-soft`, `link`), `IconButton`, `CloseButton`, `SaveButton`/`useSavedFlash`, `CopyButton`/`CopyLinkButton`, `DangerLink` |
| Inputs | `Input`, `Textarea`, `Select`, `Combobox` (+`ComboOption`), `Checkbox`, `Radio`, `Toggle`, `InlineCreate`, `submitOnEnter`, `inlineEditKeys` |
| Structure | `Panel` (+`as=`), `SectionHeader`, `Tabs`, `Segmented`, `Modal`, `Disclosure`, `StatCard`, `Steps` |
| Display | `Chip` (tones, filter pills via `onSelect`/`selected`, removable tokens via `onRemove`), `StatusDot`, `EmptyState` (`full`/`compact`/`inline`), `Avatar`, `Kbd`/`KeyHint`, `CodeBlock`, `Markdown`, `RichEditor` |
| Feedback | `Skeleton`/`SkeletonRows`/`SkeletonCard`, `Generating`/`GeneratingDots`/`GeneratingOverlay`, `confirm`/`alert`/`prompt` |
| Menus | `useContextMenu` (right-click), `DropdownMenu` (anchored), `ContextMenuItem`/`ContextMenuEntry` types |
| Utilities | `cn`, `controlSizes`/`ControlSize`, component `Props` types |

### Session + data

```tsx
import { useMe, useHasPerm, useIsAdmin } from '@talaria/sdk'

const { data: me } = useMe()          // SessionUser: id, name, email, role
const mayUpload = useHasPerm('files.upload')
```

React-query re-exports: `useQuery`, `useMutation`, `useQueryClient`, `keepPreviousData`.

### Talking to servers

```tsx
import { api, appApi, useAppQuery, useAppInvalidate } from '@talaria/sdk'

// Any platform API — runs as the signed-in user; every permission and ACL applies.
const { agents } = await api<{ agents: Agent[] }>('/api/agents')

// Your own app server (see below) — pre-bound to /api/apps/<slug>/…
const contacts = appApi('contacts')
await contacts.post('contacts', { name: 'Ada' })
const { data } = useAppQuery<{ contacts: Doc[] }>('contacts', 'contacts')  // react-query read
const invalidate = useAppInvalidate('contacts')                            // after writes
```

`api()` throws on non-2xx with the server's `error` string; all helpers send/parse JSON.

## Server — `@talaria/sdk/server`

### App server (`server.ts`)

One fetch handler mounted at `/api/apps/<slug>/*`. The host authenticates the session, checks the
app is enabled, and checks the caller may reach this app **before** your handler runs.

```ts
import { defineAppServer, json } from '@talaria/sdk/server'

export default defineAppServer({
  async fetch(request, ctx) {
    // ctx.user   SessionUser (authenticated) — role checks beyond view access are yours
    // ctx.app    your slug
    // ctx.path   the part after /api/apps/<slug>/  e.g. "contacts/123"
    // ctx.url    parsed URL (query params)
    // ctx.store  your document store (below)
    if (ctx.path === 'things' && request.method === 'GET') {
      return json({ things: await ctx.store.list('things') })
    }
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

Follow the platform's [API conventions](./API-CONVENTIONS.md): errors are
`json({ error }, { status })`, mutations return `{ ok: true }` or the created object.

### Document store

Every app gets namespaced collections of JSON documents in Postgres — no migrations, invisible to
other apps:

```ts
interface AppStore {
  list(collection, { limit?, offset?, newestFirst? }?): Promise<AppDoc[]>
  get(collection, id): Promise<AppDoc | null>
  insert(collection, data): Promise<AppDoc>          // returns { id, data, createdAt, updatedAt }
  update(collection, id, patch): Promise<AppDoc | null>  // shallow merge
  remove(collection, id): Promise<boolean>
  count(collection): Promise<number>
  wipe(): Promise<void>                              // everything this app stored
}
```

For platform data (boards, knowledge, comms, agents, …) call the regular APIs from the client — they
run under the user's session, so an app can never do more than the person using it.

### MCP tools for agents (`mcp.ts`)

Publish tools the org's **agents** can call. They register as an MCP server (badged **app**) in
Manage → MCP and inherit the platform's full granular governance — per-agent assignment, tool
subsets, per-person access — enforced by the same gateway as every other server, dispatched
in-process. Lifecycle follows the app: disable retires the server and rolls its carriers.

```ts
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'things_search',
      description: 'Search things by text.',
      inputSchema: { type: 'object', properties: { query: { type: 'string' } } },
      async handler(args, ctx) {
        // ctx.agent  the calling agent's name (gateway-authenticated)
        // ctx.store  the same per-app store server.ts uses
        return ctx.store.list('things')     // return value serialized for the agent
      },
    },
  ],
})
```

## Security model

Apps compile into the deployment and run **fully trusted**, like platform code — admins install only
what they trust (the marketplace labels Outcrop-maintained apps **official**). The safety boundary
is the *session*: client calls run as the signed-in user; server handlers receive an authenticated
`ctx.user`; agent tools receive a gateway-authenticated agent identity. All existing permissions,
ACLs, and audit logging apply unchanged.

## Reference

`apps/contacts` in the repo is the working reference app: three surfaces, an app server, the store,
and three agent tools. [APPS.md](./APPS.md) covers anatomy, install, the marketplace, and
publishing.
