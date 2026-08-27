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
  import { delJson, errorMessage, getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { pushToast } from '@/lib/toast.svelte'

  interface GoogleStatus {
    available: boolean
    connected: boolean
    email: string | null
    scope: string[]
    connectedAt: string | null
  }

  // Connected accounts. Per-user OAuth: connecting grants Talaria (and the
  // agents working for you) offline access to YOUR Google Workspace — mail,
  // calendar, and Drive. Your assistant acts strictly as you; outbound mail
  // and invites it drafts wait for your approval (confirm-sends).
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
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect Google? Your assistant loses mail, calendar, and Drive access until you reconnect.', confirmLabel: 'Disconnect', danger: true }))) return
    try {
      await delJson<{ ok: true }>('/api/integrations/google')
    } catch (e) {
      // Nothing inline to hold it: the status row still claims "connected",
      // so the toast is what says the disconnect didn't happen.
      pushToast({ title: 'Disconnect failed', body: errorMessage(e), tone: 'danger' })
      return
    }
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
    info="Connect your Google Workspace account to give your assistant mail, calendar, and Drive access. It acts as you, and outbound mail and invites wait for your approval."
  />

  {#if isLoading}
    <!-- Never show "Not connected" + Connect while the status is unknown —
         that's a false state. Hold the row's shape until the query resolves. -->
    <div aria-hidden="true" class="flex items-center gap-3 rounded-md border border-line p-4">
      <Skeleton class="h-9 w-9 shrink-0 rounded-md" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-40 rounded-full" />
        <Skeleton class="h-2.5 w-56 rounded-full" />
      </div>
      <Skeleton class="h-9 w-24 rounded-lg" />
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
    <!-- An unregistered client is the ADMIN's gap to close — the old copy
         dead-ended here with no hint of whose fix it was. -->
    <div class="text-xs text-muted">Google isn’t set up on this server yet. Ask an admin to register the Google client in Admin → Org.</div>
  {:else}
    <div class="flex items-center gap-3 rounded-md border border-line p-4">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-lg">
        ✉️
      </div>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5 text-sm font-medium text-fg">
          Google Workspace
          {#if data?.connected}<span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>{/if}
        </div>
        <div class="truncate font-mono text-[11px] text-muted">
          {data?.connected
            ? `Connected${data.email ? ` as ${data.email}` : ''}${data.connectedAt ? ` · ${relativeTime(data.connectedAt)}` : ''}`
            : 'Gmail · Calendar · Drive (not connected)'}
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
    <p class="mt-2 max-w-prose text-xs text-muted">
      Your assistant reads your mail and calendar live, finds Drive files, and drafts emails and events as you. Every send waits for your approval in the Inbox. Also powers doc/sheet export into your Drive.
    </p>
  {/if}

  {#if flash}
    <div transition:slide={{ duration: 150 }} class={cn('mt-3 text-xs', flash === 'connected' ? 'text-success' : 'text-danger')}>
      {flashText[flash] ?? flash}
    </div>
  {/if}
</Panel>
