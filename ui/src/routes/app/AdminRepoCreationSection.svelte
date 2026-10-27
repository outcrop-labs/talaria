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

  /** Agent repo creation: which orgs allow it, and the pending request queue
   *  (agents propose, THIS is where humans ratify — approval creates the repo
   *  at the visibility chosen here, protects its default branch against
   *  force-push and deletion unless unticked, and grants it to the requester;
   *  a rejection posts its reason on the requesting ticket). */
  let {
    status,
    save,
  }: {
    status: { mode?: 'app' | 'pat' | null; repoCreationOrgs?: string[] }
    save: (body: unknown) => Promise<void>
  } = $props()

  const qc = useQueryClient()
  const cfgOrgs = $derived(status.repoCreationOrgs ?? [])
  // svelte-ignore state_referenced_locally -- reason: seed-once form draft; the parent gates this section on the loaded status
  let orgs = $state((status.repoCreationOrgs ?? []).join(', '))
  type Visibility = 'private' | 'public'
  interface RepoReq {
    id: string
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
  /** The last approval's word on the default branch: GitHub may refuse to
   *  protect a private repo on a free plan, and the repo exists anyway. */
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
    <span class="text-xs text-muted">needs the App installed on each org, with the org Administration permission</span>
  </div>
  {#if status.mode !== 'app'}
    <div class="text-xs text-warning" data-testid="repo-creation-mode">
      Repo creation needs the GitHub App connection. While GitHub is connected with an access token, agents' requests are refused.
    </div>
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
          title="Block force-push and deletion on the new repo's default branch (a GitHub ruleset; no required reviews or checks)"
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
