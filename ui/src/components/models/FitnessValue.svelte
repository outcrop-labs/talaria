<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import {
    BAND_BG,
    BAND_META,
    BAND_TEXT,
    centsPerRun,
    costCaveat,
    pct,
    perDay,
    usdRate,
    workloadSentence,
    type FitnessBand,
    type ModelValue,
    type ValuePayload,
  } from './fitness'

  // PRICE AGAINST PERFORMANCE. The matrix says whether a model CAN hold a slot;
  // this says what holding it costs and how much of a real day it covers. The
  // two axes are weighted by the same runs-per-day vector — see the header of
  // `api/src/fitness/value.rs` for why neither number means anything alone.
  let { data, selected, onSelect }: { data: ValuePayload; selected: string | null; onSelect: (model: string) => void } = $props()

  type Sort = 'value' | 'cost' | 'coverage'
  let sort = $state<Sort>('value')

  const SORTS: Array<{ id: Sort; label: string; hint: string }> = [
    { id: 'value', label: 'Best value', hint: 'Cost per run the model is actually Ready for: the price-to-performance number.' },
    { id: 'cost', label: 'Cheapest day', hint: 'What your measured workload costs on this model, regardless of how much of it the model can carry.' },
    { id: 'coverage', label: 'Most coverage', hint: 'Share of your daily runs the model is Ready for, regardless of price.' },
  ]

  /** Nulls sort LAST in every mode. An unpriced model may well be the cheapest
   *  and a page that ranked it first would be guessing — the same rule
   *  the Rust value engine (api/src/fitness/value.rs) applies inside a slot's
   *  candidate list. */
  const rank = (a: ModelValue, b: ModelValue): number => {
    const asc = (x: number | null, y: number | null): number => (x === null ? (y === null ? 0 : 1) : y === null ? -1 : x - y)
    if (sort === 'cost') return asc(a.usdPerDay, b.usdPerDay) || a.model.localeCompare(b.model)
    if (sort === 'coverage') return b.readyShare - a.readyShare || asc(a.usdPerDay, b.usdPerDay)
    return asc(a.usdPerReadyRun, b.usdPerReadyRun) || b.readyShare - a.readyShare
  }
  const rows = $derived([...data.models].sort(rank))

  // ── The plot ───────────────────────────────────────────────────────────────
  //
  // LOG SCALE ON COST, and it is not a stylistic choice: a fleet's candidates
  // routinely span a 7B self-host at $0 and a frontier model at 100x the price
  // of the next one down. Linear, every model but the dearest lands in the same
  // pixel column and the chart says nothing.
  const costs = $derived(data.models.map((m) => m.usdPerDay).filter((n): n is number => n !== null && n > 0))
  const lo = $derived(costs.length ? Math.min(...costs) : 0)
  const hi = $derived(costs.length ? Math.max(...costs) : 0)

  /** Percent across the plot. Free (a local endpoint) pins to the left edge
   *  rather than falling off a log axis; a single priced model sits mid-plot,
   *  because a lone point on a scale it defines has no position to report. */
  const xOf = (usd: number | null): number => {
    if (usd === null) return 0
    if (usd <= 0) return 0
    if (hi <= lo) return 50
    return (Math.log10(usd) - Math.log10(lo)) / (Math.log10(hi) - Math.log10(lo)) * 100
  }

  const plotted = $derived(rows.filter((m) => m.usdPerDay !== null))
  const unpriced = $derived(rows.filter((m) => m.usdPerDay === null))

  const dotTone = (m: ModelValue): FitnessBand =>
    m.readyShare >= 0.9 ? 'ready' : m.readyShare > 0 ? 'workable' : m.shares.unfit > 0 ? 'unfit' : 'untested'

  const rowTitle = (m: ModelValue): string => {
    const caveat = costCaveat(m, data.unmeasured.length)
    const shares = (Object.entries(m.shares) as Array<[FitnessBand, number]>)
      .filter(([, v]) => v > 0)
      .map(([band, v]) => `${pct(v)} ${BAND_META[band].label.toLowerCase()}`)
      .join(', ')
    return `${m.model}\n${usdRate(m.usdPerDay, 'day')} across your measured workload\n${shares}${caveat ? `\n\n${caveat}` : ''}`
  }

  const BANDS: FitnessBand[] = ['ready', 'workable', 'unfit', 'untested', 'unbound']
