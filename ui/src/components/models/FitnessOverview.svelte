<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { cn } from '@/lib/cn'
  import { BAND_BG, BAND_META, BAND_SEVERITY, BAND_TEXT, caseCategory, pct, usd, type DetailPayload } from './fitness'

  // THE ANSWER, BEFORE THE EVIDENCE.
  //
  // Every other compartment in this report is a drill-down: per-slot verdicts,
  // per-capability probes, per-fixture transcripts, per-rule adversarial counts.
  // Each is the right depth for the question it answers and none of them answers
  // the question an admin opens the dialog with, which is "so — can I use this,
  // and what would bite me?".
  //
  // WHAT IT MAY AND MAY NOT DO. Everything here is COUNTED off the same objects
  // the other tabs draw; nothing is re-scored. A summary that computed its own
  // verdict would eventually disagree with the tab beneath it, and the summary
  // is the one people quote. Where a number is missing it says so rather than
  // showing a zero — an unmeasured tier reading as a clean sweep is the single
  // most dangerous thing this page could do.
  let { detail, onOpen }: { detail: DetailPayload; onOpen: (pane: string) => void } = $props()

  const record = $derived(detail.record)
  const slots = $derived([...(record?.report.slots ?? [])].filter((s) => s.slot.live).sort((a, b) => BAND_SEVERITY[a.band] - BAND_SEVERITY[b.band]))

  const tally = $derived.by(() => {
    const by = { ready: 0, workable: 0, unfit: 0, untested: 0, unbound: 0 }
    for (const s of slots) by[s.band]++
    return by
  })

  /** THE HEADLINE, in the vocabulary the rest of the page already uses. Read off
   *  the slot tally rather than invented: "worst band that occurs more than
   *  once" is a judgement, and this page does not make judgements the scorer did
   *  not make. */
  const headline = $derived.by(() => {
    if (!record) return null
    if (tally.unfit > 0) return { band: 'unfit' as const, line: `Not a fit for ${tally.unfit} of ${slots.length} assignable slots.` }
    if (tally.untested > 0 && tally.ready + tally.workable === 0) return { band: 'untested' as const, line: 'Nothing was measured on this model.' }
    if (tally.workable > 0) return { band: 'workable' as const, line: `Usable everywhere it was measured, with a named weakness on ${tally.workable}.` }
    return { band: 'ready' as const, line: `Ready for all ${tally.ready} assignable slots that were measured.` }
  })

  /** Slot bands as one bar, so the shape of a run reads before any number does. */
  const bar = $derived(
    (['ready', 'workable', 'unfit', 'untested', 'unbound'] as const)
      .map((band) => ({ band, n: tally[band] }))
      .filter((x) => x.n > 0),
  )

  // ── Tier 1 ─────────────────────────────────────────────────────────────────
  const probes = $derived.by(() => {
    const r = record?.probes
    if (!r) return null
    const scored = r.results.filter((x) => x.outcome.kind === 'scored' || x.outcome.kind === 'known')
    const yes = scored.filter((x) => (x.outcome.kind === 'scored' || x.outcome.kind === 'known') && x.outcome.verdict.value)
    return {
      yes: yes.length,
      no: scored.length - yes.length,
      // REUSED, NOT RE-BOUGHT. Worth saying out loud: an admin who sees "9
      // capabilities" on a run that made no tier-1 calls should know why.
      reused: r.results.filter((x) => x.outcome.kind === 'known').length,
      unmeasured: r.results.length - scored.length,
      missing: yes.length === 0 && scored.length === 0,
    }
  })

  // ── Tier 2 ─────────────────────────────────────────────────────────────────
  const fixtures = $derived.by(() => {
    const cases = record?.cases ?? []
    if (cases.length === 0) return null
    const failing = cases.filter((c) => c.skipped === null && c.gap === null && (!c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null || c.findings > 0))
    return {
      total: cases.length,
      failing: failing.length,
      gaps: cases.filter((c) => c.gap !== null).length,
      skipped: cases.filter((c) => c.skipped !== null).length,
      timedOut: cases.filter((c) => c.timedOut).length,
      findings: cases.reduce((n, c) => n + c.findings, 0),
      // WHERE it breaks, by category — the one thing a rate cannot say. Three
      // failures all in Workbench is a completely different decision from three
      // spread across everything.
      worst: [...failing.reduce((m, c) => m.set(caseCategory(c.harness).label, (m.get(caseCategory(c.harness).label) ?? 0) + 1), new Map<string, number>())]
        .sort((a, b) => b[1] - a[1])
        .slice(0, 4),
    }
  })

  /** The harness rates, worst contract first — the row an admin acts on. */
  const weakest = $derived(
    [...(record?.harnesses ?? [])]
      .filter((h) => h.cases > 0)
      // `taskScore` is NULL when nothing scorable ran — sorted last rather than
      // as a zero, because "no fixture reached a verdict" is not "it failed
      // every one".
      .sort((a, b) => a.contractRate - b.contractRate || (a.taskScore ?? 1) - (b.taskScore ?? 1))
      .slice(0, 5),
  )

  // ── What it cost, and what production says ─────────────────────────────────
  const spend = $derived.by(() => {
    const cases = record?.cases ?? []
    const usdTotal = cases.reduce((n, c) => n + (c.costUsd ?? 0), 0)
    const priced = cases.some((c) => c.costUsd !== null)
    const latencies = cases
      .filter((c) => c.latencyMs > 0)
      .map((c) => c.latencyMs)
      .sort((a, b) => a - b)
    // WALL CLOCK, WHICH IS NOT THE SUM OF THE LATENCIES. Under concurrency the
    // run is shorter than its cases add up to, and with retries a case can cost
    // far more than its recorded latency. `startedAt` makes the run a timeline,
    // so the elapsed figure is the real one rather than a sum that flatters or
    // exaggerates depending on which way the width went.
    const stamps = cases.map((c) => Date.parse(c.startedAt)).filter((n) => Number.isFinite(n) && n > 0)
    const ends = cases.map((c) => Date.parse(c.startedAt) + c.wallMs).filter((n) => Number.isFinite(n) && n > 0)
    const elapsedMs = stamps.length && ends.length ? Math.max(...ends) - Math.min(...stamps) : 0
    const wallTotal = cases.reduce((n, c) => n + c.wallMs, 0)
    return {
      usd: priced ? usdTotal : null,
      p50: latencies[Math.floor(latencies.length * 0.5)] ?? 0,
      p95: latencies[Math.floor(latencies.length * 0.95)] ?? 0,
      estimated: cases.some((c) => c.estimated),
      elapsedMs,
      // Model time bought per second of waiting. Above 1 means the width paid
      // off; at 1 the sweep was effectively sequential.
      parallelism: elapsedMs > 0 ? wallTotal / elapsedMs : 0,
      perCase: cases.length > 0 ? Math.round(wallTotal / cases.length) : 0,
    }
  })

  const mins = (ms: number): string => (ms >= 60_000 ? `${Math.round(ms / 60_000)}m` : `${Math.round(ms / 1000)}s`)

  const divergences = $derived(detail.divergences ?? [])
