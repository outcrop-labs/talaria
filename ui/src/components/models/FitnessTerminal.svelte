<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { EvalLogLine, LiveRun } from './fitness'

  // THE SWEEP AS IT HAPPENS, as a log.
  //
  // WHY A TERMINAL AND NOT A LIST. The drill-down beside this one shows what
  // FAILED, which is the right shape for reading a finished run and the wrong
  // shape for watching one: a sweep that is flying and a sweep that is wedged
  // look identical in a list of failures, because neither is adding to it. What
  // an admin watching a run wants is the thing every long job has ever given
  // them — lines arriving, in order, with the slow one visibly taking its time.
  //
  // It is also where a TIMEOUT becomes legible. A red cell saying "the case did
  // not finish inside 60000ms" tells you nothing; forty lines of 900ms followed
  // by six of 60000ms tells you the provider fell over at a particular moment,
  // which is a completely different diagnosis from "this model is slow".
  //
  // AND IT OPENS AND CLOSES. A console is the right first thing to look at while
  // a sweep runs and the wrong thing to keep half the pane once you have moved
  // on to reading a failure, so it collapses to its summary line — the counts and
  // whatever is running right now, which is the part worth keeping on screen.
  let { live, open = $bindable(true) }: { live: LiveRun; open?: boolean } = $props()

  // FOLLOW THE TAIL, but stop the moment the reader scrolls up — a log that
  // yanks you back to the bottom while you are reading the line you paused on is
  // worse than one that does not follow at all.
  let pane = $state<HTMLElement | null>(null)
  let follow = $state(true)

  const onScroll = () => {
    if (!pane) return
    follow = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 40
  }

  $effect(() => {
    // Depend on the line count so this re-runs as lines land.
    void live.log.length
    if (follow && pane) pane.scrollTop = pane.scrollHeight
  })

  const VERDICT: Record<EvalLogLine['verdict'], { mark: string; tone: string; word: string }> = {
    pass: { mark: '✓', tone: 'text-success', word: 'pass' },
    fail: { mark: '✕', tone: 'text-danger', word: 'FAIL' },
    // OURS, not the model's — the fixture could not fairly ask its question. It
    // gets its own mark and its own colour because reading it as a model failure
    // is exactly the mistake the `gap` verdict exists to prevent.
    gap: { mark: '⚑', tone: 'text-warning', word: 'GAP ' },
    error: { mark: '!', tone: 'text-danger', word: 'err ' },
    timeout: { mark: '⏱', tone: 'text-warning', word: 'time' },
    skip: { mark: '–', tone: 'text-ink-dim', word: 'skip' },
  }

  /** Slow is relative to what this sweep has been doing, not to a number
   *  somebody picked: a 3s call is unremarkable on a frontier model over a tool
   *  loop and alarming on a local 7B doing single-shot JSON. Anything past four
   *  times the median gets marked. */
  const slowAbove = $derived.by(() => {
    const ms = live.log.map((l) => l.ms).filter((n) => n > 0).sort((a, b) => a - b)
    const median = ms[Math.floor(ms.length / 2)] ?? 0
    return median > 0 ? median * 4 : Infinity
  })

  const counts = $derived.by(() => {
    const by: Record<EvalLogLine['verdict'], number> = { pass: 0, fail: 0, gap: 0, error: 0, timeout: 0, skip: 0 }
    for (const l of live.log) by[l.verdict]++
    return by
  })

  const pad = (s: string, n: number): string => (s.length > n ? `${s.slice(0, n - 1)}…` : s.padEnd(n))

  /** TIER 1 AND TIER 3 SHARE THIS FEED, and a watcher has to be able to tell
   *  them apart at a glance: "tools" as a probe and "tools" as a fixture are
   *  different claims. The verdict column stays in the common vocabulary — a
   *  probe that failed and a fixture that failed are both red — and only the
   *  SOURCE column is tinted, which is the column that says which is which. */
  const TIER_TONE: Record<string, string> = { probes: 'text-accent', adversarial: 'text-warning' }
  const sourceTone = (harness: string): string => TIER_TONE[harness] ?? 'text-muted'

  // ── The case running right now ─────────────────────────────────────────────
  //
  // A TICKING CLOCK IS THE POINT. A sweep that is working and a sweep that is
  // wedged produce the same still image; "running for 4s" turning into "running
  // for 90s" on a 60s-per-turn budget is the difference, and it is readable at a
  // glance without knowing any of the numbers behind it.
  //
  // SEVERAL AT ONCE, because a sweep runs `concurrency` cases in parallel. Every
  // one is listed: showing one of four would make three quarters of a working
  // sweep look idle, and the case that is stuck is not reliably the first.
  let now = $state(Date.now())
  $effect(() => {
    if (live.current.length === 0) return
    const t = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(t)
  })
  const secondsOf = (startedAt: number): number => Math.max(0, Math.round((now - startedAt) / 1000))

  /** WHICH RUNNING CASE HAS ITS TURNS OPEN — per case, and closed by default.
   *
   *  THE BUG THIS REPLACES, which the screenshot showed exactly: the turn lists
   *  rendered on `open`, the CONSOLE'S toggle, so opening the console unfurled
   *  every running case at once. At four wide that is four transcripts stacked
   *  between the header and the log, and the log — the thing the toggle is named
   *  after — was pushed off the bottom. One control silently drove five things.
   *
   *  A plain record rather than a Set: `$state` tracks property writes on an
   *  object, and a Set has to be reassigned to be reactive at all. */
  let openTurns = $state<Record<string, boolean>>({})
  const keyOf = (c: { harness: string; case: string }): string => `${c.harness}::${c.case}`

  const ROLE_TONE: Record<string, string> = {
    system: 'text-ink-dim',
    user: 'text-muted',
    assistant: 'text-success',
    tool: 'text-accent',
  }
