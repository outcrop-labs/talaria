<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
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
    <SettingsRow label={name}>
      {#snippet hint()}
        <div>
          <span title={r.prs.length > 0 ? 'Merged pull requests that moved the branch' : undefined}>
            Update{r.prs.length > 0 ? ` from merged ${r.prs.map((n) => `#${n}`).join(', ')}` : ''}
          </span>
          {#if r.maintainer}· <span title="The agent that maintains it">Maintained by {r.maintainer}</span>{/if}
        </div>
        <div class="font-mono text-xs leading-5">{shortSha(r.fromSha)} → {shortSha(r.targetSha)}</div>
      {/snippet}
      <Button size="sm" variant="outline" class="w-24" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
        {busy === r.id ? 'Updating' : 'Update'}
      </Button>
      <SettingsRowMenu
        label={`Actions for the ${name} update`}
        items={[
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
