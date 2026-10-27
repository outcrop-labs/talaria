<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { GitBranch } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideUpdateRequest, fetchUpdateRequests, shortSha, type AppUpdateRequest } from './apps'

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
  <div class="mb-4 space-y-2" data-update-requests>
    <div class="flex items-baseline gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plugin updates</span>
      <span class="text-xs text-muted">a merged change waits for your approval; a version that fails is rolled back</span>
    </div>
    {#each requests as r (r.id)}
      <div class="space-y-2 rounded-md border border-accent/30 px-3 py-2 text-sm" data-update-request>
        <div class="flex flex-wrap items-center gap-2">
          <span class="flex min-w-0 flex-1 items-center gap-1.5">
            <GitBranch size={12} class="shrink-0 text-muted" />
            <span class="truncate font-mono text-[13px] text-fg">{r.appSlug}</span>
            <span class="shrink-0 font-mono text-[11px] text-muted">{shortSha(r.fromSha)} → {shortSha(r.targetSha)}</span>
          </span>
          {#each r.prs as n (n)}<Chip title="A merged PR that moved the branch">#{n}</Chip>{/each}
          {#if r.maintainer}<Chip title="The Developer Agent that maintains it">{r.maintainer}</Chip>{/if}
          <Button size="sm" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
            {busy === r.id ? 'Updating…' : 'Update'}
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
        </div>
        {#if rejecting === r.id}
          <div transition:slide={{ duration: 150 }} class="flex items-center gap-2">
            <Input size="sm" bind:value={rejectNote} maxlength={500} placeholder="Why not? Posted on the plugin's ticket (optional)" class="min-w-0 flex-1" />
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'reject')}>Reject update</Button>
          </div>
        {/if}
      </div>
    {/each}
    {#if outcome}
      <div transition:slide={{ duration: 150 }} class={`text-xs ${outcome.ok ? 'text-muted' : 'text-danger'}`} data-update-outcome>{outcome.text}</div>
    {/if}
  </div>
{/if}
