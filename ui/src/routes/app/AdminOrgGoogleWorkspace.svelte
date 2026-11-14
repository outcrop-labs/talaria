<script lang="ts">
  import type { Snippet } from 'svelte'
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import RowStatus from '@/components/admin/RowStatus.svelte'

  // The provisioned workspace: the org calendar + Shared Drive everyone at the
  // domain can reach, and the address each org agent sends mail from. One GET
  // paints it all; the buttons run the idempotent provisioning pass. Rows of
  // the shared Google account card.

  interface ProvisionState {
    readiness: { connected: boolean; email: string | null; calendarScope: boolean; driveScope: boolean }
    orgEmail: string | null
    calendarId: string | null
    sharedDriveId: string | null
    agents: Array<{ model: string; displayName: string; slug: string; alias: string | null; effective: string | null }>
  }
  type Outcome = { ok: true; state: string; id: string } | { ok: false; error: string; message: string }

  /** Rows the card places after the two resources and before the address
   *  list, whatever state this component's own read is in. */
  let { between }: { between?: Snippet } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['org-google-provision'],
    queryFn: (): Promise<ProvisionState> => getJson<ProvisionState>('/api/integrations/google/org/provision'),
  }))
  const data = $derived(query.data)

  let working = $state<'calendar' | 'drive' | null>(null)
  let results = $state<{ calendar?: Outcome; drive?: Outcome }>({})

  const run = async (what: 'calendar' | 'drive') => {
    working = what
    let j: { calendar?: Outcome; drive?: Outcome }
    try {
      j = await postJson<{ calendar?: Outcome; drive?: Outcome }>('/api/integrations/google/org/provision', { [what]: true })
    } catch (e) {
      // Per-item failures ride in a 200 body; a throw means the call itself
      // failed — show it as that item's outcome.
      const failed: Outcome = { ok: false, error: '', message: errorMessage(e) }
      j = what === 'calendar' ? { calendar: failed } : { drive: failed }
    }
    working = null
    results = { ...results, ...j }
    // Targets changed on success — the panel's connection/targets views follow.
    if (j[what]?.ok) await qc.invalidateQueries({ queryKey: ['org-google'] })
    await qc.invalidateQueries({ queryKey: ['org-google-provision'] })
  }

  const outcomeLine = (o: Outcome | undefined): { text: string; good: boolean } | null => {
    if (!o) return null
    if (o.ok) {
      const label = o.state === 'created' ? 'Created.' : o.state === 'shared' ? 'Shared with the domain.' : 'Already in place.'
      return { text: label, good: true }
    }
    return { text: o.message, good: false }
  }
  const driveOutcome = $derived(outcomeLine(results.drive))
  const calendarOutcome = $derived(outcomeLine(results.calendar))
  let showAddresses = $state(false)
</script>

{#snippet outcome(line: { text: string; good: boolean } | null)}
  {#if line}
    <span class={line.good ? 'text-success' : 'text-danger'}>{line.text || 'Done.'}</span>
  {/if}
{/snippet}

{#snippet resourceState(what: 'calendar' | 'drive', ready: boolean)}
  {#if working === what}
    <RowStatus status="idle">Checking</RowStatus>
  {:else}
    <RowStatus status={ready ? 'ok' : 'idle'}>{ready ? 'Ready' : 'Not set'}</RowStatus>
  {/if}
{/snippet}

<!-- Neither resource is required, so setting one up is a menu item, and so
     is checking one that is already there. Both run the same pass. -->
{#snippet action(what: 'calendar' | 'drive', ready: boolean, menuLabel: string)}
  <SettingsRowMenu
    label={menuLabel}
    items={[{ label: ready ? 'Check again' : 'Set up', disabled: working !== null, onSelect: () => void run(what) }]}
  />
{/snippet}

{#if query.isPending}
  <SettingsRow label="Team calendar"><Skeleton class="h-8 w-24" /></SettingsRow>
  <SettingsRow label="Team Drive"><Skeleton class="h-8 w-24" /></SettingsRow>
{:else if data}
  {#if data.readiness.connected && (!data.readiness.calendarScope || !data.readiness.driveScope)}
    <!-- The one-time wall: a connection granted before the provisioning scopes
         existed. Re-consent (the connect flow upserts over the live row). -->
    <p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">
      This connection is missing the permission to create a calendar and a shared drive.
      <a href="/api/integrations/google/org/connect" class="text-accent underline-offset-2 hover:underline">Reconnect the shared account</a>
      to grant it.
    </p>
  {/if}

  <SettingsRow label="Team calendar">
    {#snippet hint()}
      {#if calendarOutcome}
        {@render outcome(calendarOutcome)}
      {:else if data.calendarId}
        <a
          href={`https://calendar.google.com/calendar/u/0/r?cid=${encodeURIComponent(data.calendarId)}`}
          target="_blank"
          rel="noreferrer"
          class="break-all font-mono text-xs underline-offset-2 hover:text-accent hover:underline"
        >{data.calendarId} ↗</a>
      {:else}
        A calendar everyone on your domain can edit.
      {/if}
    {/snippet}
    {#snippet status()}{@render resourceState('calendar', !!data.calendarId)}{/snippet}
    {#if data.readiness.connected}{@render action('calendar', !!data.calendarId, 'Actions for the team calendar')}{/if}
  </SettingsRow>

  <SettingsRow label="Team Drive">
    {#snippet hint()}
      {#if driveOutcome}
        {@render outcome(driveOutcome)}
      {:else if data.sharedDriveId}
        <a
          href={`https://drive.google.com/drive/folders/${data.sharedDriveId}`}
          target="_blank"
          rel="noreferrer"
          class="underline-offset-2 hover:text-accent hover:underline"
        >Open in Google Drive ↗</a>
      {:else}
        A shared drive the team owns, open to your whole domain.
      {/if}
    {/snippet}
    {#snippet status()}{@render resourceState('drive', !!data.sharedDriveId)}{/snippet}
    {#if data.readiness.connected}{@render action('drive', !!data.sharedDriveId, 'Actions for the team Drive')}{/if}
  </SettingsRow>
{/if}

{@render between?.()}

{#if data?.agents.length}
  <div>
    <SettingsRow
      label="Agent email addresses"
      hint={`${data.agents.length} ${data.agents.length === 1 ? 'address' : 'addresses'}${data.orgEmail ? `, based on ${data.orgEmail}` : ''}.`}
      onclick={() => (showAddresses = !showAddresses)}
      expanded={showAddresses}
    />
    {#if showAddresses}
      <div transition:slide={{ duration: 150 }}>
        <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 border-t border-line-subtle px-5 py-4 font-sans text-[13px] leading-5">
          {#each data.agents as a (a.model)}
            <dt class="truncate text-muted">{a.displayName}</dt>
            <dd class="flex min-w-0 items-center gap-2">
              <span class="truncate font-mono text-xs text-fg">{a.effective ?? 'No address'}</span>
              {#if a.alias}<span class="shrink-0 text-xs leading-4 text-muted">Custom</span>{/if}
            </dd>
          {/each}
        </dl>
        <p class="px-5 pb-4 font-sans text-[13px] leading-5 text-muted">Change one agent's address on that agent's summary tab.</p>
      </div>
    {/if}
  </div>
{/if}
