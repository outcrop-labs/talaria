# @talaria/sdk — the app developer surface

Everything a [Talaria app](./APPS.md) imports. Two entry points:

- `@talaria/sdk` — client: UI kit, session + data hooks, fetch helpers, motion presets. Used in
  `app.ts` and your `.svelte` files.
- `@talaria/sdk/server` — server: request context, document store, MCP tool definitions, harness
  definitions. Used in `server.ts`, `mcp.ts`, `harness.ts` and `harnesses/*.ts`.

The stack is **Svelte 5 (runes) + sv-router + @tanstack/svelte-query v6 + Tailwind v4**; see
[`ui/MIGRATION.md`](../ui/MIGRATION.md) for the conventions the platform itself follows. Both entry
points resolve from the host at build time (`@talaria/sdk` → `ui/src/sdk/index.ts`) — an app never
installs them. `svelte` and the host's own dependencies (`@tanstack/svelte-query`, `@lucide/svelte`
for icons — note that the unscoped `lucide-svelte` is an unrelated package) resolve from the host
too, so exactly one copy of each exists in the deployment. Import `svelte` freely; import anything
else through the SDK.

## Defining an app

```ts
// apps/<slug>/app.ts
import { defineApp } from '@talaria/sdk'
import ContactsWork from './ContactsWork.svelte'
import ContactsManage from './ContactsManage.svelte'
import ContactsSettings from './ContactsSettings.svelte'

export default defineApp({
  work: ContactsWork,          // Work nav → /x/<slug>
  manage: ContactsManage,      // Manage nav → /x/<slug>/manage
  settings: ContactsSettings,  // a tab on Settings
})
```

All three surfaces are optional and each is a Svelte `Component`; labels and identity come from
`talaria.json` (see [APPS.md](./APPS.md)). Access is explicit-grant: members see nothing until an
admin allows the app's views per person (Admin → People).

## Client — `@talaria/sdk`

### UI kit (Mercury design system)

The same primitives the platform is built from; visual parity with core surfaces is automatic.
See [UI-CONVENTIONS.md](./UI-CONVENTIONS.md) for when to use what.

| Group | Exports |
|---|---|
| Actions | `Button` (+`buttonClasses`; variants `primary`, `outline`, `ghost`, `danger`, `danger-outline`, `accent-soft`, `link`), `IconButton`, `CloseButton`, `SaveButton`/`useSavedFlash`, `CopyButton`/`CopyLinkButton`, `DangerLink` |
| Inputs | `Input`, `Textarea`, `Select`, `Combobox` (+`ComboOption`), `Checkbox`, `Radio`, `Toggle`, `InlineCreate`, `submitOnEnter`, `inlineEditKeys` |
| Structure | `Panel` (+`as=`), `SectionHeader`, `ViewHeader`, `Tabs` (+`TabItem`), `Segmented` (+`SegmentedOption`), `Modal`, `Disclosure`, `StatCard`, `Steps` |
| Display | `Chip` (tones, filter pills via `onSelect`/`selected`, removable tokens via `onRemove`), `StatusDot`, `EmptyState` (`variant`: `full`/`compact`/`inline`), `Avatar`, `Kbd`/`KeyHint`, `InfoTip`, `CodeBlock`, `Markdown`, `RichEditor` (+`RichEditorHandle`) |
| Feedback | `Skeleton`/`SkeletonRows`/`SkeletonCard`, `Generating`/`GeneratingDots`/`GeneratingOverlay`, `confirm`/`alert`/`prompt` |
| Menus | `useContextMenu` (right-click), `DropdownMenu` (anchored), `ContextMenuItem`/`ContextMenuEntry` types |
| Motion | `fade`, `fly`, `scale`, `slide`, `flip` + the presets `QUICK`, `POP`, `PANEL`, `LIST` |
| Utilities | `cn`, `controlSizes`/`ControlSize`, prop types (`ButtonProps`, `InputProps`, `TextareaProps`, `SelectProps`, `PanelProps`, `ChipProps`, `ChipTone`, `DotStatus`) |

Motion comes through the SDK rather than from `svelte/transition` directly: the wrappers degrade to a
quick fade when the OS asks for reduced motion, and the presets carry the platform's own durations, so
an app animates in the same grammar as the host (see [`ui/ANIMATIONS.md`](../ui/ANIMATIONS.md)). Usage
is identical to `svelte/transition` — `<div transition:fade>`, `<div in:fly={{ y: 8 }}>`.

```svelte
<script lang="ts">
  import { Button, Chip, EmptyState, Input, SkeletonRows, fade, QUICK } from '@talaria/sdk'
</script>
```

### Session + data

```ts
import { useMe, useHasPerm, useIsAdmin } from '@talaria/sdk'

const session = useMe()                        // a query; session.data is SessionUser | null
const mayUpload = useHasPerm('files.upload')   // a { current } box — read mayUpload.current
const admin = $derived(useIsAdmin(session.data))  // a plain predicate over a user, not a hook
```

`useHasPerm` returns `{ current }` rather than a bare boolean because a primitive computed once at
init would freeze before the session resolves; read `.current` in the template or inside `$derived`.
It defaults to `false` until then, so affordances appear rather than flash-then-vanish.

