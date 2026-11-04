# The skeleton `talaria-app scaffold` writes

> **Generated** from `cli/src/cmd/app/skeleton.ts`, the one source `talaria-app scaffold` (and
> `talaria app new` in a Talaria checkout) writes from, by `bun run docs:api` in the Talaria
> repository. Do not edit it by hand: `bun run check` fails when it drifts from the source.

Shown for an app named "Field Notes". Scaffold writes the name you give it: `--name`, else a
seed `talaria.json`'s `name`, else the repository's name without `talaria-app-`. Every path is
relative to the repo's root, which is the app's root. No file spells the app's slug: the instance
that installs the app decides it, and the surfaces read it from `useApp()`.

| file | written |
| :--- | :--- |
| `talaria.json` | always; a seed `talaria.json` keeps every key it declares |
| `app.ts` | always |
| `Work.svelte` | always |
| `Item.svelte` | always |
| `server.ts` | always |
| `mcp.ts` | always |
| `harnesses/tidy-title.ts` | with `--harness` |
| `harnesses/recorded/tidy-title.json` | with `--harness` |
| `.gitignore` | when the repo has none; an existing one gets the missing lines appended |
| `README.md` | when the repo has none; an existing one is kept |

## The app

A work surface with a list and a record page below it (`/x/<slug>/items/<id>`), a document-store API, and one MCP tool.

### `talaria.json`

```json
{
  "name": "Field Notes",
  "icon": "⬡",
  "version": "0.1.0",
  "description": "Field Notes — a Talaria app.",
  "surfaces": {
    "work": "Field Notes"
  }
}
```

### `app.ts`

```ts
// Field Notes — surfaces. Everything here comes from '@talaria/sdk' (+ svelte).
import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'

export default defineApp({ work: Work })
```

### `Work.svelte`

```svelte
<script lang="ts">
  import {
    Button,
    EmptyState,
    Input,
    QueryState,
    SkeletonRows,
    alert,
    appHref,
    errorMessage,
    useApp,
    useAppApi,
    useAppInvalidate,
    useAppQuery,
    useAppRoute,
  } from '@talaria/sdk'
  import ItemPage from './Item.svelte'

  // No slug anywhere: the host tells the surface which app it is, under
  // whatever slug this instance installed it as.
  type Item = { id: string; data: { title: string } }

  const app = useApp()
  const api = useAppApi()
  const invalidate = useAppInvalidate()
  const query = useAppQuery<{ items: Item[] }>('items')

  // The surface owns every route below it: /x/<slug> is the list, and
  // /x/<slug>/items/<id> one item — an address you can link, bookmark and
  // reach with Back.
  const route = useAppRoute()
  const open = $derived(route.match('items/:id'))

  let title = $state('')
  let saving = $state(false)

  const add = async () => {
    const t = title.trim()
    if (!t || saving) return
    saving = true
    try {
      await api.post('items', { title: t })
      title = ''
      await invalidate('items')
    } catch (e) {
      await alert({ title: 'Could not add the item', message: errorMessage(e) })
    } finally {
      saving = false
    }
  }

  const remove = async (id: string) => {
    try {
      await api.del(`items/${id}`)
      await invalidate('items')
    } catch (e) {
      await alert({ title: 'Could not remove the item', message: errorMessage(e) })
    }
  }
</script>

{#if open}
  <ItemPage id={open.id} />
{:else}
  <div class="h-full overflow-y-auto p-8">
    <div class="mx-auto w-full max-w-3xl">
      <div class="mb-6 flex items-center gap-3">
        <h1 class="flex-1 font-sans text-lg font-semibold text-fg">{app.manifest.name}</h1>
        <Input bind:value={title} placeholder="New item" size="sm" class="w-56" onkeydown={(e) => e.key === 'Enter' && void add()} />
        <Button size="sm" disabled={saving || !title.trim()} onclick={() => void add()}>Add</Button>
      </div>

      <!-- Loading, broken, or resolved: a failed read says so, with a Retry,
           and is never shown as "No items yet". -->
      <QueryState {query} isEmpty={(d) => d.items.length === 0} errorTitle="Could not load the items" errorVariant="compact">
        {#snippet skeleton()}
          <SkeletonRows rows={5} />
        {/snippet}
        {#snippet empty()}
          <EmptyState icon={app.manifest.icon} title="No items yet" hint="Add one to see the store and the API working" />
        {/snippet}
        {#snippet children(data)}
          <ul class="divide-y divide-line overflow-hidden rounded-lg border border-line bg-panel">
            {#each data.items as item (item.id)}
              <li class="flex items-center gap-3 px-4 py-3">
                <a href={appHref(`items/${item.id}`)} class="min-w-0 flex-1 truncate font-sans text-sm text-fg hover:underline">{item.data.title}</a>
                <button type="button" class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:text-danger" onclick={() => void remove(item.id)}>
                  Remove
                </button>
              </li>
            {/each}
          </ul>
        {/snippet}
      </QueryState>
    </div>
  </div>
{/if}
```

### `Item.svelte`

