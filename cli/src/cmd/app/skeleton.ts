// The starter `talaria app new` writes. One work surface with a list and a
// record page below it (/x/<slug>/items/<id>), a document-store API, and one
// MCP tool — enough to enable and extend. It is the pattern authors and
// agents copy, so it follows the host's rules: a failed read renders as a
// failure (QueryState), a failed write says so (catch → alert), a record has
// an address of its own (useAppRoute, appHref), and the client never spells
// the app's slug. ui/src/sdk/client-docs.test.ts compiles this output against
// the SDK.

export const APP_SLUG_RE = /^[a-z0-9][a-z0-9-]{0,63}$/

/** Slugs install refuses: the client router's config keys, which it would
 *  read as path segments under /x/<slug>. Rust's RESERVED_APP_SLUGS
 *  (talaria-users) is the install-side list; both match the shared fixture's
 *  `routes.reservedSlugs` (api/tests/it/fixtures/app-grants.json). */
export const RESERVED_APP_SLUGS: readonly string[] = ['layout', 'hooks', 'meta']

export function displayName(slug: string): string {
  return slug
    .split('-')
    .filter(Boolean)
    .map((part) => part.slice(0, 1).toUpperCase() + part.slice(1))
    .join(' ')
}

/** The files, by path under apps/<slug>/. The slug names the directory and
 *  nothing else: no file embeds it, because the instance that installs the
 *  app decides it (install derives it from the repo name). */
export function skeletonFiles(opts: { name: string; icon: string }): Record<string, string> {
  return {
    'talaria.json': `${JSON.stringify(
      {
        name: opts.name,
        icon: opts.icon,
        version: '0.1.0',
        description: `${opts.name} — a Talaria app.`,
        surfaces: { work: opts.name },
      },
      null,
      2,
    )}\n`,

    'app.ts': `// ${opts.name} — surfaces. Everything here comes from '@talaria/sdk' (+ svelte).
import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'

export default defineApp({ work: Work })
`,

    'Work.svelte': `<script lang="ts">
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
      await api.del(\`items/\${id}\`)
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
                <a href={appHref(\`items/\${item.id}\`)} class="min-w-0 flex-1 truncate font-sans text-sm text-fg hover:underline">{item.data.title}</a>
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
`,

    'Item.svelte': `<script lang="ts">
  import { QueryState, SkeletonRows, appHref, useAppQuery } from '@talaria/sdk'

  // One item, at /x/<slug>/items/<id>. Work.svelte routes here off the
  // address; a new id re-keys the read (the path is a getter).
  type Item = { id: string; data: { title: string }; createdAt: string; updatedAt: string }

  let { id }: { id: string } = $props()
  const query = useAppQuery<{ item: Item }>(() => \`items/\${encodeURIComponent(id)}\`)
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
`,

    'server.ts': `// App server — mounted at /api/apps/<slug>/*. The host already authenticated
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
    const id = /^items\\/([0-9a-f-]{36})$/.exec(path)?.[1]
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
`,

    'mcp.ts': `// MCP tools for agents — governed in Manage → MCP like any server.
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
`,
  }
}

/** A name as a harness id's family: lowercase words joined by dashes, the
 *  way a builtin spells `muse:ticket`. The id must be unique across the
 *  instance's enabled apps (the first slug in sort order keeps a contested
 *  one), so it carries the app's name rather than a fixed word every
 *  scaffolded app would share. */
export function harnessFamily(name: string): string {
  const family = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60)
  return family || 'app'
}

/** The starter harness: one JSON activity harness under `harnesses/`, with one
 *  fixture, and a reply recorded for that fixture so `talaria-app eval` runs
 *  offline from the first commit. By path under the app's root. */
export function harnessStarterFiles(opts: { name: string }): Record<string, string> {
  const id = `${harnessFamily(opts.name)}:tidy-title`
  const fixture = 'a rambling title gets shorter'
  return {
    'harnesses/tidy-title.ts': `// An activity harness: a model call this app declares — a prompt, an output
// contract, a failure policy, and fixtures the fitness sweep scores candidate
// models on (Admin → Models → Fitness). One per file under harnesses/.
// \`talaria-app eval\` replays the fixtures against the replies recorded in
// harnesses/recorded/, or through the Talaria gateway with --gateway.
import { defineHarness, z } from '@talaria/sdk/server'

const TIDY = z.object({ title: z.string().min(1), changed: z.boolean() })

export default defineHarness<{ title: string }, z.infer<typeof TIDY>>({
  id: ${JSON.stringify(id)},
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
      name: ${JSON.stringify(fixture)},
      input: { title: 'so basically we need to remember to call the supplier about the late invoice from march' },
      check: (v) => (v.title.trim().split(/\\s+/).length <= 8 ? null : \`"\${v.title}" is longer than 8 words\`),
    },
  ],
})
`,
    'harnesses/recorded/tidy-title.json': `${JSON.stringify(
      {
        harness: id,
        note: 'Replies talaria-app eval replays offline, by fixture name. Re-record through the gateway with `talaria-app eval --gateway --model <id> --record`.',
        model: 'recorded by hand',
        cases: {
          [fixture]: [{ text: JSON.stringify({ title: 'Call the supplier about the March invoice', changed: true }) }],
        },
      },
      null,
      2,
    )}\n`,
  }
}

/** What a plugin repo carries beyond the app: the ignore list and a README
 *  that says how the app is checked before a pull request. By path under the
 *  repo's root, which is the app's root. */
export function standaloneRepoFiles(opts: { name: string }): Record<string, string> {
  return {
    '.gitignore': `# talaria-app writes its work here; the kit lives outside the repo.
.talaria/
node_modules/
`,
    'README.md': `# ${opts.name}

A Talaria app: \`talaria.json\` is its manifest, \`app.ts\` its surfaces, \`server.ts\`
its API under \`/api/apps/<slug>/\`, \`mcp.ts\` the tools agents can call, and
\`harnesses/\` the model calls it declares.

It builds against \`@talaria/sdk\`, which the Talaria instance serves as a kit
(\`GET /api/sdk/kit.tgz\`). With the kit's \`talaria-app\` on the PATH:

    talaria-app check   # talaria.json, typecheck, the app's own tests
    talaria-app build   # the host's build recipe and load probe, run here
    talaria-app eval    # the harnesses' fixtures, against harnesses/recorded/

An admin installs it from this repository (Manage → Apps → Install from Git).
`,
  }
}
