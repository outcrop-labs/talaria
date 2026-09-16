// The starter `talaria app new` writes. One work surface, a document-store
// API, and one MCP tool — enough to enable and extend.


export const APP_SLUG_RE = /^[a-z0-9][a-z0-9-]{0,63}$/

export function displayName(slug: string): string {
  return slug
    .split('-')
    .filter(Boolean)
    .map((part) => part.slice(0, 1).toUpperCase() + part.slice(1))
    .join(' ')
}

export function skeletonFiles(opts: { slug: string; name: string; icon: string }): Record<string, string> {
  const slugLit = JSON.stringify(opts.slug)
  const nameLit = JSON.stringify(opts.name)
  const iconLit = JSON.stringify(opts.icon)
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
  import { Button, EmptyState, Input, SkeletonRows, useAppInvalidate, useAppQuery, appApi } from '@talaria/sdk'

  const slug = ${slugLit}
  const api = appApi(slug)
  const invalidate = useAppInvalidate(slug)
  const query = useAppQuery<{ items: Array<{ id: string; data: { title: string } }> }>(slug, 'items')

  let title = $state('')
  let saving = $state(false)

  const add = async () => {
    const t = title.trim()
    if (!t || saving) return
    saving = true
    try {
      await api.post('items', { title: t })
      title = ''
      await invalidate()
    } finally {
      saving = false
    }
  }

  const remove = async (id: string) => {
    await api.del(\`items/\${id}\`)
    await invalidate()
  }
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-3xl">
    <div class="mb-6 flex items-center gap-3">
      <h1 class="flex-1 font-sans text-lg font-semibold text-fg">{${nameLit}}</h1>
      <Input bind:value={title} placeholder="New item" size="sm" class="w-56" onkeydown={(e) => e.key === 'Enter' && void add()} />
      <Button size="sm" disabled={saving || !title.trim()} onclick={() => void add()}>Add</Button>
    </div>

    {#if query.isLoading}
      <SkeletonRows rows={5} />
    {:else if !(query.data?.items.length)}
      <EmptyState icon={${iconLit}} title="No items yet" hint="Add one to see the store and the API working" />
    {:else}
      <ul class="divide-y divide-line overflow-hidden rounded-lg border border-line bg-panel">
        {#each query.data.items as item (item.id)}
          <li class="flex items-center gap-3 px-4 py-3">
            <div class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{item.data.title}</div>
            <button type="button" class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:text-danger" onclick={() => void remove(item.id)}>
              Remove
            </button>
          </li>
        {/each}
      </ul>
    {/if}
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
