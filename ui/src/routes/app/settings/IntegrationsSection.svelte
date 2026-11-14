<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import McpServerMark from '@/routes/app/McpServerMark.svelte'
  import McpConnectionsSection from './McpConnectionsSection.svelte'

  interface GoogleStatus {
    available: boolean
    connected: boolean
    email: string | null
    scope: string[]
    connectedAt: string | null
  }

  // Connected accounts, one list: Google Workspace, then the tool accounts
  // (`McpConnectionsSection`, rows of the same card). Per-user OAuth:
  // connecting grants Talaria (and the agents working for you) offline access
  // to YOUR Google Workspace — mail, calendar, and Drive. Your assistant acts
  // strictly as you; outbound mail and invites it drafts wait for your
  // approval (confirm-sends).
  //
  // Every row has a mark on the one tile: Google's here, and for a tool
  // account the mark the Tools page shows for that server, or the neutral
  // glyph when this viewer cannot read the registry.
  const GOOGLE_INFO = 'Your assistant acts as you. Mail and invites it drafts wait for your approval in the Inbox.'
  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['integration-google'],
    queryFn: (): Promise<GoogleStatus> => getJson<GoogleStatus>('/api/integrations/google'),
  }))
  const data = $derived(query.data)
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
      toastError('Disconnect failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['integration-google'] })
  }

  const flashText: Record<string, string> = {
    connected: 'Google account connected.',
    denied: 'Connection canceled.',
    bad_state: 'Connection expired. Please try again.',
    exchange_failed: 'Google rejected the connection. Please try again.',
    disabled: 'Google integration is not configured.',
  }
</script>

{#snippet googleMark()}<McpServerMark title="Google" domain="google.com" />{/snippet}

<SettingsSection
  title="Accounts"
  info="Your sign-ins are stored encrypted. Disconnect an account any time, and a tool drops off your assistant on its next restart."
>
  <SettingsGroup>
    <QueryState
      query={query}
      errorTitle="Could not load your connected accounts"
      errorVariant="compact"
      isEmpty={(d) => !d.available}
    >
      {#snippet skeleton()}
        <!-- Never show "Not connected" + Connect while the status is unknown:
             that is a false state. Hold the row's shape until the query resolves. -->
        <div aria-hidden="true" class="flex min-h-[52px] items-center gap-3 px-5 py-3">
          <Skeleton class="h-8 w-8 shrink-0 rounded-md" />
          <div class="min-w-0 flex-1 space-y-2">
            <Skeleton class="h-3 w-40 rounded-full" />
            <Skeleton class="h-2.5 w-56 rounded-full" />
          </div>
          <Skeleton class="h-3 w-24 rounded-full" />
        </div>
      {/snippet}
      {#snippet empty()}
        <!-- An unregistered client is the ADMIN's gap to close. -->
        <SettingsRow label="Google Workspace" hint="Not set up on this server yet. Ask an admin to register the Google client in Admin.">
          {#snippet lead()}{@render googleMark()}{/snippet}
        </SettingsRow>
      {/snippet}
      {#snippet children(_data)}
        <!-- "Not connected" is a claim about the account, and a failed status
             read cannot make it. QueryState's error branch owns that. -->
        <SettingsRow
          label="Google Workspace"
          info={GOOGLE_INFO}
          hint={data?.connected
            ? `${data.email ?? 'Connected'}${data.connectedAt ? ` · connected ${relativeTime(data.connectedAt)}` : ''}`
            : 'Mail, Calendar and Drive.'}
        >
          {#snippet lead()}{@render googleMark()}{/snippet}
          {#snippet status()}
            <StatusDot status={data?.connected ? 'ok' : 'idle'} />{data?.connected ? 'Connected' : 'Not connected'}
          {/snippet}
          {#if data?.connected}
            <SettingsRowMenu
              label="Actions for Google Workspace"
              items={[{ label: 'Disconnect Google', danger: true, onSelect: () => void disconnect() }]}
            />
          {:else}
            <!-- Not connected is a choice, not a fault: the same quiet action a fix has, under its own word. -->
            <SettingsRowFix text="Connect" label="Connect Google Workspace" href="/api/integrations/google/connect" />
            <SettingsRowMenu label="Actions for Google Workspace" items={[]} />
          {/if}
        </SettingsRow>
      {/snippet}
    </QueryState>
    <McpConnectionsSection />
  </SettingsGroup>
  {#if flash}
    <div
      transition:slide={{ duration: 150 }}
      class={cn('mt-2 font-sans text-[13px] leading-5', flash === 'connected' ? 'text-success' : 'text-danger')}
    >
      {flashText[flash] ?? flash}
    </div>
  {/if}
</SettingsSection>
