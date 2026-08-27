<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'

  /** Per-repo git flow — every reachable repo is a row; blank fields mean the
   *  defaults, so there is no separate "add" ceremony to learn. */
  interface FlowData {
    flows: Array<{ repo: string; baseBranch: string | null; testingBranch: string | null }>
    repos: string[]
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['workbench-flow'],
    queryFn: (): Promise<FlowData> => getJson<FlowData>('/api/workbench/flow'),
  }))
  const data = $derived(query.data)
  const savedFlash = useSavedFlash()

  const save = async (repo: string, patch: { baseBranch?: string | null; testingBranch?: string | null }) => {
    const cur = data?.flows.find((f) => f.repo === repo)
    try {
      await putJson<{ ok: true }>('/api/workbench/flow', {
        repo,
        baseBranch: patch.baseBranch !== undefined ? patch.baseBranch : (cur?.baseBranch ?? null),
        testingBranch: patch.testingBranch !== undefined ? patch.testingBranch : (cur?.testingBranch ?? null),
      })
    } catch (e) {
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    savedFlash.flash()
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
  // Every reachable repo is a row; configs for repos the connection lost
  // access to still show (flagged) so they can be understood and cleared.
  const rows = $derived(data ? [...new Set([...data.repos, ...data.flows.map((f) => f.repo)])].sort() : [])
</script>

<!-- A failed read used to REMOVE the whole section, so per-repo base and
     testing branches looked unconfigured — and re-entering them would have
     overwritten the real flow. -->
{#if !data}
  {#if query.isError}
    <div class="border-t border-line-subtle pt-3">
      <QueryError
        variant="inline"
        error={query.error}
        title="Could not load repository flow"
        onRetry={() => void query.refetch()}
      />
    </div>
  {/if}
{:else}
  <div class="space-y-2 border-t border-line-subtle pt-3">
    <div class="flex items-baseline gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repository flow</span>
      <span class="text-xs text-muted">how workbench branches and PRs move, per repo</span>
      {#if savedFlash.saved}<span class="ml-auto text-xs text-success">Saved</span>{/if}
    </div>
    <div class="overflow-x-auto">
      <div class="min-w-[36rem]">
        <div class="grid grid-cols-[minmax(0,1fr)_11rem_11rem] gap-2 pb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Repository</span>
          <span>PRs land on</span>
          <span>Testing branch</span>
        </div>
        <div class="space-y-1.5">
          {#each rows as repo (repo)}
            {@const f = data.flows.find((x) => x.repo === repo)}
            {@const unreachable = !data.repos.includes(repo)}
            <div class="grid grid-cols-[minmax(0,1fr)_11rem_11rem] items-center gap-2">
              <span class="min-w-0 truncate font-mono text-xs text-fg">
                {repo}
                {#if unreachable}<span class="ml-2 font-sans text-xs text-warning">no longer reachable</span>{/if}
              </span>
              <Input
                size="sm"
                value={f?.baseBranch ?? ''}
                placeholder="default branch"
                onblur={(e) => e.currentTarget.value.trim() !== (f?.baseBranch ?? '') && void save(repo, { baseBranch: e.currentTarget.value.trim() || null })}
              />
              <Input
                size="sm"
                value={f?.testingBranch ?? ''}
                placeholder="none"
                onblur={(e) => e.currentTarget.value.trim() !== (f?.testingBranch ?? '') && void save(repo, { testingBranch: e.currentTarget.value.trim() || null })}
              />
            </div>
          {/each}
          {#if rows.length === 0}<p class="text-xs text-muted">No repositories reachable yet. Finish the install on GitHub.</p>{/if}
        </div>
      </div>
    </div>
    <p class="text-[11px] text-muted">
      <span class="text-fg">PRs land on</span>: the branch workbench jobs cut from and pull requests target; blank uses the repo's default branch.{' '}
      <span class="text-fg">Testing branch</span>: optional integration branch a feature can be merged into (from the ticket or by the agent) before its PR ships; blank disables it. Testing merges never replace review; the PR still lands normally.
    </p>
  </div>
{/if}
