<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getJson } from '@/lib/fetch-json'
  import AdminOrgGoogleTargets from './AdminOrgGoogleTargets.svelte'

  interface OrgGoogle {
    available: boolean
    connected: boolean
    email: string | null
    connectedAt: string | null
    targets: { driveFolderId: string | null; calendarId: string | null; sendAs: string | null }
  }

  // The shared org Google account general fleet agents act as for Drive/Docs.
  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['org-google'],
    queryFn: (): Promise<OrgGoogle> => getJson<OrgGoogle>('/api/integrations/google/org'),
  }))
  const data = $derived(query.data)
  let flash = $state<string | null>(null)
  $effect(() => {
    const p = new URLSearchParams(window.location.search).get('googleOrg')
    if (!p) return
    flash = p
    window.history.replaceState({}, '', window.location.pathname)
    if (p === 'connected') void qc.invalidateQueries({ queryKey: ['org-google'] })
    const t = setTimeout(() => (flash = null), 4000)
    return () => clearTimeout(t)
  })

  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect the org Google account? General agents lose Drive/Docs access.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/integrations/google/org', { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  const msg: Record<string, string> = {
    connected: 'Org Google account connected.',
    denied: 'Connection cancelled.',
    bad_state: 'Connection expired. Try again.',
    exchange_failed: 'Google rejected the connection. Try again.',
    forbidden: 'Admins only.',
    disabled: 'Google integration is not configured.',
  }
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Organization Google account"
    info={"A single shared Google account the fleet builds in. General agents (no personal owner) create Docs, Sheets, and Drive files here; a personal assistant instead acts as its owner’s own connected Google."}
  />
  {#if query.isPending}
    <!-- Never show "Not connected" + Connect while the status is in flight —
         hold the connection row's shape until it resolves. -->
    <div class="flex items-center gap-3 rounded-md border border-line p-4">
      <Skeleton class="h-9 w-9 shrink-0" />
      <div class="min-w-0 flex-1 space-y-1.5">
        <Skeleton class="h-3 w-36 rounded-full" />
        <Skeleton class="h-2.5 w-52 rounded-full" />
      </div>
      <Skeleton class="h-7 w-24 shrink-0" />
    </div>
  {:else if !data}
    <!-- Without this branch a failed status read fell through to the row
         below, which renders "Not connected" and a Connect button — and
         reconnecting a connection that is already live re-consents the org. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load the org Google account"
      onRetry={() => void query.refetch()}
    />
  {:else if !data.available}
    <!-- Points at the client panel directly above rather than a dead end —
         "isn't configured on this server" sent admins to a .env they no
         longer need to touch. -->
    <div class="text-xs text-muted">Set up the Google OAuth client above first, then connect the org account here.</div>
  {:else}
    <div class="flex items-center gap-3 rounded-md border border-line p-4">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-lg">🏢</div>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5 text-sm font-medium text-fg">
          Shared Drive &amp; Docs
          {#if data?.connected}<span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>{/if}
        </div>
        <div class="truncate font-mono text-[11px] text-muted">
          {data?.connected ? `Connected${data.email ? ` as ${data.email}` : ''}` : 'Not connected'}
        </div>
      </div>
      {#if data?.connected}
        <Button variant="danger-outline" size="sm" onclick={() => void disconnect()}>Disconnect</Button>
      {:else}
        <a href="/api/integrations/google/org/connect" class={buttonClasses({ size: 'sm' })}>Connect</a>
      {/if}
    </div>
  {/if}
  {#if data?.connected}<AdminOrgGoogleTargets targets={data.targets} />{/if}
  {#if flash}
    <div class={cn('mt-3 text-xs', flash === 'connected' ? 'text-success' : 'text-danger')}>
      {msg[flash] ?? flash}
    </div>
  {/if}
</Panel>
