<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import DitherLayer from '@/components/ui/DitherLayer.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { useWorkSession } from '@/lib/work-session.svelte'
  import RunDetailModal from './RunDetailModal.svelte'

  // The ticket's live-work treatment: while an agent's work session is on
  // this ticket, the strip carries the dither field (the same material the
  // generating blocks wear — "work is happening here"), the live phase
  // sentence, and the WATCH affordance. The CTA opens the watch modal.
  let { taskId }: { taskId: string } = $props()

  const session = useWorkSession(() => taskId)
  let watchOpen = $state(false)
  const live = $derived(session.data?.session ?? null)
  const wait = $derived(session.data?.wait ?? null)

  const agentLabel = $derived.by(() => {
    const m = live?.agentModel ?? wait?.agentModel ?? ''
    const head = m.split('-')[0] ?? ''
    return head ? head[0]!.toUpperCase() + head.slice(1) : 'An agent'
  })
</script>

{#if live || wait}
  <div class="relative overflow-hidden rounded-lg border border-line">
    <DitherLayer
      sources={[
        { id: 'lull', kind: 'edge', side: 'top', depth: 30, strength: 0.14 },
        { id: 'drift', kind: 'wave', axis: 'x', wavelength: 190, speed: 16, strength: 0.34 },
      ]}
      pitch={4}
      dot={1.4}
      alphaFloor={0.04}
      maxAlpha={0.3}
    />
    <div class="relative flex flex-wrap items-center gap-2 px-3 py-2">
      <WaitingMark site="ticket/work-watch" class="text-accent" size={13} />
      <span class="min-w-0 flex-1 truncate text-sm text-fg">
        {#if live}
          {agentLabel} is working this ticket
          {#if live.turn}<span class="text-muted"> · turn {live.turn}</span>{/if}
          {#if live.phase}<span class="text-muted"> · {live.phase}</span>{/if}
        {:else if wait}
          {agentLabel} is queued
          {#if wait.phase}<span class="text-muted"> · {wait.phase}</span>{/if}
          <span class="block truncate text-xs text-muted">{wait.reason}</span>
        {/if}
      </span>
      {#if live}
        <Button size="xs" variant="ghost" onclick={() => (watchOpen = true)}>Watch the work</Button>
      {/if}
    </div>
  </div>

  {#if live}
    <RunDetailModal open={watchOpen} onClose={() => (watchOpen = false)} runId={live.runId} {taskId} onEnded={() => (watchOpen = false)} />
  {/if}
{/if}
