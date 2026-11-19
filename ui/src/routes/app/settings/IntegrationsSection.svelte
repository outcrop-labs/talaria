<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowAction from '@/components/settings/SettingsRowAction.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, getJson } from '@/lib/fetch-json'
  import { googleBounceText, readGoogleBounce, type GoogleBounce } from '@/lib/google-bounce'
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
    /** The OAuth client that minted the stored grant (null until recorded). */
    clientId: string | null
    /** Why the connection needs a person — its client is configured nowhere
     *  here, or Google refused that client (in Google's words) — else null. */
    reconnectReason: string | null
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
  // Surface the callback outcome (?google=connected|denied|…, plus Google's
  // own `reason` code when it sent one) once, then clean the URL.
  let flash = $state<GoogleBounce | null>(null)
  $effect(() => {
    const bounce = readGoogleBounce(window.location.search, 'google')
    if (!bounce) return
    flash = bounce
    window.history.replaceState({}, '', window.location.pathname)
    if (bounce.status === 'connected') void qc.invalidateQueries({ queryKey: ['integration-google'] })
    const t = setTimeout(() => (flash = null), 4000)
    return () => clearTimeout(t)
  })

  // Set when a disconnect forgot the connection but did not revoke the grant
  // at Google (another stored connection shares it, or Google refused) — the
  // server's own sentence. Inline, not a toast: a toast cuts it to one line.
  let revokeNote = $state('')

  const disconnect = async () => {
    revokeNote = ''
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect Google? Your assistant loses mail, calendar, and Drive access until you reconnect.', confirmLabel: 'Disconnect', danger: true }))) return
    try {
      const r = await delJson<{ ok: true; revoked: boolean; revokeNote: string | null }>('/api/integrations/google')
      revokeNote = r.revokeNote ?? ''
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
        <SettingsRow label="Google Workspace" hint="Not set up on this server yet. Ask an admin to register the Google client under Organization, General.">
          {#snippet lead()}{@render googleMark()}{/snippet}
        </SettingsRow>
      {/snippet}
      {#snippet children(_data)}
        <!-- "Not connected" is a claim about the account, and a failed status
             read cannot make it. QueryState's error branch owns that. -->
        <SettingsRow label="Google Workspace" info={GOOGLE_INFO}>
          {#snippet lead()}{@render googleMark()}{/snippet}
          {#snippet hint()}
            {#if data?.connected}
              {data.email ?? 'Connected'}{data.connectedAt ? ` · connected ${relativeTime(data.connectedAt)}` : ''}
              <!-- The server's sentence, Google's words inside it when Google refused. -->
              {#if data.reconnectReason}<span class="block">{data.reconnectReason}</span>{/if}
            {:else}
              Mail, Calendar and Drive.
            {/if}
          {/snippet}
          {#snippet status()}
            {#if data?.connected && data.reconnectReason}
              <StatusDot status="warn" />Needs sign-in
            {:else}
              <StatusDot status={data?.connected ? 'ok' : 'idle'} />{data?.connected ? 'Connected' : 'Not connected'}
            {/if}
          {/snippet}
          {#if data?.connected}
            {#if data.reconnectReason}
              <SettingsRowAction label="Reconnect Google Workspace" href="/api/integrations/google/connect">Reconnect</SettingsRowAction>
            {/if}
            <SettingsRowMenu
              label="Actions for Google Workspace"
              items={[{ label: 'Disconnect Google', danger: true, onSelect: () => void disconnect() }]}
            />
          {:else}
            <SettingsRowAction label="Connect Google Workspace" href="/api/integrations/google/connect">Connect</SettingsRowAction>
            <SettingsRowMenu label="Actions for Google Workspace" items={[]} />
          {/if}
        </SettingsRow>
      {/snippet}
    </QueryState>
    <McpConnectionsSection />
  </SettingsGroup>
  {#if revokeNote}
    <!-- Under the card, not a toast: a toast cuts the server's sentence to one line. -->
    <p transition:slide={{ duration: 150 }} class="mt-2 font-sans text-[13px] leading-5 text-muted">{revokeNote}</p>
  {/if}
  {#if flash}
    <div
      transition:slide={{ duration: 150 }}
      class={cn('mt-2 font-sans text-[13px] leading-5', flash.status === 'connected' ? 'text-success' : 'text-danger')}
    >
      {googleBounceText(flash, flashText)}
    </div>
  {/if}
</SettingsSection>