Svelte-query re-exports: `createQuery`, `createMutation`, `useQueryClient`, `keepPreviousData`.
Options are a **function** in v6 (`createQuery(() => ({ … }))`) and reactive sources must not be
destructured — read `query.data`, never `const { data } = query`.

### Talking to servers

```ts
import { api, appApi, useAppQuery, useAppInvalidate } from '@talaria/sdk'

// Any platform API — runs as the signed-in user; every permission and ACL applies.
const { agents } = await api<{ agents: Agent[] }>('/api/agents')

// Your own app server (see below) — pre-bound to /api/apps/<slug>/…
const contacts = appApi('contacts')
await contacts.post('contacts', { name: 'Ada' })

// svelte-query read. `path` may be a getter so reactive state re-keys the query.
const query = useAppQuery<{ contacts: Doc[] }>('contacts', () => `contacts?q=${q}`)
const invalidate = useAppInvalidate('contacts')   // after writes
```

`api()` throws on non-2xx with the server's `error` string; all helpers send/parse JSON.
`appApi(slug)` gives `get` / `post` / `put` / `patch` / `del`. Call `useAppQuery` and
`useAppInvalidate` during component init — they read the query client from context.

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

## Harnesses — two contracts, one word

Talaria has two things called a harness, and they are not specializations of each other. The SDK
names both explicitly; `defineHarness` is overloaded so old app code keeps compiling.

| | Workbench harness | Activity harness |
|---|---|---|
| What it is | A coding CLI an **agent** drives inside a sandbox | A model call **Talaria** makes on your behalf |
| Declared as | Shell templates, auth, env | A prompt renderer, an output contract, a model chain |
| Ships at | `apps/<slug>/harness.ts` | `apps/<slug>/harnesses/*.ts` (one per file) |
| Factory | `defineWorkbenchHarness` | `defineHarness<I, O>` |
| Carries code | No — declarative only | Yes (`render`, `verify`, `check`) |

### Workbench harnesses (`harness.ts`)

A coding tool agents drive through their workbench, default-exported from `apps/<slug>/harness.ts`:

```ts
import { defineWorkbenchHarness } from '@talaria/sdk/server'

export default defineWorkbenchHarness({
  slug: 'aider',
  label: 'Aider',
  auth: 'gateway',                                     // pointed at Talaria's gateway, metered
  invoke: 'aider --model <model> --message "<task>"',
  jsonInvoke: 'aider --model <model> --message "<task>" --yes --no-pretty',
  mcpConfig: { format: 'claude-json', filename: 'aider-mcp.json' },
  guide: 'Aider works in git-aware sessions; read its structured output and verify diffs yourself.',
})
```

Declarative only — no host code runs from the definition. The host merges builtin < app-shipped <
admin-custom JSON by slug. See "Shipping a harness" in [`APPS.md`](./APPS.md) and
[`WORKBENCH.md`](./WORKBENCH.md) for the full contract (auth, invocation templates, MCP serve and
pass-through, custom config renderers, probes, install hints).

### Activity harnesses (`harnesses/*.ts`)

A prompt, an output contract, a model policy and a failure policy — run by the one runner. Declaring
one gets your app model resolution, the capability floor, the guardrail pass, ledger attribution, the
repair turn on malformed JSON, a `harness_runs` row, and — if it declares `evals` — **its own column
in the org's model-fitness matrix**, without writing a line of any of it.

```ts
// apps/<slug>/harnesses/triage.ts
import { z } from 'zod'
import { defineHarness } from '@talaria/sdk/server'

const TRIAGE = z.object({ severity: z.enum(['low', 'medium', 'high']), reason: z.string() })

export default defineHarness<{ subject: string; body: string }, z.infer<typeof TRIAGE>>({
  id: 'support:triage',
  label: 'Support triage',
  job: 'Grades an inbound support message so the queue can order itself.',
  requires: ['json', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model; a weak one grades more coarsely and the queue stays usable.',
  },
  model: {},                                    // the default chain: Utility, env, first routable
  render: (input) => [
    { role: 'system', content: 'Grade this support message. Reply with severity and a one-line reason.' },
    { role: 'user', content: `${input.subject}\n\n${input.body}` },
  ],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  evals: [
    {
      name: 'an outage report grades high',
      input: { subject: 'Checkout is down', body: 'Nobody can pay since 09:00. Every card is declined.' },
      check: (v) => (v.severity === 'high' ? null : `graded "${v.severity}" — a total checkout outage is high`),
    },
  ],
})
```

Also exported for this: the types `HarnessDefinition`, `EvalCase`, `RoleFloor`, `RenderContext`,
`Message`, `Grounding`, `Verify`, `Capability`, `ModelSpec`, `HarnessResult`, and the helper
`belowAnswerFloor` — the floor a one-sided *text* fixture needs, because a `check` that only asserts
what an answer must **not** be is passed by a model that says almost nothing.

Only **enabled** apps are loaded, one harness per file, from the module's default export. A
definition that fails to import, or that is not a well-formed harness, is a logged skip rather than an
outage: the caller is usually the admin panel or the fitness matrix enumerating every harness in the
install, and one broken app must not be able to empty that page.

**[`HARNESSES.md`](./HARNESSES.md) is the contract**: every field, the small-model story (the
balanced-brace scanner, the repair turn, why `verify` exists), the capability model, and how to write
evals a machine can check without another model judging.

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
