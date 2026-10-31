<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { GitBranch } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideInstallRequest, fetchInstallRequests, type AppInstallRequest, type KeptDataConflict } from './apps'

  // Agents' requests to install the app they built (`request_app_install`).
  // THIS is where an admin decides: Install runs the same credentialed
  // install as Install from Git and records the plugin (the agent maintains
  // it, the ticket's person owns it, the ticket's board takes its
  // maintenance tickets). The app lands disabled — enabling it and granting
  // its surfaces stay here, on its card. Every outcome is a line on the
  // requesting ticket.
  const qc = useQueryClient()
  // A failed poll must never read as "nothing waiting on you".
  const query = createQuery(() => ({
    queryKey: ['app-install-requests'],
    queryFn: fetchInstallRequests,
    refetchInterval: 60_000,
  }))
  const requests = $derived(query.data ?? [])
  let busy = $state<string | null>(null)
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  /** The last decision's outcome, per request, as it was written on the ticket. */
  let outcome = $state<{ ok: boolean; text: string } | null>(null)
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

{#if query.isError && query.data === undefined}
  <QueryError
    variant="inline"
    error={query.error}
    title="Could not load install requests"
    onRetry={() => void query.refetch()}
  />
{/if}
{#if requests.length > 0 || outcome}
  <div class="mb-4 space-y-2" data-install-requests>
    <div class="flex items-baseline gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Install requests</span>
      <span class="text-xs text-muted">agents asked for the app they built to be installed; it lands disabled</span>
    </div>
    {#each requests as r (r.id)}
      {@const conflict = conflicts[r.id]}
      <div class="space-y-2 rounded-md border border-warning/30 px-3 py-2 text-sm" data-install-request>
        <div class="flex flex-wrap items-center gap-2">
          <span class="flex min-w-0 flex-1 items-center gap-1.5">
            <GitBranch size={12} class="shrink-0 text-muted" />
            <span class="truncate font-mono text-[13px] text-fg" title={r.url}>{r.repo}</span>
            <span class="shrink-0 font-mono text-[11px] text-muted">@ {r.ref ?? 'default branch'} on {r.hostId}</span>
          </span>
          <Chip title="The Developer Agent that asked, and will maintain it">{r.agentName ?? r.agentModel}</Chip>
          {#if r.ticketRef}<Chip title={`The ticket it was built for, on ${r.boardName ?? 'its board'}: maintenance tickets go there`}>{r.ticketRef}</Chip>{/if}
          {#if r.ownerName}<Chip title="Who the app is for">for {r.ownerName}</Chip>{/if}
          <Button size="sm" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
            {busy === r.id ? 'Installing…' : 'Install'}
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
        {#if r.why}<div class="text-xs text-muted">{r.why}</div>{/if}
        {#if conflict}
          <div transition:slide={{ duration: 150 }} class="flex flex-wrap items-center gap-2 text-xs" data-install-conflict>
            <span class="min-w-0 flex-1 text-warning">
              The slug "{conflict.slug}" holds data kept from {conflict.keptFrom ?? 'an earlier install that did not record its repository'}.
            </span>
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'adopt')}>Adopt it</Button>
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'wipe')}>Wipe it first</Button>
          </div>
        {/if}
        {#if rejecting === r.id}
          <div transition:slide={{ duration: 150 }} class="flex items-center gap-2">
            <Input
              size="sm"
              bind:value={rejectNote}
              maxlength={500}
              placeholder="Why not? Posted on the requesting ticket (optional)"
              class="min-w-0 flex-1"
            />
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'reject')}>Reject request</Button>
          </div>
        {/if}
      </div>
    {/each}
    {#if outcome}
      <div transition:slide={{ duration: 150 }} class={`text-xs ${outcome.ok ? 'text-muted' : 'text-danger'}`} data-install-outcome>
        {outcome.text}
      </div>
    {/if}
  </div>
{/if}
