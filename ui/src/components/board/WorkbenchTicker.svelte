<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import DitherLayer from '@/components/ui/DitherLayer.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { Square } from '@lucide/svelte'
  import { useStopWorkSession, useWorkSession } from '@/lib/work-session.svelte'
  import { agentName, behindLine } from '@/lib/work-pulse'
  import RunDetailModal from './RunDetailModal.svelte'
  import WorkPulse from './WorkPulse.svelte'

  // The ticket's live-work treatment: while an agent's work session is on
  // this ticket, the strip carries the dither field (the same material the
  // generating blocks wear — "work is happening here"), the live phase
  // sentence, and the two controls. WATCH opens the run-detail modal; STOP
  // is the brake — ghost, but danger-tinted, and disabled while its POST is
  // in flight so a double-click cannot queue a second one.
  let { taskId }: { taskId: string } = $props()

  const session = useWorkSession(() => taskId)
  const stopWork = useStopWorkSession()
  let watchOpen = $state(false)
  let stopping = $state(false)
  const live = $derived(session.data?.session ?? null)
  const wait = $derived(session.data?.wait ?? null)
  // Latch the run being watched. The strip is driven by the LIVE session, so
  // gating the modal on `live` unmounted it the instant the session ended —
  // which is precisely when the transcript becomes the thing worth reading.
  // Held separately, the modal survives the session and closes on request.
  let watchedRunId = $state<string | null>(null)
  $effect(() => {
    if (live?.runId) watchedRunId = live.runId
  })

  const agentLabel = $derived(agentName(live?.agentModel ?? wait?.agentModel))

  const stop = async () => {
    stopping = true
    try {
      await stopWork(taskId)
    } finally {
      stopping = false
    }
  }
</script>

{#if live || wait}
  <div class="relative overflow-hidden rounded-lg border border-line">
    <!-- Tuned to the median look it had across browsers, DPRs and sub-pixel
         positions while its dots built up (N11): see LIVE_FIELD_TUNING in
         lib/dither-engine.ts. A 1 px dot: at its 1.2 px one the strip covered a
         quarter more pixels than the median did for the same ink. 48 alpha
         levels keep its crest's rewrites under rc's canvas calls; at 64 the
         strip cost 1.19 times rc's. -->
    <DitherLayer
      sources={[
        { id: 'lull', kind: 'edge', side: 'top', depth: 30, strength: 0.28 },
        { id: 'drift', kind: 'wave', axis: 'x', wavelength: 190, speed: 16, strength: 0.68 },
      ]}
      pitch={4}
      dot={1}
      alphaFloor={0.1067}
      maxAlpha={0.8}
      alphaSteps={48}
    />
    <div class="relative flex flex-wrap items-center gap-2 px-3 py-2">
      <WaitingMark site="ticket/work-watch" class="text-accent" size={13} />
      <span class="min-w-0 flex-1 truncate text-sm text-fg">
        {#if live}
          {agentLabel} is working this ticket
          <!-- The phase line only when there is no pulse to say it better:
               "turn 2 of 12 — waiting on doug" reads the same for a turn the
               agent has not taken and one forty minutes into a build. -->
          {#if !live.activity && live.phase}<span class="text-muted"> · {live.phase}</span>{/if}
        {:else if wait}
          {agentLabel} is queued
          {#if wait.phase}<span class="text-muted"> · {wait.phase}</span>{/if}
          <span class="block truncate text-xs text-muted">
            {wait.reason}{#if behindLine(wait.behind, wait.behindHidden)} · {behindLine(wait.behind, wait.behindHidden)}{/if}
          </span>
        {/if}
      </span>
      {#if live}
        <Button size="xs" variant="ghost" onclick={() => (watchOpen = true)}>Watch the work</Button>
        <Button
          size="xs"
          variant="ghost"
          class="text-danger hover:text-danger"
          title="Stop the live work session"
          disabled={stopping}
          onclick={() => void stop()}
        >
          <Square size={11} />Stop
        </Button>
      {/if}
    </div>
    {#if live?.activity}
      <WorkPulse session={live} receivedAt={session.dataUpdatedAt} class="relative border-t border-line-subtle px-3 py-2" />
    {/if}
  </div>

{/if}

<!-- Outside the strip's guard on purpose: the strip is for live work, the
     modal is the record of it, and the record outlives the session. -->
{#if watchOpen && watchedRunId}
  <RunDetailModal open={watchOpen} onClose={() => (watchOpen = false)} runId={watchedRunId} {taskId} />
{/if}
