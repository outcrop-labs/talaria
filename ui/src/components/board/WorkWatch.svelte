<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { useWorkSession } from '@/lib/work-session.svelte'
  import { slide } from '@/lib/motion'

  // The watch pane: the run's OWN SSE stream (/api/runs/{id}/events) for
  // live state and phase, the session row for the turn count and the agent's
  // last reply tail, and the ticket's activity refreshed on every event. A
  // window into the work, not control of it — nothing here touches the
  // session.
  let {
    runId,
    taskId,
    onEnded,
  }: { runId: string; taskId: string; onEnded: () => void } = $props()

  const qc = useQueryClient()
  // The same query the ticker holds — one cache entry, invalidated by the
  // SSE handler below, so both surfaces move together.
  const sessionQuery = useWorkSession(() => taskId)

  let phase = $state<string | null>(null)
  let ended = $state(false)
  $effect(() => {
    const es = new EventSource(`/api/runs/${runId}/events`)
    es.onmessage = (e) => {
      let ev: { state?: string; phase?: string }
      try {
        ev = JSON.parse(e.data) as { state?: string; phase?: string }
      } catch {
        return // a frame we cannot parse is a frame we ignore; the poll is the floor
      }
      if (ev.phase !== undefined) phase = ev.phase
      void qc.invalidateQueries({ queryKey: ['work-session', taskId] })
      void qc.invalidateQueries({ queryKey: ['task', taskId] })
      if (ev.state && ['done', 'error', 'cancelled'].includes(ev.state)) {
        ended = true
        onEnded()
      }
    }
    return () => es.close()
  })

  const session = $derived(sessionQuery.data?.session ?? null)

  const agentLabel = $derived.by(() => {
    const m = session?.agentModel ?? ''
    const head = m.split('-')[0] ?? ''
    return head ? head[0]!.toUpperCase() + head.slice(1) : 'The agent'
  })
</script>

<div class="space-y-3">
  <div class="flex items-center gap-2 text-sm">
    {#if ended}
      <span class="text-muted">Session ended — the ticket carries the outcome.</span>
    {:else}
      <WaitingMark site="ticket/work-watch" class="text-accent" size={13} />
      <span class="text-fg">{agentLabel} is working</span>
      {#if session?.turn}<span class="text-muted"> · turn {session.turn}/12</span>{/if}
      {#if phase}<span class="text-muted"> · {phase}</span>{/if}
    {/if}
  </div>

  <div>
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Last reply from the agent</div>
    {#if session?.lastTail}
      <pre class="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap rounded-md border border-line-subtle bg-raised/40 p-2.5 font-mono text-xs text-fg" transition:slide={{ duration: 150 }}>{session.lastTail}</pre>
    {:else}
      <pre class="mt-1 rounded-md border border-line-subtle bg-raised/40 p-2.5"><SkeletonRows rows={2} /></pre>
      <p class="mt-1 text-xs text-muted">The agent's first turn is still in flight — the tail appears the moment its reply is checkpointed.</p>
    {/if}
  </div>

  <p class="text-xs text-muted">
    Phases stream live from the session; the ticket's activity and comments refresh as work lands.
  </p>
</div>
