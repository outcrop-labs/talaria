<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { getList } from '@/lib/fetch-json'
  import { cn } from '@/lib/cn'

  interface WbJob {
    id: string
    agentModel: string
    repo: string
    branch: string
    effort: string
    plan: string
    status: 'awaiting_approval' | 'started' | 'pr_open' | 'abandoned'
    prUrl: string | null
    testingBranch: string | null
    mergedTestingAt: string | null
  }

  /** Workbench jobs on this ticket — the plan-approval gate and PR links live
   *  here, next to the work they govern. Hidden when there are genuinely none;
   *  LOUD when the read fails (see below). */
  let { taskId, canEdit }: { taskId: string; canEdit: boolean } = $props()

  const qc = useQueryClient()
  const jobsQuery = createQuery(() => ({
    queryKey: ['workbench-jobs', taskId],
    // This strip is the human-approval gate. `if (!r.ok) return []` fed an
    // empty list into `if (!live.length) return null`, so a 500 on this GET
    // erased the gate from the ticket entirely — no error, no skeleton, not
    // one pixel — and the ticket read exactly like one with no agent on it
    // while an agent sat stopped, waiting on a person who could not see it.
    // Non-2xx throws now, and the error branch below renders in its place: an
    // approval nobody can see is strictly worse than an error message.
    queryFn: (): Promise<WbJob[]> => getList<WbJob>(`/api/workbench/jobs?taskId=${encodeURIComponent(taskId)}`, 'jobs'),
    refetchInterval: 30_000,
  }))
  const jobs = $derived(jobsQuery.data)
  const act = async (jobId: string, action: 'approve' | 'reject' | 'merge_testing') => {
    await fetch('/api/workbench/jobs', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jobId, action }),
    })
    await qc.invalidateQueries({ queryKey: ['workbench-jobs', taskId] })
  }
  const live = $derived((jobs ?? []).filter((j) => j.status !== 'abandoned'))
</script>

<!-- A failed background refetch keeps the last good strip on screen — stale
     approval buttons beat a vanished gate. Only a failure with nothing to fall
     back on takes the strip's place, and it says what the reader is missing. -->
{#if jobsQuery.isError && jobs === undefined}
  <div class="mb-3 rounded-xl border border-[color:var(--theme-danger)]/40 bg-[color:var(--theme-danger)]/5 px-4 py-2.5">
    <QueryError
      variant="inline"
      error={jobsQuery.error}
      title="Could not load this ticket's workbench jobs"
      onRetry={() => void jobsQuery.refetch()}
    />
    <p class="mt-1 text-xs text-muted">
      If an agent is waiting on a plan approval here, it stays blocked until this loads — don't read the ticket as idle.
    </p>
  </div>
{:else if jobs !== undefined && live.length}
  <div class="mb-3 space-y-2">
    {#each live as j (j.id)}
      <div
        class={cn(
          'rounded-lg border px-4 py-2.5 text-sm',
          j.status === 'awaiting_approval' ? 'border-warning/40 bg-warning/5' : 'border-line bg-card/40',
        )}
      >
        <div class="flex items-center gap-2">
          <span class="font-sans text-fg">
            {#if j.status === 'awaiting_approval'}{`${j.agentModel} plans ${j.effort}-effort work on ${j.repo} — approve to build`}{/if}
            {#if j.status === 'started'}{`${j.agentModel} is building on ${j.repo} @ ${j.branch}`}{/if}
            {#if j.status === 'pr_open'}{`${j.agentModel} opened a PR from ${j.branch}`}{/if}
          </span>
          <span class="ml-auto flex shrink-0 items-center gap-1.5">
            {#if j.status === 'awaiting_approval' && canEdit}
              <Button size="sm" onclick={() => void act(j.id, 'approve')}>
                Approve plan
              </Button>
              <Button size="sm" variant="outline" onclick={() => void act(j.id, 'reject')}>
                Reject
              </Button>
            {/if}
            {#if j.testingBranch && (j.status === 'started' || j.status === 'pr_open') && canEdit}
              {#if j.mergedTestingAt}
                <span class="font-mono text-[11px] tracking-[0.05em] text-muted">on {j.testingBranch}</span>
              {:else}
                <Button size="sm" variant="outline" onclick={() => void act(j.id, 'merge_testing')}>
                  Merge to {j.testingBranch}
                </Button>
              {/if}
            {/if}
            {#if j.prUrl}
              <a href={j.prUrl} target="_blank" rel="noreferrer" class="text-xs text-accent hover:underline">
                View PR
              </a>
            {/if}
          </span>
        </div>
        {#if j.status === 'awaiting_approval' && j.plan}<p class="mt-1.5 line-clamp-4 whitespace-pre-wrap font-sans text-xs text-muted">{j.plan}</p>{/if}
      </div>
    {/each}
  </div>
{/if}
