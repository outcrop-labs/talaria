<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  interface GoogleStatus {
    available: boolean
    connected: boolean
    email: string | null
    scope: string[]
    connectedAt: string | null
  }

  // Connected accounts. Per-user OAuth: connecting grants Talaria (and the agents
  // working for you) offline access to build Google Docs/Sheets in YOUR Drive.

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['integration-google'],
    queryFn: (): Promise<GoogleStatus> => getJson<GoogleStatus>('/api/integrations/google'),
  }))
  const data = $derived(query.data)
  const isLoading = $derived(query.isLoading)
  // Surface the callback outcome (?google=connected|denied|) once, then clean the URL.
  let flash = $state<string | null>(null)
  $effect(() => {
    const p = new URLSearchParams(window.location.search).get('google')
    if (!p) return
    flash = p
    window.history.replaceState({}, '', window.location.pathname)
    if (p === 'connected') void qc.invalidateQueries({ queryKey: ['integration-google'] })
    const t = setTimeout(() => (flash = null), 4000)
    return () => clearTimeout(t)
  })

  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect Google? Agents and exports lose access to your Drive.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/integrations/google', { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['integration-google'] })
  }

  const flashText: Record<string, string> = {
    connected: 'Google account connected.',
    denied: 'Connection cancelled.',
    bad_state: 'Connection expired. Please try again.',
    exchange_failed: 'Google rejected the connection. Please try again.',
    disabled: 'Google integration is not configured.',
  }
</script>

<Panel as="section">
  <SectionHeader
    class="mb-4"
    title="Connected accounts"
    info="Connect Google to export docs and sheets into your Drive, and let the agents working for you build Google Docs on your behalf."
  />

  {#if isLoading}
    <!-- Never show "Not connected" + Connect while the status is unknown —
         that's a false state. Hold the row's shape until the query resolves. -->
    <div aria-hidden="true" class="flex items-center gap-3 rounded-md border border-line p-4">
      <Skeleton class="h-9 w-9 shrink-0 rounded-md" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-40 rounded-full" delay={0.12} />
        <Skeleton class="h-2.5 w-56 rounded-full" delay={0.24} />
      </div>
      <Skeleton class="h-9 w-24 rounded-lg" delay={0.12} />
    </div>
  {:else if !data}
    <!-- Same reason as the skeleton above: "Not connected" is a claim about
         the account, and a failed status read can't make it. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your connected accounts"
      onRetry={() => void query.refetch()}
    />
  {:else if !data.available}
    <div class="text-xs text-muted">Google integration isn’t configured on this server yet.</div>
  {:else}
    <div class="flex items-center gap-3 rounded-md border border-line p-4">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-lg">
        🗂️
      </div>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5 text-sm font-medium text-fg">
          Google Drive & Docs
          {#if data?.connected}<span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>{/if}
        </div>
        <div class="truncate font-mono text-[11px] text-muted">
          {data?.connected
            ? `Connected${data.email ? ` as ${data.email}` : ''}${data.connectedAt ? ` · ${relativeTime(data.connectedAt)}` : ''}`
            : 'Not connected'}
        </div>
      </div>
      {#if data?.connected}
        <Button variant="danger-outline" size="sm" onclick={() => void disconnect()}>
          Disconnect
        </Button>
      {:else}
        <a href="/api/integrations/google/connect" class={buttonClasses({ size: 'sm' })}>
          Connect
        </a>
      {/if}
    </div>
  {/if}

  {#if flash}
    <div class={cn('mt-3 text-xs', flash === 'connected' ? 'text-success' : 'text-danger')}>
      {flashText[flash] ?? flash}
    </div>
  {/if}
</Panel>
