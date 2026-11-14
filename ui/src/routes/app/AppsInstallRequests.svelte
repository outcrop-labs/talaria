<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import FactsList from '@/components/admin/FactsList.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideInstallRequest, type AppInstallRequest, type KeptDataConflict } from './apps'

  // Agents' requests to install the app they built (`request_app_install`),
  // as rows of the approvals card (`AppsApprovals`, which reads the list).
  // THIS is where an admin decides: Install runs the same credentialed
  // install as Install from Git and records the plugin (the agent maintains
  // it, the ticket's person owns it, the ticket's board takes its
  // maintenance tickets). The app lands disabled — enabling it and granting
  // its surfaces stay here, on its card. Every outcome is a line on the
  // requesting ticket.
  let {
    requests,
    outcome = $bindable(null),
  }: {
    requests: AppInstallRequest[]
    /** The last decision's outcome, as it was written on the ticket. */
    outcome?: { ok: boolean; text: string } | null
  } = $props()

  const qc = useQueryClient()
  /** The app's name as a person reads it. A request carries no name of its
   *  own before the install reads the manifest, so it is the repository's
   *  last segment in words; the repository itself is in the row's details. */
  const nameOf = (r: AppInstallRequest) => {
    const words = (r.repo.split('/').pop() || r.repo).replace(/[-_.]+/g, ' ').trim()
    return words ? words.charAt(0).toUpperCase() + words.slice(1) : r.repo
  }
  let busy = $state<string | null>(null)
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  /** The rows whose source is open: where the app comes from, by request id. */
  let open = $state<Record<string, boolean>>({})
  const sourceOf = (r: AppInstallRequest) => [
    { label: 'Repository', value: r.url, mono: true },
    { label: 'Branch', value: r.ref ?? 'Default branch', mono: r.ref !== null },
    { label: 'Git host', value: r.hostId, mono: true },
    ...(r.ticketRef ? [{ label: 'Ticket', value: r.boardName ? `${r.ticketRef} on ${r.boardName}` : r.ticketRef }] : []),
  ]
  /** Kept data from another repository stands in the way: adopt or wipe. */
  let conflicts = $state<Record<string, KeptDataConflict>>({})

  const decide = async (r: AppInstallRequest, action: 'approve' | 'reject', keptData?: 'adopt' | 'wipe') => {
    busy = r.id
    outcome = null
    try {
      const body =
        action === 'approve'
          ? { id: r.id, action, ...(keptData ? { keptData } : {}) }
          : { id: r.id, action, ...(rejectNote.trim() ? { note: rejectNote.trim() } : {}) }
      const res = await decideInstallRequest(body)
      if (res.conflict) {
        conflicts[r.id] = res.conflict
        return
      }
      delete conflicts[r.id]
      outcome = { ok: res.ok, text: res.warning ? `${res.line} (${res.warning})` : res.line }
      if (action === 'reject') {
        rejecting = null
        rejectNote = ''
      }
    } catch (e) {
      outcome = { ok: false, text: errorMessage(e) }
    } finally {
      busy = null
      await qc.invalidateQueries({ queryKey: ['app-install-requests'] })
      await qc.invalidateQueries({ queryKey: ['admin-apps'] })
      await qc.invalidateQueries({ queryKey: ['apps'] })
    }
  }
</script>

{#each requests as r (r.id)}
  {@const conflict = conflicts[r.id]}
  <div data-install-request>
    <SettingsRow label={nameOf(r)} onOpen={() => (open[r.id] = !open[r.id])} expanded={open[r.id] === true} openLabel={`Details for ${nameOf(r)}`}>
      {#snippet hint()}
        {#if r.why}<div>{r.why}</div>{/if}
        <div>
          New app, requested by <span title="The agent that asked, and will maintain it">{r.agentName ?? r.agentModel}</span>{#if r.ownerName}{' '}for <span title="Who the app is for">{r.ownerName}</span>{/if}
        </div>
      {/snippet}
      <Button size="sm" variant="outline" class="w-24" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
        {busy === r.id ? 'Installing' : 'Install'}
      </Button>
      <SettingsRowMenu
        label={`Actions for ${nameOf(r)}`}
        items={[
          { label: open[r.id] ? 'Hide details' : 'Show details', onSelect: () => (open[r.id] = !open[r.id]) },
          {
            label: rejecting === r.id ? 'Cancel rejection' : 'Reject',
            disabled: busy === r.id,
            onSelect: () => {
              rejecting = rejecting === r.id ? null : r.id
              rejectNote = ''
            },
          },
        ]}
      />
    </SettingsRow>
    {#if open[r.id]}
      <div transition:slide={{ duration: 150 }} data-request-details>
        <FactsList facts={sourceOf(r)} class="pt-0" />
      </div>
    {/if}
    {#if conflict}
      <div transition:slide={{ duration: 150 }}>
        <div class="flex flex-wrap items-center gap-3 border-t border-line-subtle px-5 py-4" data-install-conflict>
          <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-warning">
            The name "{conflict.slug}" still holds data from {conflict.keptFrom ?? 'an earlier install that did not record its repository'}.
          </span>
          <Button size="sm" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'adopt')}>Keep that data</Button>
          <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'wipe')}>Delete it first</Button>
        </div>
      </div>
    {/if}
    {#if rejecting === r.id}
      <div transition:slide={{ duration: 150 }}>
        <div class="flex items-center gap-3 border-t border-line-subtle px-5 py-4">
          <Input size="sm" bind:value={rejectNote} maxlength={500} placeholder="Reason, posted on the ticket (optional)" class="min-w-0 flex-1" />
          <Button size="md" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'reject')}>Reject request</Button>
        </div>
      </div>
    {/if}
  </div>
{/each}
