<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Check, Lock, Plug, X } from '@lucide/svelte'
  import type { LucideIcon as IconType } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { getList } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import { p } from '@/router'

  type ProbeState = 'ok' | 'auth' | 'unreachable' | 'error'
  interface Probe {
    state: ProbeState
    detail: string
  }
  const PROBE_UI: Record<ProbeState, { color: string; icon: IconType; label: string }> = {
    ok: { color: 'var(--theme-success)', icon: Check, label: 'Connected' },
    auth: { color: 'var(--theme-warning)', icon: Lock, label: 'Login required' },
    unreachable: { color: 'var(--theme-danger)', icon: X, label: 'Unreachable' },
    error: { color: 'var(--theme-danger)', icon: X, label: 'Error' },
  }
  interface McpServer {
    name: string
    url: string
    timeout: number | null
    extras: string[]
  }

  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['mcp-agents'],
    queryFn: (): Promise<Array<{ id: string; servers: McpServer[] }>> =>
      getList<{ id: string; servers: McpServer[] }>('/api/mcp', 'agents'),
  }))
  const serversOf = (agents: Array<{ id: string; servers: McpServer[] }>) => agents.find((a) => a.id === def.id)?.servers ?? []
  let probes = $state<Record<string, Probe | 'testing'>>({})
  let name = $state('')
  let url = $state('')
  let busy = $state(false)

  const test = async (s: McpServer) => {
    probes = { ...probes, [s.name]: 'testing' }
    const r = await fetch('/api/mcp/test', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ url: s.url, agentSlug: def.slug }) }).catch(() => null)
    const result: Probe = r?.ok ? ((await r.json()) as Probe) : { state: 'error', detail: 'test failed' }
    probes = { ...probes, [s.name]: result }
  }
  const edit = async (body: { add?: Array<{ name: string; url: string }>; remove?: string[] }) => {
    busy = true
    try {
      await fetch(`/api/fleet/defs/${def.id}/mcp`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ add: [], remove: [], apply: true, ...body }) })
      name = ''
      url = ''
      await qc.invalidateQueries({ queryKey: ['mcp-agents'] })
    } finally {
      busy = false
    }
  }
</script>

<div class="space-y-3">
  {#if busy}
    <Generating site="fleet/mcp-apply" label={`Applying MCP change: rolling ${def.displayName} so the new version takes effect`} lines={2} />
  {/if}
  <QueryState
    {query}
    errorTitle="Could not load MCP servers"
    errorVariant="compact"
    isEmpty={(agents) => serversOf(agents).length === 0}
  >
    {#snippet skeleton()}<SkeletonRows rows={2} class="py-2" />{/snippet}
    {#snippet empty()}<div class="text-sm text-muted">No MCP servers connected.</div>{/snippet}
    {#snippet children(agents)}
      <div class="divide-y divide-line" use:listStagger>
        {#each serversOf(agents) as s (s.name)}
          {@const probe = probes[s.name]}
          <div class="flex items-center gap-3 py-2.5 text-sm">
            <span class="w-28 shrink-0 truncate font-medium text-fg">{s.name}</span>
            {#if s.extras.includes('built-in')}
              <Chip class="shrink-0" title="The Talaria toolkit, attached to every managed agent automatically">
                built-in
              </Chip>
            {/if}
            {#if s.extras.includes('managed')}
              <a
                href={p('/mcp')}
                class="shrink-0 rounded border border-accent/50 bg-accent/10 px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-accent transition-colors hover:bg-accent/15"
                title="Attached from the org MCP registry — assignment, tool subsets, and credentials are managed on the MCP page"
              >
                org registry
              </a>
            {/if}
            <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">{s.url}</span>
            {#if probe === 'testing'}
              <WaitingMark site="fleet/mcp-roll" size={12} class="shrink-0 text-muted" />
            {:else if probe}
              {@const P = PROBE_UI[probe.state]}
              <span class="flex shrink-0 items-center gap-1 text-xs" style:color={P.color} title={probe.detail}>
                <P.icon size={13} /> {P.label}
              </span>
            {/if}
            {#if !s.extras.includes('managed')}
              <Button variant="ghost" size="xs" class="shrink-0 hover:text-accent" onclick={() => void test(s)}>
                Test
              </Button>
            {/if}
            {#if isAdmin && !s.extras.includes('built-in') && !s.extras.includes('managed')}
              <Button variant="ghost" size="xs" class="shrink-0 hover:text-danger" disabled={busy}
                onclick={async () => { if (await confirm({ title: 'Remove skill', message: `Remove "${s.name}"?`, confirmLabel: 'Remove', danger: true })) void edit({ remove: [s.name] }) }}>
                Remove
              </Button>
            {/if}
          </div>
        {/each}
      </div>
    {/snippet}
  </QueryState>
  {#if isAdmin}
    <div class="flex items-center gap-2 pt-1">
      <Input size="sm" bind:value={name} onkeydown={submitOnEnter(() => !busy && name.trim() && /^https?:\/\//.test(url.trim()) && void edit({ add: [{ name: name.trim(), url: url.trim() }] }))} placeholder="name" class="w-28" />
      <Input size="sm" bind:value={url} onkeydown={submitOnEnter(() => !busy && name.trim() && /^https?:\/\//.test(url.trim()) && void edit({ add: [{ name: name.trim(), url: url.trim() }] }))} placeholder="http://host:port/mcp" class="flex-1" />
      <Button size="sm" disabled={busy || !name.trim() || !/^https?:\/\//.test(url.trim())} onclick={() => void edit({ add: [{ name: name.trim(), url: url.trim() }] })}>
        <Plug size={14} class="mr-1.5" /> Add
      </Button>
    </div>
  {/if}
</div>