```svelte
<script lang="ts">
  import { QueryState, SkeletonRows, appHref, useAppQuery } from '@talaria/sdk'

  // One item, at /x/<slug>/items/<id>. Work.svelte routes here off the
  // address; a new id re-keys the read (the path is a getter).
  type Item = { id: string; data: { title: string }; createdAt: string; updatedAt: string }

  let { id }: { id: string } = $props()
  const query = useAppQuery<{ item: Item }>(() => `items/${encodeURIComponent(id)}`)
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-3xl">
    <a href={appHref()} class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:text-fg">← All items</a>
    <QueryState {query} errorTitle="Could not load the item" errorVariant="compact">
      {#snippet skeleton()}
        <SkeletonRows rows={2} />
      {/snippet}
      {#snippet children(data)}
        <h1 class="mt-4 font-sans text-lg font-semibold text-fg">{data.item.data.title}</h1>
        <p class="mt-1 font-mono text-[11px] text-muted">Added {new Date(data.item.createdAt).toLocaleString()}</p>
      {/snippet}
    </QueryState>
  </div>
</div>
```

### `server.ts`

```ts
// App server — mounted at /api/apps/<slug>/*. The host already authenticated
// the user and checked access; this file is TypeScript against @talaria/sdk/server.
import { defineAppServer, json, parseBody, z } from '@talaria/sdk/server'

const Create = z.object({
  title: z.string({ error: 'title is required' }).trim().min(1, 'title is required'),
})

export default defineAppServer({
  async fetch(request, ctx) {
    const { path, store } = ctx
    const method = request.method

    if (path === 'items' && method === 'GET') {
      return json({ items: await store.list('items') })
    }
    if (path === 'items' && method === 'POST') {
      const body = await parseBody(request, Create)
      if (body instanceof Response) return body
      return json({ item: await store.insert('items', body) })
    }
    const id = /^items\/([0-9a-f-]{36})$/.exec(path)?.[1]
    if (id && method === 'GET') {
      const item = await store.get('items', id)
      return item ? json({ item }) : json({ error: 'not found' }, { status: 404 })
    }
    if (id && method === 'DELETE') {
      return (await store.remove('items', id)) ? json({ ok: true }) : json({ error: 'not found' }, { status: 404 })
    }
    if (path === 'items') return json({ error: 'method not allowed' }, { status: 405, headers: { allow: 'GET, POST' } })
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

### `mcp.ts`

```ts
// MCP tools for agents — governed in Manage → MCP like any server.
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'items_list',
      description: 'List items stored by this app.',
      inputSchema: { type: 'object', properties: {} },
      async handler(_args, ctx) {
        const items = await ctx.store.list<{ title?: string }>('items', { limit: 200 })
        return items.map((row) => ({ id: row.id, title: row.data.title ?? '', updatedAt: row.updatedAt }))
      },
    },
  ],
})
```

## With `--harness`

One JSON activity harness, and a reply recorded for its one fixture, so `talaria-app eval` runs offline from the first commit.

### `harnesses/tidy-title.ts`

```ts
// An activity harness: a model call this app declares — a prompt, an output
// contract, a failure policy, and fixtures the fitness sweep scores candidate
// models on (Admin → Models → Fitness). One per file under harnesses/.
// `talaria-app eval` replays the fixtures against the replies recorded in
// harnesses/recorded/, or through the Talaria gateway with --gateway.
import { defineHarness, z } from '@talaria/sdk/server'

const TIDY = z.object({ title: z.string().min(1), changed: z.boolean() })

export default defineHarness<{ title: string }, z.infer<typeof TIDY>>({
  id: "field-notes:tidy-title",
  label: 'Tidy an item title',
  job: 'Rewrites a rough item title as a short, specific one.',
  requires: ['json', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Needs a model that returns JSON; one probed unable to is refused rather than asked.',
  },
  model: {},
  render: (input) => [
    {
      role: 'system',
      content:
        'Rewrite the item title as a short, specific title of at most 8 words. Reply with JSON: {"title": string, "changed": boolean}.',
    },
    { role: 'user', content: input.title },
  ],
  output: { kind: 'json', schema: TIDY },
  onFailure: 'null',
  evals: [
    {
      name: "a rambling title gets shorter",
      input: { title: 'so basically we need to remember to call the supplier about the late invoice from march' },
      check: (v) => (v.title.trim().split(/\s+/).length <= 8 ? null : `"${v.title}" is longer than 8 words`),
    },
  ],
})
```

### `harnesses/recorded/tidy-title.json`

```json
{
  "harness": "field-notes:tidy-title",
  "note": "Replies talaria-app eval replays offline, by fixture name. Re-record through the gateway with `talaria-app eval --gateway --model <id> --record`.",
  "model": "recorded by hand",
  "cases": {
    "a rambling title gets shorter": [
      {
        "text": "{\"title\":\"Call the supplier about the March invoice\",\"changed\":true}"
      }
    ]
  }
}
```

## The repo

The ignore list (the kit writes `.talaria/`) and a README that says how the app is checked.

### `.gitignore`

```gitignore
# talaria-app writes its work here; the kit lives outside the repo.
.talaria/
node_modules/
```

### `README.md`

```markdown
# Field Notes

A Talaria app: `talaria.json` is its manifest, `app.ts` its surfaces, `server.ts`
its API under `/api/apps/<slug>/`, `mcp.ts` the tools agents can call, and
`harnesses/` the model calls it declares.

It builds against `@talaria/sdk`, which the Talaria instance serves as a kit
(`GET /api/sdk/kit.tgz`). With the kit's `talaria-app` on the PATH:

    talaria-app check   # talaria.json, typecheck, the app's own tests
    talaria-app build   # the host's build recipe and load probe, run here
    talaria-app eval    # the harnesses' fixtures, against harnesses/recorded/

An admin installs it from this repository (Manage → Apps → Install from Git).
```