</script>

<!-- A CARD, not a bare stack. The console used to render as an unframed header
     row, a strip and a log pane, directly above a column of `Disclosure` cards —
     so collapsed it looked like one more accordion in that list, and open it
     looked like the list had grown a lid. It is a different thing from the
     failures beneath it and now says so with the same hairline card the rest of
     the surface uses.

     IT SIZES ITSELF AND IS BOUNDED. No `h-full`, no `flex-1`, no percentage: it
     is a `shrink-0` block in the Live pane and every scrolling part inside it
     carries its own max height. Height that depended on a parent chain is what
     made the collapsed console fold to nothing and the open one overflow into
     the list beneath it. -->
<div class="flex flex-col overflow-hidden rounded-md border border-line bg-card">
  <div class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.05em]">
    <button
      type="button"
      onclick={() => (open = !open)}
      class="-ml-1 flex items-center gap-1.5 rounded px-1 py-0.5 text-muted transition-colors hover:bg-hover hover:text-fg"
      aria-expanded={open}
    >
      <span class={cn('text-[9px] transition-transform duration-150', open && 'rotate-90')}>▶</span>
      console
    </button>
    <span class="text-muted">{live.phase === 'scoring' ? 'scoring' : (live.phase ?? 'running')}</span>
    <span class="text-muted">{live.done}/{live.total || '?'}</span>
    {#if counts.pass}<span class="text-success">{counts.pass} pass</span>{/if}
    {#if counts.fail}<span class="text-danger">{counts.fail} fail</span>{/if}
    {#if counts.error}<span class="text-danger">{counts.error} err</span>{/if}
    {#if counts.timeout}<span class="text-warning">{counts.timeout} timeout</span>{/if}
    {#if counts.gap}<span class="text-warning">{counts.gap} our gap</span>{/if}
    {#if counts.skip}<span class="text-ink-dim">{counts.skip} skipped</span>{/if}
    {#if open}
      <span class="ml-auto">
        <button
          type="button"
          onclick={() => {
            follow = true
            if (pane) pane.scrollTop = pane.scrollHeight
          }}
          class={cn('rounded px-1.5 py-0.5 transition-colors', follow ? 'text-ink-dim' : 'text-accent hover:bg-hover')}
        >
          {follow ? 'following' : 'jump to end'}
        </button>
      </span>
    {/if}
  </div>

  <!-- RUNNING NOW, and it stays visible when the console is closed. This is the
       line that answers "is it stuck", so hiding it behind the toggle would
       collapse away the only part of a console that has to stay on screen. -->
  {#if live.current.length > 0}
    <!-- BOUNDED. Four concurrent cases with an open transcript each must not be
         able to push the log out of the console. -->
    <div class="max-h-[40vh] shrink-0 space-y-1 overflow-y-auto border-t border-line px-3 py-1.5">
      {#each live.current as c (keyOf(c))}
        {@const elapsed = secondsOf(c.startedAt)}
        {@const turnsOpen = openTurns[keyOf(c)] === true}
        <div>
          <!-- THE ROW IS THE TOGGLE for its OWN turns and nothing else's, and it
               starts closed. Four running cases each unfurling a system prompt is
               the console losing its log. -->
          <button
            type="button"
            onclick={() => (openTurns = { ...openTurns, [keyOf(c)]: !turnsOpen })}
            disabled={c.turns.length === 0}
            aria-expanded={turnsOpen}
            class="flex w-full flex-wrap items-baseline gap-x-2 gap-y-1 rounded px-1 py-0.5 text-left font-mono text-[11px] transition-colors enabled:hover:bg-hover"
          >
            <span class={cn('text-accent transition-transform duration-150', turnsOpen && 'rotate-90')}>▶</span>
            <span class="text-muted">{c.harness}</span>
            <span class="min-w-0 flex-1 truncate text-fg">{c.case}</span>
            {#if c.maxTurns > 1}<span class="text-ink-dim">turn {c.turn}/{c.maxTurns}</span>{/if}
            {#if c.open > 0}<span class="text-warning">waiting on the model</span>{/if}
            <span class={elapsed > 60 ? 'text-warning' : 'text-ink-dim'}>{elapsed}s</span>
          </button>
          {#if turnsOpen && c.turns.length > 0}
            <!-- THE TURNS AS THEY HAPPEN. On a tool loop this is the only place
                 the conversation is visible before the case finishes — and a case
                 that never finishes is exactly the one whose conversation you
                 want. -->
            <ol class="mt-1 max-h-40 space-y-1 overflow-auto rounded border border-line-subtle bg-raised/40 p-1.5">
              {#each c.turns as t, i (i)}
                <li class="font-mono text-[10px] leading-4">
                  <span class={cn('uppercase tracking-[0.08em]', ROLE_TONE[t.role] ?? 'text-muted')}>{t.role}</span>
                  {#if t.toolCalls?.length}<span class="text-accent"> → {t.toolCalls.join(', ')}</span>{/if}
                  {#if t.content.trim()}
                    <div class="whitespace-pre-wrap break-words text-muted">{t.content}</div>
                  {:else}
                    <div class="text-ink-dim">(no prose — the turn was tool calls)</div>
                  {/if}
                </li>
              {/each}
            </ol>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if open}
    <div
      bind:this={pane}
      onscroll={onScroll}
      class="max-h-[38vh] min-h-[8rem] overflow-auto border-t border-line bg-black/40 p-3 font-mono text-[11px] leading-[1.45]"
    >
      {#if live.log.length === 0}
        <!-- Not "the first fixture": tier 1 runs before any fixture does, and a
             console that says it is waiting for something that is not next is
             the kind of small lie that makes a watcher distrust the rest. -->
        <div class="text-ink-dim">waiting for the first result to land…</div>
      {:else}
        {#each live.log as l, i (`${l.harness}::${l.case}::${i}`)}
          {@const v = VERDICT[l.verdict]}
          <div class="whitespace-pre-wrap break-words">
            <span class="text-ink-dim">{String(i + 1).padStart(3, ' ')}</span>
            <span class={v.tone}> {v.mark} {v.word}</span>
            <span class={sourceTone(l.harness)}> {pad(l.harness, 20)}</span>
            <span class="text-fg">{l.case}</span>
            <span class={l.ms > slowAbove ? 'text-warning' : 'text-ink-dim'}> {l.ms}ms</span>
            {#if l.tokens > 0}<span class="text-ink-dim"> {l.tokens}tok</span>{/if}
            {#if l.calls > 0}<span class="text-accent"> {l.calls} tools</span>{/if}
            <!-- THE EVIDENCE BEHIND A TIMEOUT, on the line itself. "1 up, 1 open"
                 is a request that never came back; "4 up" is a case that spent
                 its budget retrying; nothing at all is time that went somewhere
                 before the provider was ever reached. -->
            {#if l.up}
              <span class={l.up.open > 0 ? 'text-warning' : 'text-ink-dim'}> {l.up.calls} up{l.up.open > 0 ? `, ${l.up.open} open` : ''}</span>
            {/if}
            {#if l.note}
              <div class={cn('pl-8', l.verdict === 'gap' ? 'text-warning' : l.verdict === 'skip' ? 'text-ink-dim' : 'text-danger')}>↳ {l.note}</div>
            {/if}
          </div>
        {/each}
        <div class="mt-1 text-ink-dim">
          {#if live.state === 'running'}<span class="animate-pulse">▊</span>{:else}— run {live.state} —{/if}
        </div>
      {/if}
    </div>
  {/if}
</div>
