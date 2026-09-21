<script lang="ts">
  import { openStream } from '@/lib/sse'
  import { useQueryClient } from '@tanstack/svelte-query'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { getStream } from '@/lib/fetch-json'
  import { useWorkSession } from '@/lib/work-session.svelte'

  // The watch pane: a TERMINAL of the agent's own work. Two streams feed it
  // — the run's SSE for state/phase (turn boundaries, ends), and the run's
  // watch SSE for the agent's words and tool calls as they happen, replayed
  // from the current turn's tail so the modal opens mid-work with history.
  // A window, not a steering wheel: nothing here touches the session.
  let {
    runId,
    taskId,
    onEnded,
  }: { runId: string; taskId: string; onEnded: () => void } = $props()

  const qc = useQueryClient()
  const sessionQuery = useWorkSession(() => taskId)

  let phase = $state<string | null>(null)
  let ended = $state(false)
  let lines = $state<string[]>([])
  let terminal = $state('')

  // ── Run state: phases and the end. ────────────────────────────────────────
  $effect(() => {
    return openStream(`/api/runs/${runId}/events`, (data) => {
      let ev: { state?: string; phase?: string }
      try {
        ev = JSON.parse(data) as { state?: string; phase?: string }
      } catch {
        return
      }
      if (ev.phase !== undefined) phase = ev.phase
      void qc.invalidateQueries({ queryKey: ['work-session', taskId] })
      void qc.invalidateQueries({ queryKey: ['task', taskId] })
      if (ev.state && ['done', 'error', 'cancelled'].includes(ev.state)) {
        ended = true
        onEnded()
      }
    })
  })

  // ── The work terminal: replay + live frames of the agent's stream. ───────
  // fetch-stream rather than EventSource: the watch frames ARE SSE, but we
  // render them into a transcript rather than consume them as events, and
  // fetch gives us the replay and the live half in one body.
  $effect(() => {
    lines = []
    terminal = ''
    let closed = false
    const ctl = new AbortController()
    const feed = (async () => {
      let res: Response & { body: ReadableStream<Uint8Array> }
      try {
        res = await getStream(`/api/runs/${runId}/watch`, { signal: ctl.signal })
      } catch {
        return // a stream that cannot open is a pane that stays quiet; the run SSE still reports state
      }
      const reader = res.body.getReader()
      const dec = new TextDecoder()
      let buf = ''
      while (!closed) {
        const { done, value } = await reader.read()
        if (done) break
        buf += dec.decode(value, { stream: true })
        let nl: number
        while ((nl = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, nl)
          buf = buf.slice(nl + 1)
          if (!line.startsWith('data: ')) continue
          try {
            appendEvent(
              JSON.parse(line.slice(6)) as { t: string; v: string; s?: string; p?: string; r?: string; ms?: number },
            )
          } catch {
            // a frame we cannot parse is a frame we skip
          }
        }
      }
    })()
    void feed.catch(() => {})
    return () => {
      closed = true
      ctl.abort()
    }
  })

  /** One watch line → one terminal line. Tool calls get their own marker
   *  line, their argument preview indented beneath (that preview is where
   *  the harness steering is legible — the whole command the agent ran);
   *  prose appends to the flowing paragraph. */
  function appendEvent(ev: { t: string; v: string; s?: string; p?: string; r?: string; ms?: number }) {
    if (ev.t === 'd') {
      terminal += ev.v
      return
    }
    // Every non-content event starts a new terminal line, so prose and tool
    // markers never run together.
    if (terminal && !terminal.endsWith('\n')) {
      terminal += '\n'
      lines = [...lines, terminal]
      terminal = ''
    }
    const mark =
      ev.t === 'tool'
        ? `⚙ ${ev.v}${ev.s === 'running' ? ' …' : ev.s === 'completed' ? ' ✓' : ''}`
        : ev.t === 'toolfull'
          ? `⚙ ${ev.v}${ev.s === 'running' ? ' …' : ' ✓'}`
          : ev.t === 'wtool'
            ? `🛠 ${ev.v}${ev.ms ? ` (${(ev.ms / 1000).toFixed(1)}s)` : ''}`
            : ev.t === 'r'
              ? `· ${ev.v}`
              : `⚠ ${ev.v}`
    lines = [...lines, mark]
    if ((ev.t === 'tool' || ev.t === 'toolfull' || ev.t === 'wtool') && (ev.p || ev.r)) {
      const detail = [ev.p, ev.r ? `→ ${ev.r}` : ''].filter(Boolean).join('\n')
      lines = [...lines, `  ${detail.split('\n').join('\n  ')}`]
    }
  }

  // Pinned scroll: the terminal follows the newest line while the modal is
  // open, unless the reader scrolled up to read history.
  let scrollBox: HTMLDivElement | undefined = $state()
  $effect(() => {
    void lines
    void terminal
    const el = scrollBox
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60
    if (atBottom) el.scrollTop = el.scrollHeight
  })

  const session = $derived(sessionQuery.data?.session ?? null)
  const agentLabel = $derived.by(() => {
    const m = session?.agentModel ?? ''
    const head = m.split('-')[0] ?? ''
    return head ? head[0]!.toUpperCase() + head.slice(1) : 'The agent'
  })
</script>

<div class="space-y-3">
  <div class="flex flex-wrap items-center gap-2 text-sm">
    {#if ended}
      <span class="text-muted">Session ended — the ticket carries the outcome.</span>
    {:else}
      <WaitingMark site="ticket/work-watch" class="text-accent" size={13} />
      <span class="text-fg">{agentLabel} is working</span>
      {#if session?.turn}<span class="text-muted"> · turn {session.turn}</span>{/if}
      {#if phase}<span class="text-muted"> · {phase}</span>{/if}
    {/if}
  </div>

  <!-- The terminal: the agent's own stream, replayed then live. Tool markers
       on their own lines; prose flows; newest at the bottom, pinned. -->
  <div
    class="max-h-[420px] overflow-y-auto rounded-md border border-line-subtle bg-[color-mix(in_srgb,var(--color-ink-dim)_10%,transparent)] p-3 font-mono text-xs leading-relaxed text-fg"
    bind:this={scrollBox}
  >
    {#each lines as l, i (i)}<div class="whitespace-pre-wrap">{l}</div>{/each}
    {#if terminal}<div class="whitespace-pre-wrap">{terminal}<span class="animate-pulse">▍</span></div>{/if}
    {#if !ended && lines.length === 0 && !terminal}
      <div class="text-muted">waiting for the agent's next output…</div>
    {/if}
  </div>

  {#if session?.lastTail}
    <details>
      <summary class="cursor-pointer text-xs text-muted hover:text-fg">previous turn's reply</summary>
      <pre class="mt-1 max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border border-line-subtle bg-raised/40 p-2.5 font-mono text-xs text-fg">{session.lastTail}</pre>
    </details>
  {/if}
</div>