</script>

{#if !record}
  <EmptyState
    variant="compact"
    icon="◇"
    title={detail.live ? 'This is the first run for this model' : 'No run on record for this model'}
    hint={detail.live
      ? 'Watch it in Live. A full report is archived when it finishes.'
      : 'Run the probes to fill in what this model can do, and the harness tier to fill in the matrix.'}
  />
{:else}
  <div class="space-y-5">
    <!-- ── The verdict ─────────────────────────────────────────────────── -->
    <section>
      {#if headline}
        <div class="flex flex-wrap items-baseline gap-2">
          <span class={cn('font-mono text-base', BAND_TEXT[headline.band])}>{BAND_META[headline.band].glyph}</span>
          <span class="font-sans text-sm text-fg">{headline.line}</span>
        </div>
      {/if}
      <div class="mt-2 flex h-1.5 overflow-hidden rounded-full">
        {#each bar as b (b.band)}
          <div class={cn(BAND_BG[b.band])} style="width: {(b.n / slots.length) * 100}%" title="{b.n} {BAND_META[b.band].label}"></div>
        {/each}
      </div>
      <button
        type="button"
        onclick={() => onOpen('verdicts')}
        class="mt-2 flex flex-wrap items-center gap-1.5 text-left transition-opacity hover:opacity-80"
      >
        {#each bar as b (b.band)}
          <Chip tone={BAND_META[b.band].tone} title={BAND_META[b.band].blurb}>{b.n} {BAND_META[b.band].label}</Chip>
        {/each}
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">per slot →</span>
      </button>
    </section>

    <!-- ── The three tiers, side by side ───────────────────────────────── -->
    <section class="grid gap-3 sm:grid-cols-3">
      <!-- Tier 1 -->
      <button
        type="button"
        onclick={() => onOpen('capabilities')}
        class="rounded-md border border-line bg-raised/40 p-3 text-left transition-colors hover:border-line-strong"
      >
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Capabilities</div>
        {#if !probes}
          <div class="mt-1 font-sans text-sm text-warning">Not measured</div>
          <p class="mt-1 font-sans text-xs text-muted">Tier 1 did not run, so every capability tag on this model came from somewhere else.</p>
        {:else}
          <div class="mt-1 font-sans text-sm text-fg">{probes.yes} present · {probes.no} absent</div>
          <p class="mt-1 font-sans text-xs text-muted">
            {#if probes.unmeasured > 0}{probes.unmeasured} unmeasured (skipped or errored, not a no).{/if}
            {#if probes.reused > 0}
              {probes.reused} reused from an earlier run rather than re-bought.
            {/if}
          </p>
        {/if}
      </button>

      <!-- Tier 2 -->
      <button
        type="button"
        onclick={() => onOpen('fixtures')}
        class="rounded-md border border-line bg-raised/40 p-3 text-left transition-colors hover:border-line-strong"
      >
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Fixtures</div>
        {#if !fixtures}
          <div class="mt-1 font-sans text-sm text-warning">Not measured</div>
          <p class="mt-1 font-sans text-xs text-muted">Tier 2 did not run, so no harness contract was tested.</p>
        {:else}
          <div class={cn('mt-1 font-sans text-sm', fixtures.failing > 0 ? 'text-danger' : 'text-success')}>
            {fixtures.total - fixtures.failing}/{fixtures.total} clean
          </div>
          <p class="mt-1 font-sans text-xs text-muted">
            {#if fixtures.worst.length > 0}
              Failing in {fixtures.worst.map((w) => `${w[0]} (${w[1]})`).join(', ')}.
            {:else}
              Every fixture that ran held its contract and passed its check.
            {/if}
            {#if fixtures.timedOut > 0}<span class="text-warning"> {fixtures.timedOut} timed out.</span>{/if}
            {#if fixtures.gaps > 0}<span class="text-warning"> {fixtures.gaps} could not be asked fairly; our gap.</span>{/if}
          </p>
        {/if}
      </button>

      <!-- Tier 3 -->
      <button
        type="button"
        onclick={() => onOpen(record.adversarial ? 'adversarial' : 'verdicts')}
        class="rounded-md border border-line bg-raised/40 p-3 text-left transition-colors hover:border-line-strong"
      >
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Adversarial</div>
        {#if !record.adversarial}
          <div class="mt-1 font-sans text-sm text-warning">Not measured</div>
          <p class="mt-1 font-sans text-xs text-muted">Tier 3 did not run. Nothing here says this model is safe under provocation.</p>
        {:else}
          <div class={cn('mt-1 font-sans text-sm', BAND_TEXT[record.adversarial.band])}>
            {BAND_META[record.adversarial.band].label}
          </div>
          <p class="mt-1 font-sans text-xs text-muted">
            Resistance {record.adversarial.resistance === null ? 'unscorable' : pct(record.adversarial.resistance)}
            · {record.adversarial.rules.reduce((n, r) => n + r.elicited, 0)} provocation(s) landed.
          </p>
        {/if}
      </button>
    </section>

    <!-- ── Where it is weakest ─────────────────────────────────────────── -->
    {#if weakest.length > 0}
      <section>
        <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Weakest harnesses</div>
        <ul class="space-y-1">
          {#each weakest as h (h.id)}
            <li class="flex flex-wrap items-baseline gap-2 font-mono text-[11px]">
              <span class="w-48 shrink-0 truncate text-fg">{h.label}</span>
              <span class={h.contractRate < 0.95 ? 'text-warning' : 'text-muted'}>{pct(h.contractRate)} first try</span>
              <!-- CUMULATIVE, and the distinction that matters most: a model at
                   40% first-pass and 95% after one repair is usable; 40/45 is
                   not. Shown only where a repair turn can happen at all. -->
              {#if h.repairable && h.repairRate > h.contractRate}
                <span class="text-muted">→ {pct(h.repairRate)} after repair</span>
              {/if}
              <span class="text-muted">· task {h.taskScore === null ? 'unscored' : pct(h.taskScore)}</span>
              <span class="ml-auto text-ink-dim">{h.cases} fixture(s)</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <!-- ── Bench vs production, and what the run cost ──────────────────── -->
    <section class="grid gap-3 sm:grid-cols-2">
      <button
        type="button"
        onclick={() => onOpen('observed')}
        class="rounded-md border border-line bg-raised/40 p-3 text-left transition-colors hover:border-line-strong"
      >
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Vs production</div>
        {#if divergences.length > 0}
          <div class="mt-1 font-sans text-sm text-warning">{divergences.length} divergence(s)</div>
          <p class="mt-1 font-sans text-xs text-muted">{divergences[0]?.note ?? 'The bench and the last few days of live traffic disagree.'}</p>
        {:else if detail.observed.length === 0}
          <div class="mt-1 font-sans text-sm text-muted">No production traffic</div>
          <p class="mt-1 font-sans text-xs text-muted">Nothing has run this model in the observed window, so the bench stands alone.</p>
        {:else}
          <div class="mt-1 font-sans text-sm text-success">Bench matches production</div>
          <p class="mt-1 font-sans text-xs text-muted">{detail.observed.length} harness(es) with live traffic agree with what this run measured.</p>
        {/if}
      </button>

      <div class="rounded-md border border-line bg-raised/40 p-3">
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">This run</div>
        <div class="mt-1 font-sans text-sm text-fg">
          {usd(spend.usd)}{spend.estimated ? ' (part estimated)' : ''}
        </div>
        <p class="mt-1 font-sans text-xs text-muted">
          {#if spend.elapsedMs > 0}
            {mins(spend.elapsedMs)} wall clock · {spend.perCase}ms per fixture{#if spend.parallelism >= 1.2}
              · {spend.parallelism.toFixed(1)}× parallel{/if}.
          {/if}
          p50 {spend.p50}ms · p95 {spend.p95}ms across {record.cases.length} fixture(s), {record.tiers.join(' + ')}.
          <!-- WIDTH CHANGES WHAT THE LATENCY MEANS. At four wide the number
               includes queueing at the provider, so it is "what a call costs
               under this load" and not "what a call costs". Two runs at
               different widths are not comparable, and the page has to say so
               rather than let the same field hold two measurements. -->
          {#if record.sweep.concurrency.ended > 1}
            <span class="text-ink-dim">Measured at {record.sweep.concurrency.ended} fixtures in flight, so it includes queueing.</span>
          {/if}
        </p>
      </div>
    </section>
  </div>
{/if}
