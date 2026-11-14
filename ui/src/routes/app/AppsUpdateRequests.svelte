<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import FactsList from '@/components/admin/FactsList.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideUpdateRequest, shortSha, type AppUpdateRequest, type InstalledApp } from './apps'

  // Plugin updates waiting for an admin (`app_update`), as rows of the
  // approvals card (`AppsApprovals`, which reads the list): the maintenance
  // pass raises one when a plugin's branch moves past the commit installed —
  // a merged PR. Update builds and probes the new version beside the live one,
  // switches to it, and rolls back if it fails; a plugin set to automatic
  // updates never waits here. Every outcome is a line on the plugin's ticket.
  let {
    requests,
    apps,
    outcome = $bindable(null),
  }: {
    requests: AppUpdateRequest[]
    /** The installed apps, so a request shows its app's name. */
    apps: InstalledApp[]
    /** The last decision's outcome, as it was written on the ticket. */
    outcome?: { ok: boolean; text: string } | null
  } = $props()

  const qc = useQueryClient()
  let busy = $state<string | null>(null)
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  /** The rows whose source is open: what moved and where, by request id. */
  let open = $state<Record<string, boolean>>({})
  const ready = (r: AppUpdateRequest) =>
    r.prs.length === 0 ? 'A newer version is ready.' : `A newer version is ready, from ${r.prs.length} merged pull ${r.prs.length === 1 ? 'request' : 'requests'}.`
  const sourceOf = (r: AppUpdateRequest) => [
    ...(r.prs.length > 0 ? [{ label: 'Pull requests', value: r.prs.map((n) => `#${n}`).join(', ') }] : []),
    { label: 'Repository', value: r.repo, mono: true },
    { label: 'Branch', value: r.ref ?? 'Default branch', mono: r.ref !== null },
    { label: 'Installed commit', value: shortSha(r.fromSha), mono: true },
    { label: 'New commit', value: shortSha(r.targetSha), mono: true },
  ]

  const decide = async (r: AppUpdateRequest, action: 'approve' | 'reject') => {
    busy = r.id
    outcome = null
    try {
      const body = action === 'approve' ? { id: r.id, action } : { id: r.id, action, ...(rejectNote.trim() ? { note: rejectNote.trim() } : {}) }
      const res = await decideUpdateRequest(body)
      outcome = { ok: res.ok, text: res.line }
      if (action === 'reject') {
        rejecting = null
        rejectNote = ''
      }
    } catch (e) {
      outcome = { ok: false, text: errorMessage(e) }
    } finally {
      busy = null
      await qc.invalidateQueries({ queryKey: ['app-update-requests'] })
      await qc.invalidateQueries({ queryKey: ['admin-apps'] })
      await qc.invalidateQueries({ queryKey: ['admin-apps-updates'] })
    }
  }
</script>

{#each requests as r (r.id)}
  {@const name = apps.find((a) => a.slug === r.appSlug)?.name ?? r.appSlug}
  <div data-update-request>
    <SettingsRow label={name} onOpen={() => (open[r.id] = !open[r.id])} expanded={open[r.id] === true} openLabel={`Details for the ${name} update`}>
      {#snippet hint()}
        <div>{ready(r)}</div>
        <div>Update{#if r.maintainer}, maintained by <span title="The agent that maintains it">{r.maintainer}</span>{/if}</div>
      {/snippet}
      <Button size="sm" variant="outline" class="w-24" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
        {busy === r.id ? 'Updating' : 'Update'}
      </Button>
      <SettingsRowMenu
        label={`Actions for the ${name} update`}
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
    {#if rejecting === r.id}
      <div transition:slide={{ duration: 150 }}>
        <div class="flex items-center gap-3 border-t border-line-subtle px-5 py-4">
          <Input size="sm" bind:value={rejectNote} maxlength={500} placeholder="Reason, posted on the app's ticket (optional)" class="min-w-0 flex-1" />
          <Button size="md" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'reject')}>Reject update</Button>
        </div>
      </div>
    {/if}
  </div>
{/each}
