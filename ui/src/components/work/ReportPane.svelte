<script lang="ts">
  import { ExternalLink, MessageCircleQuestion } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import type { Snippet } from 'svelte'
  import { p } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import SourcesList from './SourcesList.svelte'
  import ReportSkeleton from '@/routes/app/research/ReportSkeleton.svelte'
  import { useArtifact } from '@/lib/artifacts'
  import { errorMessage } from '@/lib/fetch-json'
  import { answerResearch, useResearchRun } from '@/lib/research'
  import { useSession } from '@/lib/session'

  // ONE RUN'S REPORT, AS A PANE — the document the run produced, where the run
  // has got to, and the question it is parked on, in one scrolling column.
  //
  // It owns its own reads (`useResearchRun`, `useArtifact`), so a surface can
  // mount it beside whatever it puts next to a report — a discussion, a task
  // list, another run — without threading a run's data through itself. The run
  // query polls while the run is in flight (including parked: an answer can
  // land between any two ticks), so the pane follows a live run on its own.
  let {
    runId,
    actions,
    showSources = false,
  }: {
    runId: string
    /** Anything the host surface wants in the meta row, left of the
     *  open-in-Artifacts link — the share control, typically. The host owns
     *  its placement, because only it knows whether sharing already has a
     *  home in its own header. */
    actions?: Snippet
    /** Render the run's source registry under the report. Off by default: the
     *  report body already ends in a `## Sources` section, so a surface opts
     *  in when it wants the rows as a list instead. */
    showSources?: boolean
  } = $props()

  const runQuery = useResearchRun(() => runId)
  const run = $derived(runQuery.data?.run)
  const sources = $derived(runQuery.data?.sources ?? [])
  const artifactQuery = useArtifact(() => run?.artifactId ?? null)
  const artifact = $derived(artifactQuery.data)
  const artifactLoading = $derived(artifactQuery.isLoading)

  // ── THE QUESTION A PARKED RUN ASKS ─────────────────────────────────────────
  //
  // A run that needs a person is an approval, and this pane is one of the
  // places it surfaces: the question renders in place (the projection's
  // `awaiting` field) with the options the step itself offered, and answering
  // here resumes the run. It is not a second inbox — the approvals machinery
  // owns who is told and who may decide; this is the same question, answered
  // where the run already is.
  const session = useSession()
  const me = $derived(session.data)
  // Mirrors the run's declared audience (runs/defs/research.ts): the owner
  // decides an owned run, admins decide an org run, members read the question
  // with the buttons withheld rather than offered-then-refused.
  const mayDecide = $derived(!!me && (!!run && (me.id === run.ownerUserId || (run.ownerUserId === null && me.role === 'admin'))))

  let answering = $state<string | null>(null)
  let answerError = $state<string | null>(null)
  const queryClient = useQueryClient()
  const answer = async (optionId: string) => {
    answering = optionId
    answerError = null
    try {
      await answerResearch(runId, optionId)
      await queryClient.invalidateQueries({ queryKey: ['research-run', runId] })
      void queryClient.invalidateQueries({ queryKey: ['research-runs'] })
    } catch (e) {
      // 409 is the friendly one: somebody answered it already, which is what
      // the person clicking wanted. Everything else is the server's sentence.
      answerError = errorMessage(e)
      void queryClient.invalidateQueries({ queryKey: ['research-run', runId] })
    } finally {
      answering = null
    }
  }
</script>

