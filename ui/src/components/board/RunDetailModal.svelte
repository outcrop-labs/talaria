<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { Square } from '@lucide/svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import { getJson } from '@/lib/fetch-json'
  import { useStopWorkSession, useWorkSession } from '@/lib/work-session.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { framesFromTranscript, splitWatch, type WatchFrame } from '@/lib/work-watch'
  import WorkWatch from './WorkWatch.svelte'

  // The run-detail modal: the full-insight view of a work session, one pane
  // per question, in the takeover shape (a tabbed manager per Modal's own
  // guidance). AGENT is Hermes: replies and its own tool calls, retained
  // turns above the live tail. TURNS is the harness exchange — the ask, the
  // harness output, and the prose the agent wrote back. Both read the
  // retained transcript through the run's own door, plus the live watch.
  // RESOURCES is the agent container's cpu/mem/pids over the run's window.
  // The header's Stop ends the ticket's live session. Everything else in
  // here still only reads.
  let {
    open,
    onClose,
    runId,
    taskId,
    onEnded,
    focus = 'agent',
  }: {
    open: boolean
    onClose: () => void
    runId: string
    taskId: string
    onEnded: () => void
    /** The eye opens on the agent. Turns is the harness exchange. */
    focus?: 'agent' | 'turns'
  } = $props()

  type Tab = 'agent' | 'turns' | 'resources'
  // svelte-ignore state_referenced_locally -- reason: seeded from the opener; the effect below re-seeds when the run changes
  let tab = $state<Tab>(focus)
  // A new run opens where the caller asked. Tab switches stay until the run
  // changes — focus alone would not reset a reused modal.
  $effect(() => {
    void runId
    tab = focus
  })

  const session = useWorkSession(() => taskId)
  const stopWork = useStopWorkSession()
  const agentModel = $derived(session.data?.session?.agentModel ?? '')
  // The brake rides in the header, but only against a LIVE session on this
  // ticket — a modal opened from the work log (a finished run) has nothing
  // to stop and shows no button.
  const live = $derived(session.data?.session ?? null)
  let stopping = $state(false)
  const stop = async () => {
    stopping = true
    try {
      await stopWork(taskId)
    } finally {
      stopping = false
    }
  }

  // ── Transcript + live tail, split into agent and harness. ──────────────
  let transcript = $state<string | null>(null)
  let transcriptError = $state<unknown>(null)
  let transcriptReady = $state(false)
  let transcriptAttempt = $state(0)
  let liveFrames = $state<WatchFrame[]>([])
  const retryTranscript = () => {
    transcriptAttempt += 1
  }
  const noteFrame = (ev: WatchFrame) => {
    liveFrames = [...liveFrames, ev]
  }
  $effect(() => {
    if (!open) return
    const id = runId
    void transcriptAttempt
    const polling = true
    let current = true
    let loaded = false
    const load = () => {
      void getJson<{ body: string | null }>(`/api/runs/${id}/transcript`)
        .then((r) => {
          if (!current) return
          transcript = r.body
          transcriptError = null
          transcriptReady = true
          loaded = true
        })
        .catch((e) => {
          if (!current) return
          if (!loaded) transcriptError = e
          transcriptReady = true
        })
    }
    transcript = null
    transcriptError = null
    transcriptReady = false
    liveFrames = []
    load()
    const timer = polling ? setInterval(load, 15_000) : undefined
    return () => {
      current = false
      if (timer) clearInterval(timer)
    }
  })
  const retained = $derived(splitWatch(framesFromTranscript(transcript)))
  const liveHarness = $derived(splitWatch(liveFrames).turns)
  const harnessTurns = $derived([...retained.turns, ...liveHarness])
  // A captured turn moves into the transcript. Drop the live copy and let
  // the watch reconnect replay only the turn still in flight.
  let seenTranscript = -1
  $effect(() => {
    const len = transcript?.length ?? -1
    if (len === seenTranscript) return
    seenTranscript = len
    liveFrames = []
  })

  // ── Resources: the sampler's series for this agent. ─────────────────────
  type Sample = { t: number; cpu: number; mem: number; pids: number }
  let samples = $state<Sample[]>([])
  let latest = $state<{ cpu: number; mem: number; pids: number } | null>(null)
  let leak = $state(false)
  let host = $state<{
    available: number
    reserve: number
    agentCeiling: number
    pressure: string
  } | null>(null)
  let resourcesDenied = $state(false)
  $effect(() => {
    if (!open || tab !== 'resources') return
    void (async () => {
      try {
        const res = await getJson<{
          host: { available: number; reserve: number; agentCeiling: number; pressure: string } | null
          agents: {
            agent: string
            points: Sample[]
            latest: { cpu: number; mem: number; pids: number } | null
            leak?: boolean
          }[]
        }>(`/api/fleet/resources?minutes=120${agentModel ? `&agent=${encodeURIComponent(agentModel)}` : ''}`)
        const mine = res.agents.find((a) => a.agent === agentModel) ?? res.agents[0]
        samples = mine?.points ?? []
        latest = mine?.latest ?? null
        leak = mine?.leak ?? false
        host = res.host
        resourcesDenied = false
      } catch {
        resourcesDenied = true
      }
    })()
  })

  // A tiny inline sparkline: one metric, no axes, latest value in text.
  function spark(points: number[], width = 260, height = 36): string {
    if (points.length < 2) return ''
    const max = Math.max(...points, 0.0001)
    const step = width / (points.length - 1)
    return points
      .map((p, i) => `${i === 0 ? 'M' : 'L'}${(i * step).toFixed(1)},${(height - (p / max) * (height - 4) - 2).toFixed(1)}`)
      .join(' ')
  }
  const fmtMem = (b: number) =>
    b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GiB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(0)} MiB` : `${b} B`

  const tabs = $derived<TabItem<Tab>[]>([
    { id: 'agent', label: 'Agent' },
    { id: 'turns', label: harnessTurns.length ? `Turns (${harnessTurns.length})` : 'Turns' },
    { id: 'resources', label: 'Resources' },
  ])
</script>

<Modal {open} {onClose} title="Run detail" takeover>
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex items-center gap-1 border-b border-line-subtle pb-3">
      <Tabs items={tabs} value={tab} onChange={(id) => (tab = id)} />
      {#if agentModel || live}
        <span class="ml-auto flex min-w-0 items-center gap-2">
          {#if agentModel}<span class="truncate text-xs text-muted">{agentModel}</span>{/if}
          {#if live}
            <Button
              size="xs"
              variant="ghost"
              class="shrink-0 text-danger hover:text-danger"
              title="Stop the live work session"
              disabled={stopping}
              onclick={() => void stop()}
            >
              <Square size={11} />Stop
            </Button>
          {/if}
        </span>
      {/if}
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pt-4">
      <div class={tab === 'agent' ? '' : 'hidden'}>
        <WorkWatch
          {runId}
          {taskId}
          {onEnded}
          history={retained.agent}
          resetKey={transcript?.length ?? 0}
          onFrame={noteFrame}
        />
      </div>
      {#if tab === 'turns'}
        {#if transcriptError && harnessTurns.length === 0}
          <QueryError
            variant="inline"
            error={transcriptError}
            title="Could not load the harness turns"
            onRetry={retryTranscript}
          />
        {:else if !transcriptReady && harnessTurns.length === 0}
          <div class="py-10 text-center text-sm text-muted">Loading the harness turns</div>
        {:else if harnessTurns.length === 0}
          <div class="py-10 text-center text-sm text-muted">No harness turns. The agent has not called one.</div>
        {:else}
          <div class="space-y-4">
            {#each harnessTurns as turn, i (i)}
              <div class="rounded-lg border border-line-subtle">
                <div class="border-b border-line-subtle px-4 py-2 text-xs font-medium text-fg">
                  Turn {i + 1}{turn.running ? ' · running' : ''}
                </div>
                <div class="space-y-3 px-4 py-3">
                  {#if turn.steer}
                    <div>
                      <div class="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Asked</div>
                      <pre class="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border border-line-subtle bg-raised/40 p-2.5 font-mono text-xs text-fg">{turn.steer}</pre>
                    </div>
                  {/if}
                  <div>
                    <div class="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Harness</div>
                    <pre class="max-h-[420px] overflow-y-auto whitespace-pre-wrap rounded-md border border-line-subtle bg-[color-mix(in_srgb,var(--color-ink-dim)_10%,transparent)] p-3 font-mono text-xs leading-relaxed text-fg">{turn.output || (turn.running ? 'running' : 'No output captured.')}</pre>
                  </div>
                  {#if turn.response}
                    <div>
                      <div class="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Agent</div>
                      <p class="whitespace-pre-wrap font-sans text-sm text-fg">{turn.response}</p>
                    </div>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
      {#if tab === 'resources'}
        {#if resourcesDenied}
        <div class="py-10 text-center text-sm text-muted">Resource samples are admin-only.</div>
        {:else if !latest}
        <div class="py-10 text-center text-sm text-muted">No samples yet — the sampler runs once a minute.</div>
        {:else}
        {#if host}
          <p class="mb-3 text-xs text-muted">
            VM {fmtMem(host.available)} free · {fmtMem(host.reserve)} kept for the platform · agent
            ceiling {fmtMem(host.agentCeiling)}
            {#if host.pressure !== 'ok'}
              · <span class="text-fg">{host.pressure}</span>
            {/if}
          </p>
        {/if}
        {#if leak}
          <p class="mb-3 text-xs text-fg">
            Memory is climbing on this agent. The cgroup will OOM this container, not the VM.
          </p>
        {/if}
        <div class="grid gap-4 sm:grid-cols-3">
          <div class="rounded-lg border border-line-subtle p-4">
            <div class="text-xs text-muted">CPU</div>
            <div class="mt-1 text-lg font-semibold text-fg">{latest.cpu.toFixed(1)}%</div>
            <svg viewBox="0 0 260 36" class="mt-2 w-full text-accent"><path d={spark(samples.map((s) => s.cpu))} fill="none" stroke="currentColor" stroke-width="1.5" /></svg>
          </div>
          <div class="rounded-lg border border-line-subtle p-4">
            <div class="text-xs text-muted">Memory</div>
            <div class="mt-1 text-lg font-semibold text-fg">{fmtMem(latest.mem)}</div>
            <svg viewBox="0 0 260 36" class="mt-2 w-full text-accent"><path d={spark(samples.map((s) => s.mem))} fill="none" stroke="currentColor" stroke-width="1.5" /></svg>
          </div>
          <div class="rounded-lg border border-line-subtle p-4">
            <div class="text-xs text-muted">Processes</div>
            <div class="mt-1 text-lg font-semibold text-fg">{latest.pids}</div>
            <svg viewBox="0 0 260 36" class="mt-2 w-full text-accent"><path d={spark(samples.map((s) => s.pids))} fill="none" stroke="currentColor" stroke-width="1.5" /></svg>
          </div>
        </div>
      {/if}
      {/if}
    </div>
  </div>
</Modal>
