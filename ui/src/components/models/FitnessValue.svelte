<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { cn } from '@/lib/cn'
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
  import { useModelNames } from './model-names.svelte'

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

  const modelName = useModelNames()
  /** What a point, a row or a chip calls a model: the catalog's name, else the
   *  id without its provider prefix. */
  const shortName = (model: string) => modelName(model) ?? model.split('/').at(-1) ?? model

  const plotted = $derived(rows.filter((m) => m.usdPerDay !== null))
  const unpriced = $derived(rows.filter((m) => m.usdPerDay === null))

  // A point's label sits under it. Where two models land close together the
  // second label would print over the first, so it goes above its point.
  const LABEL = { charPx: 7, maxPx: 144, heightPx: 16, gapPx: 6 }
  let plotW = $state(0)
  let plotH = $state(0)
  const labelSide = $derived.by(() => {
    type Box = { left: number; right: number; top: number; bottom: number }
    const placed: Box[] = []
    const sides = new Map<string, 'below' | 'above'>()
    const hits = (b: Box) => placed.some((p) => b.left < p.right && b.right > p.left && b.top < p.bottom && b.bottom > p.top)
    for (const m of [...plotted].sort((a, b) => xOf(a.usdPerDay) - xOf(b.usdPerDay))) {
      const width = Math.min(LABEL.maxPx, shortName(m.model).length * LABEL.charPx)
      const x = (xOf(m.usdPerDay) / 100) * plotW
      const y = (1 - m.readyShare) * plotH
      const box = (side: 'below' | 'above'): Box => {
        const top = side === 'below' ? y + LABEL.gapPx : y - LABEL.gapPx - LABEL.heightPx
        return { left: x - width / 2, right: x + width / 2, top, bottom: top + LABEL.heightPx }
      }
      const side = hits(box('below')) && !hits(box('above')) ? 'above' : 'below'
      placed.push(box(side))
      sides.set(m.model, side)
    }
    return sides
  })

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

  const focus = 'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent'
  const th = 'px-3 py-2.5 font-medium'
  const num = 'whitespace-nowrap px-3 py-3 text-right font-sans text-[13px] tabular-nums'
</script>

