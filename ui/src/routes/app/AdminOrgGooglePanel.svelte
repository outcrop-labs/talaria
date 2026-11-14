<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, errorMessage, getJson, getJsonOr } from '@/lib/fetch-json'
  import type { GoogleApiHealth } from '@/lib/google-apis'
  import { toastError } from '@/lib/toast.svelte'
  import AdminOrgGoogleTargets from './AdminOrgGoogleTargets.svelte'
  import AdminOrgGoogleWorkspace from './AdminOrgGoogleWorkspace.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'

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
    try {
      await delJson<{ ok: true }>('/api/integrations/google/org')
    } catch (e) {
      toastError('Disconnect failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  // Live probe of Drive / Calendar / Gmail — consent succeeds even with the
  // APIs disabled in Google Cloud Console, so "connected" alone proves nothing
  // about calls actually working. On demand, never on load (three real Google
  // requests per click).
  let apiResults = $state<GoogleApiHealth[] | null>(null)
  let probing = $state(false)
  let probeError = $state<string | null>(null)
  const probeApis = async () => {
    probing = true
    probeError = null
    try {
      // 409/502 carry an answer in the body ("not connected", "Google hiccup")
      // rather than a results list — render it, don't treat it as failure.
      const j = await getJsonOr<{ results?: GoogleApiHealth[]; message?: string }>('/api/integrations/google/org/health', [409, 502])
      if (!j.results) probeError = j.message ?? 'The probe failed. Try again.'
      else apiResults = j.results
    } catch (e) {
      probeError = errorMessage(e)
    } finally {
      probing = false
    }
  }

  const CONNECT = '/api/integrations/google/org/connect'

  const msg: Record<string, string> = {
    connected: 'Shared Google account connected.',
    denied: 'Connection canceled.',
    bad_state: 'Connection expired. Try again.',
    exchange_failed: 'Google rejected the connection. Try again.',
    forbidden: 'Admins only.',
    disabled: 'Google is not set up.',
  }
</script>

{#snippet mark()}<McpServerMark title="Google" domain="google.com" />{/snippet}

<SettingsSection
  title="Shared Google account"
  description="One Google account that agents without an owner work in."
  info="Agents that belong to the whole team create Docs, Sheets and Drive files in this account. A personal assistant uses its owner's own Google connection instead."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the shared Google account" errorVariant="compact" isEmpty={(d) => !d.available}>
      {#snippet skeleton()}
        <!-- Never show "Not connected" + Connect while the status is in flight —
             hold the connection row's shape until it resolves. -->
        <SettingsRow label="Shared account">
          {#snippet lead()}{@render mark()}{/snippet}
          <Skeleton class="h-8 w-24" />
        </SettingsRow>
      {/snippet}
      {#snippet empty()}
        <SettingsRow label="Shared account" hint="Set up the Google Cloud client above first.">
          {#snippet lead()}{@render mark()}{/snippet}
          {#snippet status()}<RowStatus status="idle">Not connected</RowStatus>{/snippet}
        </SettingsRow>
      {/snippet}
      {#snippet children(_data)}
        <!-- A failed status read must not reach the row below, which renders "Not
             connected" and a Connect button — reconnecting a connection that is
             already live re-consents the org. That is QueryState's error branch. -->
        <SettingsRow label="Shared account">
          {#snippet lead()}{@render mark()}{/snippet}
          {#snippet hint()}
            {#if data?.connected && data.email}
              <span class="font-mono text-xs">{data.email}</span>
            {:else if !data?.connected}
              Agents cannot use Drive or Docs until an account is connected.
            {/if}
          {/snippet}
          {#snippet status()}
            {#if probing}
              <RowStatus status="idle">Testing</RowStatus>
            {:else}
              <RowStatus status={data?.connected ? 'ok' : 'idle'}>{data?.connected ? 'Connected' : 'Not connected'}</RowStatus>
            {/if}
          {/snippet}
          {#if data?.connected}
            <SettingsRowMenu
              label="Actions for the shared Google account"
              items={[
                { label: 'Test access', disabled: probing, onSelect: () => void probeApis() },
                'sep',
                { label: 'Disconnect Google', danger: true, onSelect: () => void disconnect() },
              ]}
            />
          {:else}
            <SettingsRowFix href={CONNECT} label="Fix the shared Google account: connect it" />
            <SettingsRowMenu
              label="Actions for the shared Google account"
              items={[{ label: 'Connect Google', onSelect: () => window.location.assign(CONNECT) }]}
            />
          {/if}
        </SettingsRow>
        {#if data?.connected && (probeError || apiResults)}
          <div class="space-y-1.5 px-5 py-3 font-sans text-[13px] leading-5">
            {#if probeError}<div class="text-danger">{probeError}</div>{/if}
            {#each apiResults ?? [] as a (a.service)}
              <div class="flex items-center gap-2">
                <StatusDot status={a.state === 'ok' ? 'ok' : 'danger'} />
                {#if a.state === 'disabled'}
                  <!-- The one state an admin can fix from the linked console page. -->
                  <span class="text-fg">{a.name}</span>
                  <span class="text-muted">{a.detail}</span>
                  <a href={a.consoleUrl} target="_blank" rel="noreferrer" class="text-accent underline-offset-2 hover:underline">Turn it on ↗</a>
                {:else}
                  <span class="text-fg">{a.name}</span>
                  <span class="text-muted">{a.state === 'error' ? a.detail : 'Reachable'}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      {/snippet}
    </QueryState>
    {#if data?.connected}
      <AdminOrgGoogleWorkspace>
        {#snippet between()}
          {#if data}<AdminOrgGoogleTargets targets={data.targets} />{/if}
        {/snippet}
      </AdminOrgGoogleWorkspace>
    {/if}
    {#if flash}
      <p class={cn('px-5 py-3 font-sans text-[13px] leading-5', flash === 'connected' ? 'text-success' : 'text-danger')}>
        {msg[flash] ?? flash}
      </p>
    {/if}
  </SettingsGroup>
</SettingsSection>