<div class="min-w-0 flex-1 overflow-y-auto">
  {#if !run}
    <!-- The shape of the report to come: meta row, then the document. A
         skeleton the width of the content that replaces it, so the swap is
         not itself a jump. -->
    <div aria-hidden="true" class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 p-8">
      <Skeleton class="h-6 w-64" />
      <ReportSkeleton />
    </div>
  {:else}
    <div class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 p-8">
      <!-- Run meta line — chrome voice: 11px mono muted (spec §2). -->
      <div class="flex items-center gap-2 font-mono text-[11px] text-muted">
        <Avatar name={run.agentModel} class="h-6 w-6 text-[10px]" />
        <span>by {run.requestedBy}</span>
        {#if run.stats.sources !== undefined}<span>· {run.stats.sources} sources ({run.stats.cited} cited)</span>{/if}
        <span class="ml-auto"></span>
        {@render actions?.()}
        {#if run.artifactId}
          <a href={`${p('/artifacts')}?a=${run.artifactId}`} class="flex shrink-0 items-center gap-1 text-muted transition-colors hover:text-fg">
            Open in Artifacts <ExternalLink size={12} />
          </a>
        {/if}
      </div>

      {#if run.awaiting}
        <!-- The run stopped to ask, and this is the ask — above the status
             panel because it IS the status, and with the options the step
             itself offered: the run can only be told something it wrote a
             branch for. A free-text ask carries no buttons — it is answered
             in the discussion, where a sentence is the input. -->
        <Panel class="space-y-3">
          <div class="flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.08em] text-ink-dim">
            <MessageCircleQuestion size={13} aria-hidden="true" />
            <span>Needs a decision</span>
          </div>
          <div class="text-sm text-fg">{run.awaiting.question}</div>
          {#if run.awaiting.detail}<div class="text-xs text-muted">{run.awaiting.detail}</div>{/if}
          {#if run.awaiting.freeText}
            <!-- The scope question: free text, so the chat resume hook spends
                 the owner's next message in the discussion as the answer.
                 Buttons here would be a placeholder pretending to be an
                 input. -->
            {#if mayDecide}
              <div class="text-sm text-fg">Waiting on you — reply in the discussion.</div>
            {:else}
              <div class="text-xs text-muted">
                Waiting on {run.ownerUserId ? 'the run’s owner' : 'an admin'} to reply in the discussion.
              </div>
            {/if}
          {:else if mayDecide}
            <div class="flex flex-wrap gap-2">
              {#each run.awaiting.options as opt (opt.id)}
                <Button size="sm" onclick={() => answer(opt.id)} disabled={answering !== null}>
                  {answering === opt.id ? 'Sending…' : opt.label}
                </Button>
              {/each}
            </div>
            {#each run.awaiting.options as opt (opt.id)}
              {#if opt.detail}<div class="text-xs text-muted">{opt.label} — {opt.detail}</div>{/if}
            {/each}
            {#if answerError}<div class="text-xs text-danger">{answerError}</div>{/if}
          {:else}
            <div class="text-xs text-muted">
              Waiting on {run.ownerUserId ? 'the run’s owner' : 'an admin'} to answer.
            </div>
          {/if}
        </Panel>
      {:else if run.status === 'awaiting'}
        <!-- Awaiting with no question on the wire — a projection gap, not a
             state to paper over: the panel above is the surface when the
             question lands, and until then this is the honest sentence. -->
        <Panel class="flex items-center gap-3">
          <WaitingMark site="research/run" size={16} class="text-accent" />
          <div class="min-w-0">
            <div class="text-sm text-fg">Waiting on you</div>
            <div class="text-xs text-muted">The run is parked on a question</div>
          </div>
        </Panel>
      {:else if run.status === 'queued' || run.status === 'running'}
        <Panel class="flex items-center gap-3">
          <WaitingMark site="research/run" size={16} class="text-accent" />
          <div class="min-w-0">
            <div class="text-sm text-fg">{run.status === 'queued' ? 'Queued' : 'Researching'}</div>
            {#if run.phase}<div class="truncate text-xs text-muted">{run.phase}</div>{/if}
          </div>
        </Panel>
      {/if}
      {#if run.status === 'error'}
        <Panel>
          <div class="text-sm text-danger">{run.error ?? 'The run failed.'}</div>
        </Panel>
      {/if}

      <!-- The report body holds its shape while the artifact fetch is in
           flight — no null-then-pop when the document lands. -->
      {#if run.artifactId && !artifact && artifactLoading}<ReportSkeleton />{/if}
      {#if artifact}
        <Panel>
          <Markdown class="prose-sm" children={artifact.body} />
        </Panel>
      {/if}

      {#if sources.length > 0 && run.status === 'done' && !artifact}
        <Panel>
          <div class="text-xs text-muted">Report document is loading</div>
        </Panel>
      {/if}

      {#if showSources}<SourcesList {sources} />{/if}
    </div>
  {/if}
</div>
