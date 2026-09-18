<script lang="ts">
  import Modal from '@/components/ui/Modal.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { useWorkSession } from '@/lib/work-session.svelte'
  import { useTargetArtifacts } from '@/lib/artifacts'
  import WorkWatch from './WorkWatch.svelte'

  // The run-detail modal: the full-insight view of a work session, one pane
  // per question, in the takeover shape (a tabbed manager per Modal's own
  // guidance). LIVE is the agent's stream as it happens (the old watch pane,
  // verbatim). TURNS is the retained per-turn transcript — the prompt that
  // drove each turn, every tool call with its argument preview (where the
  // harness steering is legible), and the workbench's own MCP calls with
  // args and outcomes. RESOURCES is the agent container's cpu/mem/pids over
  // the run's window. A window, not a steering wheel: nothing here touches
  // the session.
  let {
    open,
    onClose,
    runId,
    taskId,
    onEnded,
  }: {
    open: boolean
    onClose: () => void
    runId: string
    taskId: string
    onEnded: () => void
  } = $props()

  type Tab = 'live' | 'turns' | 'resources'
  let tab = $state<Tab>('live')

  const session = useWorkSession(() => taskId)
  const artifacts = useTargetArtifacts('task', () => taskId)
  const agentModel = $derived(session.data?.session?.agentModel ?? '')

  // ── Turns: parse the run's transcript artifact. ─────────────────────────
  type StreamLine = { t: string; v: string; s?: string; p?: string; r?: string; ms?: number }
  type TurnBlock = { n: string; prompt: string; lines: StreamLine[] }
  const transcript = $derived(
    (artifacts.data ?? []).find((a) => a.kind === 'run-transcript' && a.title.includes(runId))?.body ?? null,
  )
  const turns = $derived.by<TurnBlock[]>(() => {
    const body = transcript
    if (!body) return []
    const out: TurnBlock[] = []
    for (const raw of body.split('\n## Turn ')) {
      const n = raw.slice(0, raw.indexOf('\n')).trim()
      if (!/^\d+$/.test(n)) continue
      const promptAt = raw.indexOf('### Prompt')
      const streamAt = raw.indexOf('### Stream')
      const prompt =
        promptAt >= 0 && streamAt > promptAt
          ? raw.slice(promptAt + 11, streamAt).replace(/```/g, '').trim()
          : ''
      const jsonBlock = streamAt >= 0 ? raw.slice(streamAt + 11).match(/```json\n([\s\S]*?)```/) : null
      const lines: StreamLine[] = []
      for (const line of (jsonBlock?.[1] ?? '').split('\n')) {
        if (!line.startsWith('{')) continue
        try {
          lines.push(JSON.parse(line) as StreamLine)
        } catch {
          // a line we cannot parse is a line we skip
        }
      }
      out.push({ n, prompt, lines })
    }
    return out
  })

  // ── Resources: the sampler's series for this agent. ─────────────────────
  type Sample = { t: number; cpu: number; mem: number; pids: number }
  let samples = $state<Sample[]>([])
  let latest = $state<{ cpu: number; mem: number; pids: number } | null>(null)
  let resourcesDenied = $state(false)
  $effect(() => {
    if (!open || tab !== 'resources') return
    void (async () => {
      try {
        const res = await getJson<{
          agents: { agent: string; points: Sample[]; latest: { cpu: number; mem: number; pids: number } | null }[]
        }>(`/api/fleet/resources?minutes=120${agentModel ? `&agent=${encodeURIComponent(agentModel)}` : ''}`)
        const mine = res.agents.find((a) => a.agent === agentModel) ?? res.agents[0]
        samples = mine?.points ?? []
        latest = mine?.latest ?? null
        resourcesDenied = false
      } catch {
        // 403 for non-admins is the designed gate, not an error state.
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

  function lineLabel(l: StreamLine): string {
    if (l.t === 'tool') return `⚙ ${l.v}${l.s === 'running' ? ' …' : l.s === 'completed' ? ' ✓' : ''}`
    if (l.t === 'wtool') return `🛠 ${l.v}${l.ms ? ` (${(l.ms / 1000).toFixed(1)}s)` : ''}`
    if (l.t === 'r') return `· ${l.v}`
    if (l.t === 'err') return `⚠ ${l.v}`
    return l.v
  }
  const tabs: { id: Tab; label: string }[] = [
    { id: 'live', label: 'Live' },
    { id: 'turns', label: 'Turns' },
    { id: 'resources', label: 'Resources' },
  ]
</script>

<Modal {open} {onClose} title="Run detail" takeover>
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex items-center gap-1 border-b border-line-subtle pb-3">
      {#each tabs as t (t.id)}
        <button
          type="button"
          class="rounded-md px-3 py-1.5 text-xs font-medium transition-colors {tab === t.id
            ? 'bg-raised text-fg'
            : 'text-muted hover:text-fg'}"
          onclick={() => (tab = t.id)}
        >
          {t.label}{t.id === 'turns' && turns.length ? ` (${turns.length})` : ''}
        </button>
      {/each}
      {#if agentModel}<span class="ml-auto truncate text-xs text-muted">{agentModel}</span>{/if}
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pt-4">
      {#if tab === 'live'}
        <WorkWatch {runId} {taskId} {onEnded} />
      {:else if tab === 'turns'}
        {#if !transcript}
          <div class="py-10 text-center text-sm text-muted">
            No retained transcript{#if !session.data?.session} — the session may predate transcript capture{/if}.
          </div>
        {:else}
          <div class="space-y-4">
            {#each [...turns].reverse() as turn (turn.n)}
              <details class="rounded-lg border border-line-subtle" open={turn.n === turns[turns.length - 1]?.n}>
                <summary class="cursor-pointer px-4 py-2 text-xs font-medium text-fg">
                  Turn {turn.n} · {turn.lines.filter((l) => l.t === 'tool' || l.t === 'wtool').length} tool calls
                </summary>
                <div class="space-y-3 border-t border-line-subtle px-4 py-3">
                  {#if turn.prompt}
                    <div>
                      <div class="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Prompt</div>
                      <pre class="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border border-line-subtle bg-raised/40 p-2.5 font-mono text-xs text-fg">{turn.prompt}</pre>
                    </div>
                  {/if}
                  <div>
                    <div class="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Stream</div>
                    <div class="max-h-[420px] overflow-y-auto rounded-md border border-line-subtle bg-[color-mix(in_srgb,var(--color-ink-dim)_10%,transparent)] p-3 font-mono text-xs leading-relaxed text-fg">
                      {#each turn.lines as l, i (i)}
                        <div class="whitespace-pre-wrap">{lineLabel(l)}</div>
                        {#if (l.t === 'tool' || l.t === 'wtool') && (l.p || l.r)}
                          <div class="mb-1 whitespace-pre-wrap border-l-2 border-line-subtle pl-2 text-muted">
                            {#if l.p}<div>{l.p}</div>{/if}
                            {#if l.r}<div class="mt-0.5">→ {l.r}</div>{/if}
                          </div>
                        {/if}
                      {/each}
                    </div>
                  </div>
                </div>
              </details>
            {/each}
          </div>
        {/if}
      {:else if resourcesDenied}
        <div class="py-10 text-center text-sm text-muted">Resource samples are admin-only.</div>
      {:else if !latest}
        <div class="py-10 text-center text-sm text-muted">No samples yet — the sampler runs once a minute.</div>
      {:else}
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
    </div>
  </div>
</Modal>