{#if data.models.length === 0}
  <EmptyState
    icon="◇"
    title="No model has been tested yet"
    hint="Test a model and it appears here with what a day of your workload would cost on it."
  />
{:else}
  <div class="space-y-6">
    <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-muted">{workloadSentence(data.workload)}</p>

    <!-- ── The plot ────────────────────────────────────────────────────────
         Cost across, coverage up. The corner an admin wants is top-LEFT:
         covers the day, costs the least. -->
    {#if data.priced}
      <figure class="rounded-lg border border-line bg-panel px-5 py-4">
        <figcaption>
          <div class="font-sans text-sm font-medium leading-5 text-fg">Coverage against cost</div>
          <div class="font-sans text-[13px] leading-5 text-muted">
            Up is the share of your day a model is ready for. Right is what that day would cost. Top left is best.
          </div>
        </figcaption>
        <!-- THE PLOT AREA IS INSET FROM THE BOX, and that is the overflow fix
             rather than a clipping one. A point at 0% or 100% sits ON the axis,
             so its dot and label need a gutter to be drawn in. Clipping would
             hide the cheapest and the dearest model, which are the two an admin
             is looking for. -->
        <div class="relative mt-4 h-60">
          <div bind:clientWidth={plotW} bind:clientHeight={plotH} class="absolute bottom-10 left-16 right-16 top-2 border-b border-l border-line">
            {#each [1, 0.75, 0.5, 0.25] as line (line)}
              <div class="absolute inset-x-0 border-t border-line-subtle" style:bottom="{line * 100}%">
                <span class="absolute -left-2 -top-2 -translate-x-full font-sans text-xs leading-4 tabular-nums text-muted">{pct(line)}</span>
              </div>
            {/each}

            {#each plotted as m (m.model)}
              <!-- The BUTTON is the point, centred on (cost, coverage). The
                   label is positioned under it and out of the layout, so it
                   cannot drag the dot off its own coordinates. -->
              <button
                type="button"
                onclick={() => onSelect(m.model)}
                title={rowTitle(m)}
                aria-label="{m.model}: {pct(m.readyShare)} ready, {usdRate(m.usdPerDay, 'day')}"
                class={cn(
                  'absolute h-2.5 w-2.5 -translate-x-1/2 translate-y-1/2 rounded-full',
                  BAND_BG[dotTone(m)],
                  selected === m.model && 'ring-2 ring-fg ring-offset-2 ring-offset-[var(--theme-panel)]',
                  focus,
                )}
                style:left="{xOf(m.usdPerDay)}%"
                style:bottom="{m.readyShare * 100}%"
              >
                <span
                  class={cn(
                    'pointer-events-none absolute left-1/2 max-w-36 -translate-x-1/2 truncate text-xs leading-4',
                    labelSide.get(m.model) === 'above' ? 'bottom-3.5' : 'top-3.5',
                    modelName(m.model) ? 'font-sans' : 'font-mono',
                    selected === m.model ? 'text-fg' : 'text-muted',
                  )}
                >
                  {shortName(m.model)}
                </span>
              </button>
            {/each}
          </div>

          <!-- X axis: the scale's ends and its name, under the plot area only. -->
          <div class="absolute bottom-0 left-16 right-16 flex items-baseline justify-between font-sans text-xs leading-4 tabular-nums text-muted">
            <span>{lo > 0 ? usdRate(lo, 'day') : 'Free'}</span>
            <span>Cost of your day, log scale</span>
            <span>{usdRate(hi, 'day')}</span>
          </div>
        </div>

        {#if unpriced.length > 0}
          <p class="mt-3 font-sans text-[13px] leading-5 text-muted">
            {unpriced.length} tested model{unpriced.length === 1 ? ' is' : 's are'} off the chart because nothing on this install prices
            {unpriced.map((m) => shortName(m.model)).join(', ')}. Set a price on the provider to place {unpriced.length === 1 ? 'it' : 'them'}.
          </p>
        {/if}
      </figure>
    {:else}
      <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
        Nothing on this install prices any tested model, so there is no cost axis to draw. The coverage column below does not depend on
        a price.
      </p>
    {/if}

    <!-- ── The table ───────────────────────────────────────────────────── -->
    <section>
      <div class="mb-3 flex flex-wrap items-center gap-3">
        <h3 class="font-sans text-[15px] font-semibold leading-5 text-fg">What a day costs on each model</h3>
        <Segmented
          class="ml-auto"
          options={SORTS.map((s) => ({ id: s.id, label: s.label, title: s.hint }))}
          value={sort}
          onChange={(id) => (sort = id)}
        />
      </div>
      <div class="overflow-x-auto rounded-lg border border-line bg-panel">
        <table class="w-full border-collapse text-left">
          <thead>
            <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
              <th class="px-5 py-2.5 font-medium">Model</th>
              <th class={cn(th, 'text-right')} title="What the provider charges per million tokens, in and out. Sticker price; it is not comparable across models on its own, which is what the next column is for.">Per 1M tokens</th>
              <th class={cn(th, 'text-right')} title="Your measured workload: the runs your agents actually did, priced on this model.">Your day</th>
              <th class={cn(th, 'text-right')} title="Cost divided by the runs this model is Ready for. The price-to-performance number: a cheap model that can only carry a tenth of your day is not cheap.">Per ready run</th>
              <th class="py-2.5 pl-3 pr-5 font-medium" title="Share of your daily runs by verdict. Gray is never-measured, which is not a pass.">Coverage of your day</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as m (m.model)}
              {@const caveat = costCaveat(m, data.unmeasured.length)}
              <tr class="border-b border-line-subtle transition-colors last:border-0 hover:bg-hover">
                <th scope="row" class="p-0 font-normal">
                  <button
                    type="button"
                    onclick={() => onSelect(m.model)}
                    title="Open {m.model} for per-slot verdicts, capabilities and every failing fixture"
                    class={cn('flex w-full cursor-pointer flex-col items-start gap-0.5 px-5 py-3 text-left', focus)}
                  >
                    <span
                      class={cn('max-w-[20rem] truncate leading-5 text-fg', modelName(m.model) ? 'font-sans text-sm font-medium' : 'font-mono text-[13px]')}
                      title={m.model}
                    >
                      {modelName(m.model) ?? m.model}
                    </span>
                    <span class="font-sans text-xs leading-4 text-muted">
                      {m.at ? `Tested ${new Date(m.at).toLocaleDateString()}` : 'Never tested'}
                    </span>
                  </button>
                </th>
                <td class={cn(num, 'text-muted')}>
                  {#if m.price}
                    ${m.price.in.toFixed(2)} in · ${m.price.out.toFixed(2)} out
                  {:else}
                    No price
                  {/if}
                </td>
                <td class={num} title={caveat ?? undefined}>
                  <span class={m.usdPerDay === null ? 'text-muted' : 'text-fg'}>{usdRate(m.usdPerDay, 'day')}</span>
                  <!-- A floor is marked where it is read, not in a footnote. -->
                  {#if caveat}<span class="text-warning">*</span>{/if}
                </td>
                <td class={num}>
                  <span class={m.usdPerReadyRun === null ? 'text-muted' : 'text-fg'}>{centsPerRun(m.usdPerReadyRun)}</span>
                </td>
                <td class="py-3 pl-3 pr-5">
                  <span class="flex h-1.5 w-full min-w-40 overflow-hidden rounded-full bg-card2" role="img" aria-label="{pct(m.readyShare)} ready">
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
                  <span class="mt-1.5 flex flex-wrap gap-x-2 font-sans text-xs leading-4 tabular-nums">
                    <span class={BAND_TEXT.ready}>{pct(m.readyShare)} ready</span>
                    {#if m.shares.workable > 0}<span class={BAND_TEXT.workable}>{pct(m.shares.workable)} workable</span>{/if}
                    {#if m.shares.unfit > 0}<span class={BAND_TEXT.unfit}>{pct(m.shares.unfit)} not a fit</span>{/if}
                    {#if m.shares.untested + m.shares.unbound > 0}
                      <span class="text-muted">{pct(m.shares.untested + m.shares.unbound)} untested</span>
                    {/if}
                  </span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      {#if rows.some((m) => costCaveat(m, data.unmeasured.length) !== null)}
        <p class="mt-3 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
          <span class="text-warning">*</span> A floor, not a total. Hover the figure for what is missing from it.
          {#if data.unmeasured.length > 0}
            {data.unmeasured.length} harness{data.unmeasured.length === 1 ? '' : 'es'} carrying real traffic
            (<span class="font-mono text-xs">{data.unmeasured.slice(0, 4).join(', ')}</span>{data.unmeasured.length > 4 ? ' and more' : ''}) have never had their tokens measured, so no
            run can price them.
          {/if}
        </p>
      {/if}
    </section>

    <!-- ── Per slot ────────────────────────────────────────────────────────
         The actionable half. "Which model should hold Research" is answered
         by the cheapest one that clears Research's floor, priced on
         RESEARCH's share of the day rather than on the whole workload. -->
    {#if data.slots.some((s) => s.candidates.length > 0)}
      <section>
        <h3 class="mb-3 font-sans text-[15px] font-semibold leading-5 text-fg">Cheapest model that is ready for each job</h3>
        <div class="overflow-x-auto rounded-lg border border-line bg-panel">
          <table class="w-full border-collapse text-left">
            <thead>
              <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
                <th class="px-5 py-2.5 font-medium">Job</th>
                <th class={cn(th, 'text-right')} title="Runs a day across the harnesses bound to this slot. A harness serving two slots counts in both; this is demand per slot, not a partition of your day.">Runs a day</th>
                <th class={th}>Cheapest ready</th>
                <th class={cn(th, 'text-right')} title="What this slot's own harnesses cost per day on that model, not the model's whole-workload bill.">This job costs</th>
                <th class="py-2.5 pl-3 pr-5 font-medium">Also ready or workable</th>
              </tr>
            </thead>
            <tbody>
              {#each data.slots.filter((s) => s.perDay > 0 || s.candidates.length > 0) as slot (slot.key)}
                {@const best = slot.candidates.find((c) => c.model === slot.best)}
                <tr class="h-11 border-b border-line-subtle last:border-0">
                  <th scope="row" class="px-5 font-sans text-[13px] font-normal text-fg">{slot.label}</th>
                  <td class={cn(num, 'py-0 text-muted')}>{perDay(slot.perDay)}</td>
                  <td class="px-3">
                    {#if best}
                      <button
                        type="button"
                        onclick={() => onSelect(best.model)}
                        title={best.model}
                        class={cn('rounded text-fg underline-offset-2 hover:underline', modelName(best.model) ? 'font-sans text-[13px]' : 'font-mono text-xs', focus)}
                      >
                        {modelName(best.model) ?? best.model}
                      </button>
                    {:else}
                      <span class="font-sans text-[13px] text-muted" title="Nothing tested reaches Ready here. That is a finding, not a gap. Test another model, or read the reason in the matrix.">
                        Nothing tested is ready
                      </span>
                    {/if}
                  </td>
                  <td class={cn(num, 'py-0 text-fg')}>{best ? usdRate(best.usdPerDay, 'day') : '—'}</td>
                  <td class="pl-3 pr-5">
                    <span class="flex flex-wrap gap-1">
                      {#each slot.candidates.filter((c) => c.model !== slot.best).slice(0, 3) as c (c.model)}
                        <Chip
                          tone={BAND_META[c.band].tone}
                          title="{c.model}: {BAND_META[c.band].label}. This slot would cost {usdRate(c.usdPerDay, 'day')} on it."
                        >
                          {shortName(c.model)}
                        </Chip>
                      {/each}
                      {#if slot.candidates.length === 0}
                        <span class="font-sans text-[13px] text-muted">—</span>
                      {/if}
                    </span>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>
    {/if}
  </div>
{/if}
