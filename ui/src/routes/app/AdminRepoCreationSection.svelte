<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** Agent repo creation: which orgs allow it, and the pending request queue
   *  (agents propose, THIS is where humans ratify — approval creates the repo
   *  and grants it to the requester). */
  let {
    status,
    save,
  }: {
    status: { repoCreationOrgs?: string[] }
    save: (body: unknown) => Promise<void>
  } = $props()

  const qc = useQueryClient()
  const cfgOrgs = $derived(status.repoCreationOrgs ?? [])
  let orgs = $state((status.repoCreationOrgs ?? []).join(', '))
  interface RepoReq {
    id: string
    agentModel: string
    org: string
    name: string
    why: string
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
  let decideError = $state<string | null>(null)
  const decide = async (id: string, action: 'approve' | 'reject') => {
    decideError = null
    const r = await fetch('/api/workbench/repo-requests', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id, action }),
    })
    if (!r.ok) decideError = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed'
    await qc.invalidateQueries({ queryKey: ['repo-requests'] })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
</script>

<div class="space-y-2 border-t border-line-subtle pt-3">
  <div class="flex items-baseline gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent repo creation</span>
    <span class="text-xs text-muted">agents may REQUEST new repos in these orgs; you approve each one</span>
  </div>
  <div class="flex items-center gap-2">
    <Input
      size="sm"
      bind:value={orgs}
      onblur={() => {
        const list = orgs.split(',').map((o) => o.trim()).filter(Boolean)
        if (list.join(',') !== cfgOrgs.join(',')) void save({ repoCreationOrgs: list })
      }}
      placeholder="approved orgs, comma-separated (empty = off)"
      class="max-w-md"
    />
    <span class="text-xs text-muted">needs the App's org Administration permission to create</span>
  </div>
  {#if requestsFailed}
    <QueryError
      variant="inline"
      error={requestsQuery.error}
      title="Could not load pending repo requests"
      onRetry={() => void requestsQuery.refetch()}
    />
  {/if}
  {#each requests as r (r.id)}
    <div class="flex items-center gap-2 rounded-md border border-warning/30 px-3 py-2 text-sm">
      <span class="min-w-0 flex-1 truncate">
        <span class="font-mono text-[13px] text-fg">{r.org}/{r.name}</span>
        <span class="text-muted"> — {r.agentModel}: {r.why}</span>
      </span>
      <Button size="sm" onclick={() => void decide(r.id, 'approve')}>Create + grant</Button>
      <Button size="sm" variant="ghost" onclick={() => void decide(r.id, 'reject')}>Reject</Button>
    </div>
  {/each}
  {#if decideError}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{decideError}</div>{/if}
</div>
