<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { errorMessage, getList, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** Agent repo creation: the pending request queue across every git host
   *  (agents propose, THIS is where humans ratify — approval creates the repo
   *  on the host the request names, at the visibility chosen here, protects
   *  its default branch against force-push and deletion unless unticked, and
   *  grants it to the requester; a rejection posts its reason on the
   *  requesting ticket). Which orgs a host takes requests for is set on the
   *  host's own card. */
  const qc = useQueryClient()
  type Visibility = 'private' | 'public'
  interface RepoReq {
    id: string
    host: string
    agentModel: string
    org: string
    name: string
    why: string
    visibility?: Visibility
    appSlug?: string | null
    createdAt: string
  }
  // This is the humans-ratify queue. An empty render means "nothing waiting on
  // you" — a failed poll must never say that, or an agent's repo request sits
  // unseen for ever.
  const requestsQuery = createQuery(() => ({
    queryKey: ['repo-requests'],
    queryFn: (): Promise<RepoReq[]> => getList<RepoReq>('/api/workbench/repo-requests', 'requests'),
    refetchInterval: 60_000,
  }))
  const requests = $derived(requestsQuery.data ?? [])
  const requestsFailed = $derived(requestsQuery.isError && requestsQuery.data === undefined)
  // The approver's visibility per request; absent = what the agent asked for.
  let chosen = $state<Record<string, Visibility>>({})
  const visibilityOf = (r: RepoReq): Visibility => chosen[r.id] ?? r.visibility ?? 'private'
  // Default-branch protection (no force-push, no deletion — nothing else) is
  // on unless the approver unticks it.
  let unprotected = $state<Record<string, boolean>>({})
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  let decideError = $state<string | null>(null)
  /** The last approval's word on the default branch: a host may refuse to
   *  protect it (GitHub does for a private repo on a free plan), and the repo
   *  exists anyway. */
  let decided = $state<string | null>(null)
  const decide = async (r: RepoReq, action: 'approve' | 'reject') => {
    decideError = null
    decided = null
    try {
      const body =
        action === 'approve'
          ? { id: r.id, action, visibility: visibilityOf(r), protect: !unprotected[r.id] }
          : { id: r.id, action, ...(rejectNote.trim() ? { note: rejectNote.trim() } : {}) }
      const res = await putJson<{ ok: true; repo?: string; protected?: boolean; protection?: string | null }>(
        '/api/workbench/repo-requests',
        body,
      )
      if (action === 'approve' && res.repo) {
        decided = res.protected
          ? `${res.repo} created; its default branch is protected.`
          : `${res.repo} created; its default branch is not protected: ${res.protection ?? 'not asked for'}.`
      }
      if (action === 'reject') {
        rejecting = null
        rejectNote = ''
      }
    } catch (e) {
      decideError = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['repo-requests'] })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
</script>

<div class="space-y-2">
  <div class="flex flex-wrap items-baseline gap-x-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent repo requests</span>
    <span class="text-xs text-muted">agents REQUEST new repos in a host's approved orgs; you approve each one</span>
  </div>
  {#if requests.length === 0 && !requestsFailed}
    <p class="text-xs text-muted">Nothing waiting on you.</p>
  {/if}
  {#if requestsFailed}
    <QueryError
      variant="inline"
      error={requestsQuery.error}
      title="Could not load pending repo requests"
      onRetry={() => void requestsQuery.refetch()}
    />
  {/if}
  {#each requests as r (r.id)}
    <div class="space-y-2 rounded-md border border-warning/30 px-3 py-2 text-sm" data-testid="repo-request">
      <div class="flex flex-wrap items-center gap-2">
        <span class="w-full min-w-0 truncate sm:w-auto sm:flex-1">
          <span class="font-mono text-[13px] text-fg">{r.org}/{r.name}</span>
          <span class="font-mono text-[11px] text-muted"> on {r.host}</span>
          <span class="text-muted"> · {r.agentModel}: {r.why}</span>
        </span>
        {#if r.appSlug}<Chip title="the Talaria app this repo will hold">app {r.appSlug}</Chip>{/if}
        <Select
          size="sm"
          aria-label="Visibility"
          value={visibilityOf(r)}
          onchange={(e) => (chosen[r.id] = e.currentTarget.value as Visibility)}
          class="w-28"
        >
          <option value="private">private</option>
          <option value="public">public</option>
        </Select>
        <Checkbox
          checked={!unprotected[r.id]}
          onChange={(on) => (unprotected[r.id] = !on)}
          label="protect default branch"
          title="Block force-push and deletion on the new repo's default branch (no required reviews or checks)"
        />
        <Button size="sm" onclick={() => void decide(r, 'approve')}>Create + grant</Button>
        <Button
          size="sm"
          variant="ghost"
          onclick={() => {
            rejecting = rejecting === r.id ? null : r.id
            rejectNote = ''
          }}>Reject</Button
        >
      </div>
      {#if rejecting === r.id}
        <div transition:slide={{ duration: 150 }} class="flex items-center gap-2">
          <Input
            size="sm"
            bind:value={rejectNote}
            maxlength={500}
            placeholder="Why not? Posted on the requesting ticket (optional)"
            class="min-w-0 flex-1"
          />
          <Button size="sm" variant="ghost" onclick={() => void decide(r, 'reject')}>Reject request</Button>
        </div>
      {/if}
    </div>
  {/each}
  {#if decideError}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{decideError}</div>{/if}
  {#if decided}<div transition:slide={{ duration: 150 }} class="text-xs text-muted" data-testid="repo-request-decided">{decided}</div>{/if}
</div>
