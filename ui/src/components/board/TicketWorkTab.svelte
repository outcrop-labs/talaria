<script lang="ts">
  import JudgeVerdict from './JudgeVerdict.svelte'
  import WorkLogStrip from './WorkLogStrip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { useWorkSessionHistory } from '@/lib/work-session.svelte'
  import type { JudgeReview } from '@/lib/boards.svelte'

  // THE WORK TAB — everything the work this ticket has seen LEFT BEHIND.
  //
  // This is where the run history went, and why. The history used to live in a
  // 144px-capped scroll box stacked above the description, alongside the live
  // ticker and the approval gates. So the moment an agent started working, the
  // record of every previous run pushed into the same fixed budget as the
  // description and the discussion, and the pane a reader actually came for got
  // squeezed — worst on a short window, where the description's own minimum
  // height meant the discussion was what gave way.
  //
  // The split that fixes it is by URGENCY, not by topic. Things that want a
  // decision NOW — the review gate, a plan waiting on approval, the live
  // session — stay pinned above the description, and there are never more than
  // a couple of them. Things that are a RECORD live here, in a tab, where they
  // have a whole pane to themselves and cost the rest of the ticket nothing.
  let {
    taskId,
    judgeReviews,
    onViewRun,
  }: {
    taskId: string
    judgeReviews: JudgeReview[]
    onViewRun: (runId: string) => void
  } = $props()

  // The same query key WorkLogStrip holds, so reading it here to decide the
  // zero state adds no fetch. The strip still owns the rows and its own failure
  // notice; this only needs to know whether there is anything at all.
  const history = useWorkSessionHistory(() => taskId)
  const sessions = $derived(history.data?.sessions ?? [])
  // A zero state may only render once the read has RESOLVED empty — never
  // while it is in flight, and never instead of a failure (the strip says that
  // itself). `data === undefined && !isError` is exactly "still loading".
  const loading = $derived(history.data === undefined && !history.isError)
  const empty = $derived(!loading && !history.isError && sessions.length === 0 && judgeReviews.length === 0)
</script>

<div class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
  {#if loading}
    <SkeletonRows rows={3} />
  {:else if empty}
    <EmptyState
      variant="compact"
      icon="◎"
      title="No work yet"
      hint="When an agent picks this ticket up, every run it does shows here with its log."
    />
  {:else}
    <WorkLogStrip {taskId} onView={onViewRun} />

    {#if judgeReviews.length}
      <section class="space-y-2">
        <h3 class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Review history</h3>
        <!-- Newest first, as the read hands them back. The gate above the
             description shows only the most recent verdict; the rest are here
             rather than nowhere. -->
        {#each judgeReviews as review (review.id)}
          <JudgeVerdict {review} />
        {/each}
      </section>
    {/if}
  {/if}
</div>