</script>

<p class="mb-3 max-w-prose font-sans text-xs text-muted">{workloadSentence(data.workload)}</p>

{#if data.models.length === 0}
  <EmptyState
    icon="◇"
    title="No model has been tested yet"
    hint="Cost is easy to get; what it buys is not. Test a model above and it appears here with what a day of your workload would cost on it."
  />
{:else}
  <!-- ── The plot ──────────────────────────────────────────────────────────
       Cost across, coverage up. The corner an admin wants is top-LEFT: covers
       the day, costs the least. -->
  {#if data.priced}
    <!-- THE PLOT AREA IS INSET FROM THE BOX, and that is the overflow fix rather
         than a clipping one. A point at 0% or 100% sits ON the axis, so anything
         drawn around it — the dot's own width, its label — hung outside the
         border and over the panel beside it. Clipping would have hidden the
         cheapest and the dearest model, which are the two an admin is looking
         for. The frame keeps a gutter on every side instead, so an extreme point
         has somewhere to be. -->
    <div class="rounded-lg border border-line p-3">
      <div class="relative h-60">
        <!-- Y axis title, along the axis it names. -->
        <span
          class="absolute top-1/2 left-0 -translate-y-1/2 [writing-mode:vertical-rl] [text-orientation:mixed] rotate-180 font-mono text-[9px] tracking-[0.06em] text-ink-dim uppercase"
        >
          Ready share of your day
        </span>

        <div class="absolute top-2 right-4 bottom-8 left-16 border-b border-l border-line">
          {#each [1, 0.75, 0.5, 0.25] as line (line)}
            <div class="absolute inset-x-0 border-t border-line-subtle" style:bottom="{line * 100}%">
              <span class="absolute -top-2 -left-1 -translate-x-full font-mono text-[9px] text-ink-dim">{pct(line)}</span>
            </div>
          {/each}

          {#each plotted as m (m.model)}
            <!-- The BUTTON is the point, centred on (cost, coverage) — the label
                 is absolutely positioned under it and out of the layout, so it
                 cannot drag the dot off its own coordinates the way an inline
                 flex row did. -->
            <button
              type="button"
              onclick={() => onSelect(m.model)}
              title={rowTitle(m)}
              aria-label="{m.model}: {pct(m.readyShare)} ready, {usdRate(m.usdPerDay, 'day')}"
              class={cn('absolute h-3 w-3 -translate-x-1/2 translate-y-1/2 rounded-full leading-none', focusGold)}
              style:left="{xOf(m.usdPerDay)}%"
              style:bottom="{m.readyShare * 100}%"
            >
              <span class={cn('text-[13px] leading-none', BAND_TEXT[dotTone(m)])}>{selected === m.model ? '◉' : '●'}</span>
              <span
                class={cn(
                  'pointer-events-none absolute top-3.5 left-1/2 max-w-28 -translate-x-1/2 truncate font-mono text-[9px]',
                  selected === m.model ? 'text-fg' : 'text-muted',
                )}
              >
                {m.model.split('/').at(-1)}
              </span>
            </button>
          {/each}
        </div>

        <!-- X axis: the scale ends and the title, under the plot area only. -->
        <div class="absolute right-4 bottom-0 left-16 flex items-baseline justify-between font-mono text-[9px] text-ink-dim">
          <span>{lo > 0 ? usdRate(lo, 'day') : 'free'}</span>
          <span class="tracking-[0.06em] text-muted uppercase">Cost of your day (log scale) →</span>
          <span>{usdRate(hi, 'day')}</span>
        </div>
      </div>

      {#if unpriced.length > 0}
        <p class="mt-2 font-sans text-[11px] text-muted">
          {unpriced.length} tested model{unpriced.length === 1 ? ' is' : 's are'} off the chart: nothing on this install prices
          {unpriced.map((m) => m.model).join(', ')}. Set a price on the endpoint to place {unpriced.length === 1 ? 'it' : 'them'}.
        </p>
      {/if}
    </div>
  {:else}
    <p class="rounded-lg border border-line p-3 font-sans text-xs text-muted">
      Nothing on this install prices any tested model, so there is no cost axis to draw. The coverage half of the table below does not
      depend on a price catalog.
    </p>
  {/if}

  <!-- ── The table ─────────────────────────────────────────────────────────── -->
  <div class="mt-3 mb-2 flex flex-wrap items-center gap-1.5">
    {#each SORTS as s (s.id)}
      <button
        type="button"
        onclick={() => (sort = s.id)}
        title={s.hint}
        class={cn(
          'rounded-full border px-2 py-0.5 font-mono text-[10px] transition-colors',
          sort === s.id ? 'border-accent/40 bg-accent/10 text-fg' : 'border-line text-muted dither-fill',
          focusGold,
        )}
      >
        {s.label}
      </button>
    {/each}
  </div>

  <div class="overflow-x-auto rounded-lg border border-line">
    <table class="w-full border-collapse text-left">
      <thead>
        <tr class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <th class="px-3 py-2 font-normal">Model</th>
          <th class="px-3 py-2 text-right font-normal" title="What the provider charges per million tokens, in and out. Sticker price; it is not comparable across models on its own, which is what the next column is for.">$/MTok</th>
          <th class="px-3 py-2 text-right font-normal" title="Your measured workload: the runs your agents actually did, priced on this model.">Your day</th>
          <th class="px-3 py-2 text-right font-normal" title="Cost divided by the runs this model is Ready for. The price-to-performance number: a cheap model that can only carry a tenth of your day is not cheap.">Per ready run</th>
          <th class="px-3 py-2 font-normal" title="Share of your daily runs by verdict. Grey is never-measured, which is not a pass.">Coverage of your day</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as m (m.model)}
          {@const caveat = costCaveat(m, data.unmeasured.length)}
          <tr class={cn('border-b border-line-subtle transition-colors last:border-0 dither-fill', selected === m.model && 'bg-card')}>
            <th scope="row" class="p-0 font-normal">
              <!-- Same affordance as the matrix row: a hover state on the LINK
                   rather than only on the row, so it is clear which part opens
                   something. -->
              <button
                type="button"
                onclick={() => onSelect(m.model)}
                title="Open {m.model} for per-slot verdicts, capabilities and every failing fixture"
                class={cn('group/row flex w-full cursor-pointer flex-col items-start gap-0.5 px-3 py-2 text-left', focusGold)}
              >
                <span class="flex min-w-0 items-baseline gap-1.5">
                  <span
                    class="max-w-[20rem] truncate font-mono text-xs text-fg underline decoration-dotted decoration-line underline-offset-2 transition-colors group-hover/row:decoration-accent"
                  >
                    {m.model}
                  </span>
                  <span class="shrink-0 font-mono text-[10px] text-ink-dim opacity-0 transition-opacity group-hover/row:opacity-100">open →</span>
                </span>
                <span class="font-mono text-[10px] text-ink-dim">
                  {m.at ? `tested ${new Date(m.at).toLocaleDateString()}` : 'never tested'}
                </span>
              </button>
            </th>
            <td class="px-3 py-2 text-right font-mono text-[11px] text-muted whitespace-nowrap">
              {#if m.price}
                ${m.price.in.toFixed(2)} / ${m.price.out.toFixed(2)}
              {:else}
                <span class="text-ink-dim">unpriced</span>
              {/if}
            </td>
            <td class="px-3 py-2 text-right font-mono text-[11px] whitespace-nowrap" title={caveat ?? undefined}>
              <span class={m.usdPerDay === null ? 'text-ink-dim' : 'text-fg'}>{usdRate(m.usdPerDay, 'day')}</span>
              <!-- A floor is marked where it is read, not in a footnote. -->
              {#if caveat}<span class="text-warning">*</span>{/if}
            </td>
            <td class="px-3 py-2 text-right font-mono text-[11px] whitespace-nowrap">
              <span class={m.usdPerReadyRun === null ? 'text-ink-dim' : 'text-fg'}>{centsPerRun(m.usdPerReadyRun)}</span>
            </td>
            <td class="px-3 py-2">
              <span class="flex h-2 w-full min-w-40 overflow-hidden rounded-full bg-card2" role="img" aria-label="{pct(m.readyShare)} ready">
                {#each BANDS as band (band)}
                  {#if m.shares[band] > 0}
                    <span
                      class={BAND_BG[band]}
                      style:width="{m.shares[band] * 100}%"
                      title="{pct(m.shares[band])} of your daily runs: {BAND_META[band].label}"
                    ></span>
                  {/if}
                {/each}
              </span>
              <span class="mt-1 flex flex-wrap gap-x-2 font-mono text-[10px]">
                <span class={BAND_TEXT.ready}>{pct(m.readyShare)} ready</span>
                {#if m.shares.workable > 0}<span class={BAND_TEXT.workable}>{pct(m.shares.workable)} workable</span>{/if}
                {#if m.shares.unfit > 0}<span class={BAND_TEXT.unfit}>{pct(m.shares.unfit)} unfit</span>{/if}
                {#if m.shares.untested + m.shares.unbound > 0}
                  <span class="text-ink-dim">{pct(m.shares.untested + m.shares.unbound)} untested</span>
                {/if}
              </span>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  {#if rows.some((m) => costCaveat(m, data.unmeasured.length) !== null)}
    <p class="mt-2 max-w-prose font-sans text-[11px] text-muted">
      <span class="text-warning">*</span> a floor, not a total; hover the figure for what is missing from it.
      {#if data.unmeasured.length > 0}
        {data.unmeasured.length} harness{data.unmeasured.length === 1 ? '' : 'es'} carrying real traffic
        ({data.unmeasured.slice(0, 4).join(', ')}{data.unmeasured.length > 4 ? ', …' : ''}) have never had their tokens measured, so no
        run can price them.
      {/if}
    </p>
  {/if}

  <!-- ── Per slot ──────────────────────────────────────────────────────────
       The actionable half. "Which model should hold Research" is answered by
       the cheapest one that clears Research's floor, priced on RESEARCH's
       share of the day rather than on the whole workload. -->
  {#if data.slots.some((s) => s.candidates.length > 0)}
    <h4 class="mt-5 mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Cheapest model that clears each slot</h4>
    <div class="overflow-x-auto rounded-lg border border-line">
      <table class="w-full border-collapse text-left">
        <thead>
          <tr class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            <th class="px-3 py-2 font-normal">Slot</th>
            <th class="px-3 py-2 text-right font-normal" title="Runs a day across the harnesses bound to this slot. A harness serving two slots counts in both; this is demand per slot, not a partition of your day.">Runs/day</th>
            <th class="px-3 py-2 font-normal">Cheapest Ready</th>
            <th class="px-3 py-2 text-right font-normal" title="What this slot's own harnesses cost per day on that model, not the model's whole-workload bill.">This slot costs</th>
            <th class="px-3 py-2 font-normal">Also clears it</th>
          </tr>
        </thead>
        <tbody>
          {#each data.slots.filter((s) => s.perDay > 0 || s.candidates.length > 0) as slot (slot.key)}
            {@const best = slot.candidates.find((c) => c.model === slot.best)}
            <tr class="border-b border-line-subtle last:border-0 dither-fill">
              <th scope="row" class="px-3 py-2 font-sans text-xs font-normal text-fg">
                {slot.label}
              </th>
              <td class="px-3 py-2 text-right font-mono text-[11px] text-muted">{perDay(slot.perDay)}</td>
              <td class="px-3 py-2">
                {#if best}
                  <button type="button" onclick={() => onSelect(best.model)} class={cn('rounded font-mono text-[11px] text-fg', focusGold)}>
                    {best.model}
                  </button>
                {:else}
                  <span class="font-mono text-[11px] text-ink-dim" title="Nothing tested reaches Ready here. That is a finding, not a gap; test another model, or read the reason in the matrix above.">
                    nothing tested is Ready
                  </span>
                {/if}
              </td>
              <td class="px-3 py-2 text-right font-mono text-[11px] whitespace-nowrap">
                {best ? usdRate(best.usdPerDay, 'day') : '—'}
              </td>
              <td class="px-3 py-2">
                <span class="flex flex-wrap gap-1">
                  {#each slot.candidates.filter((c) => c.model !== slot.best).slice(0, 3) as c (c.model)}
                    <Chip
                      tone={BAND_META[c.band].tone}
                      title="{c.model}: {BAND_META[c.band].label}. This slot would cost {usdRate(c.usdPerDay, 'day')} on it."
                    >
                      {c.model.split('/').at(-1)}
                    </Chip>
                  {/each}
                  {#if slot.candidates.length === 0}
                    <span class="font-mono text-[10px] text-ink-dim">—</span>
                  {/if}
                </span>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}
