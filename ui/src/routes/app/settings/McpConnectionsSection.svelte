<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** Per-user MCP connected accounts: servers the org registered in per-user
   *  auth mode. Connect yours (a token header, sealed at rest) and your
   *  assistant starts carrying the server — acting as YOU there. */
  interface McpConnection {
    id: string
    name: string
    label: string
    description: string | null
    requiredHeaders: Array<{ name: string; description: string | null; isSecret: boolean; placeholder: string | null }>
    authKind: 'oauth' | 'headers'
    connected: boolean
  }

  const qc = useQueryClient()
  // The section hides itself when the org runs no per-user servers. That is a
  // statement about the org, so only a REAL empty list may make it — a failed
  // read would otherwise quietly remove a connected account from view.
  const query = createQuery(() => ({
    queryKey: ['me-mcp'],
    queryFn: (): Promise<McpConnection[]> => getList<McpConnection>('/api/me/mcp', 'servers'),
  }))
  const data = $derived(query.data)
  const isPending = $derived(query.isPending)
  let connecting = $state<string | null>(null)
  let values = $state<Record<string, string>>({})

  // The OAuth popup announces completion — flip to "connected" live.
  $effect(() => {
    const onMsg = (e: MessageEvent) => {
      if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
        void qc.invalidateQueries({ queryKey: ['me-mcp'] })
      }
    }
    window.addEventListener('message', onMsg)
    return () => window.removeEventListener('message', onMsg)
  })

  const put = async (serverId: string, headers: Record<string, string> | null) => {
    await fetch('/api/me/mcp', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ serverId, headers }),
    })
    connecting = null
    values = {}
    await qc.invalidateQueries({ queryKey: ['me-mcp'] })
  }
</script>

{#if !(!isPending && data?.length === 0)}
  <Panel as="section" class="mt-4">
    <SectionHeader
      title="Tool accounts"
      info="MCP servers your org runs in per-user mode. Connect your own account (stored encrypted) and your assistant can use the server as you. Disconnect any time — the server drops off your assistant on the next config render."
    />
    {#if isPending}
      <SkeletonRows rows={2} />
    {:else if !data}
      <QueryError variant="inline" error={query.error} title="Could not load your tool accounts" onRetry={() => void query.refetch()} />
    {:else}
      <ul class="divide-y divide-line-subtle">
        {#each data ?? [] as s (s.id)}
          <li class="space-y-2 py-2.5">
            <div class="flex items-center gap-3">
              <div class="min-w-0 flex-1">
                <div class="text-sm text-fg">{s.label}</div>
                {#if s.description}<div class="truncate font-sans text-xs text-muted">{s.description}</div>{/if}
              </div>
              {#if s.connected}
                <span class="flex shrink-0 items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success">
                  <span aria-hidden="true" class="h-1.5 w-1.5 rounded-full bg-success"></span>
                  connected
                </span>
                <Button size="sm" variant="danger-outline" onclick={() => void put(s.id, null)}>
                  Disconnect
                </Button>
              {:else if s.authKind === 'oauth'}
                <Button
                  size="sm"
                  title="Sign in with the provider — your assistant then acts as you on this server"
                  onclick={() => void window.open(`/api/mcp/oauth/start?server=${s.id}&scope=me`, 'talaria-mcp-oauth', 'width=620,height=780')}
                >
                  Connect
                </Button>
              {:else}
                <Button size="sm" onclick={() => (connecting = connecting === s.id ? null : s.id)}>
                  Connect
                </Button>
              {/if}
            </div>
            {#if connecting === s.id}
              <div transition:slide={{ duration: 150 }} class="space-y-2 pl-1">
                <!-- Publisher-declared credentials drive the form; a server
                     without declarations falls back to one auth header. -->
                {#each s.requiredHeaders.length ? s.requiredHeaders : [{ name: 'Authorization', description: null, isSecret: true, placeholder: 'Bearer …' }] as h (h.name)}
                  <div class="flex items-end gap-2">
                    <span class="w-40 shrink-0 truncate pb-2 font-mono text-[11px] text-muted" title={h.name}>
                      {h.name}
                    </span>
                    <div class="min-w-0 flex-1">
                      <Input
                        size="sm"
                        type={h.isSecret ? 'password' : 'text'}
                        value={values[h.name] ?? ''}
                        oninput={(e) => (values = { ...values, [h.name]: e.currentTarget.value })}
                        placeholder={h.placeholder ?? ''}
                        autocomplete="off"
                      />
                      {#if h.description}<div class="mt-0.5 font-sans text-[11px] text-muted/80">{h.description}</div>{/if}
                    </div>
                  </div>
                {/each}
                <div class="flex justify-end">
                  <Button
                    size="sm"
                    disabled={!Object.values(values).some((v) => v.trim())}
                    onclick={() => void put(s.id, Object.fromEntries(Object.entries(values).filter(([, v]) => v.trim())))}
                  >
                    Connect
                  </Button>
                </div>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
{/if}
