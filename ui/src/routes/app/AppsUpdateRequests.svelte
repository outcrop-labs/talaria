<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideUpdateRequest, fetchUpdateRequests, shortSha, type AppUpdateRequest, type InstalledApp } from './apps'
  import AppsMark from './AppsMark.svelte'

  /** The installed apps, so a request shows its app's name and mark. */
  let { apps }: { apps: InstalledApp[] } = $props()

  // Plugin updates waiting for an admin (`app_update`): the maintenance pass
  // raises one when a plugin's branch moves past the commit installed — a
  // merged PR. Update builds and probes the new version beside the live one,
  // switches to it, and rolls back if it fails; a plugin set to automatic
  // updates never waits here. Every outcome is a line on the plugin's ticket.
  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['app-update-requests'],
    queryFn: fetchUpdateRequests,
    refetchInterval: 60_000,
  }))
  const requests = $derived(query.data ?? [])
  let busy = $state<string | null>(null)
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  let outcome = $state<{ ok: boolean; text: string } | null>(null)

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

{#if query.isError && query.data === undefined}
  <QueryError variant="inline" error={query.error} title="Could not load plugin updates" onRetry={() => void query.refetch()} />
{/if}
{#if requests.length > 0 || outcome}
  <div data-update-requests>
    <SettingsSection title="Updates to approve" description="A merged change is waiting. A version that fails to load is rolled back.">
      {#if requests.length > 0}
        <SettingsGroup>
          {#each requests as r (r.id)}
            {@const app = apps.find((a) => a.slug === r.appSlug)}
            <div data-update-request>
              {#snippet mark()}{#if app}<AppsMark icon={app.icon} tile />{/if}{/snippet}
              <SettingsRow label={app?.name ?? r.appSlug} lead={app ? mark : undefined}>
                {#snippet meta()}
                  <span class="shrink-0 font-mono text-xs text-muted">{shortSha(r.fromSha)} → {shortSha(r.targetSha)}</span>
                {/snippet}
                {#snippet hint()}
                  {#if r.prs.length > 0}
                    <span title="Merged pull requests that moved the branch">Merged {r.prs.map((n) => `#${n}`).join(', ')}</span>
                  {/if}
                  {#if r.prs.length > 0 && r.maintainer}·{/if}
                  {#if r.maintainer}<span title="The agent that maintains it">Maintained by {r.maintainer}</span>{/if}
                {/snippet}
                <Button size="sm" variant="outline" class="w-24" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
                  {busy === r.id ? 'Updating' : 'Update'}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy === r.id}
                  onclick={() => {
                    rejecting = rejecting === r.id ? null : r.id
                    rejectNote = ''
                  }}>Reject</Button
                >
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
        </SettingsGroup>
      {/if}
      {#if outcome}
        <div
          transition:slide={{ duration: 150 }}
          class={`font-sans text-[13px] leading-5 ${requests.length > 0 ? 'mt-2' : ''} ${outcome.ok ? 'text-muted' : 'text-danger'}`}
          data-update-outcome
        >
          {outcome.text}
        </div>
      {/if}
    </SettingsSection>
  </div>
{/if}
