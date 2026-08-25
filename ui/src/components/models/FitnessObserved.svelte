<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { cn } from '@/lib/cn'
  import { pct, type Divergence, type HarnessScore, type ObservedHarness, type ObservedModel, type Thresholds } from './fitness'

  // TESTED vs OBSERVED, side by side. The bench says what a model did on
  // fixtures; production says what it is doing on real traffic. A model that
  // benched Ready and is running at a 12% repair rate is the alert that
  // matters, and it is the one thing no external benchmark can ever give you.
  //
  // The two halves are computed by the same definitions on purpose (see the
  // header of server/fitness/observed.ts): if they diverge it must be because
  // the model changed, not because the ruler did.
  let {
    tested,
    observed,
    observedModel,
    divergences,
    thresholds,
    harnessLabels = {},
  }: {
    tested: HarnessScore[]
    observed: ObservedHarness[]
    observedModel: ObservedModel | null
    divergences: Divergence[]
    thresholds: Thresholds
    harnessLabels?: Record<string, string>
  } = $props()

  // Every harness either half knows about — a harness production runs and the
  // bench never covered is exactly the row an admin needs to see.
  const rows = $derived(
    [...new Set([...tested.map((t) => t.id), ...observed.map((o) => o.harness)])].sort((a, b) => a.localeCompare(b)),
  )
  // A HARNESS THE SWEEP SKIPPED HAS NO BENCH NUMBERS, and `cases: 0` is how it
  // says so — every rate on it is the `n === 0` zero, not a measurement. It is
  // kept out of `testedOf` so the three bench columns render a dash and the row
  // takes the "unbenched" chip, which is exactly what happened: nothing was
  // benched. Printing 0% for a model the sweep never called is the defect this
  // whole pass is about, and the table was its loudest surface.
  const testedOf = $derived(new Map(tested.filter((t) => t.cases > 0).map((t) => [t.id, t])))
  const labelOf = $derived(new Map(tested.map((t) => [t.id, t.label])))
  const skipOf = $derived(new Map(tested.filter((t) => t.skipReason).map((t) => [t.id, t.skipReason as string])))
  const liveOf = $derived(new Map(observed.map((o) => [o.harness, o])))
  const label = (id: string): string => harnessLabels[id] ?? labelOf.get(id) ?? id
  const dash = '—'
</script>

{#if divergences.length > 0}
  <ul class="mb-3 space-y-1">
    {#each divergences as d (`${d.harness}:${d.metric}`)}
      <li class={cn('max-w-prose font-sans text-xs', d.worse ? 'text-warning' : 'text-muted')}>{d.note}</li>
    {/each}
  </ul>
{/if}

{#if rows.length === 0}
  <EmptyState
    variant="compact"
    icon="◇"
    title="No production traffic yet"
    hint="Harness runs from the last {thresholds.observedWindowDays} days appear here next to the benched numbers. An install that has not run these harnesses yet shows nothing, which is correct."
  />
{:else}
  <div class="overflow-x-auto">
    <table class="w-full border-collapse text-left">
      <thead>
        <tr class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <th class="py-1.5 pr-3 font-normal">Harness</th>
          <th class="px-2 py-1.5 text-right font-normal" title="Contract held on the first attempt, on the fixtures.">Bench 1st</th>
          <th class="px-2 py-1.5 text-right font-normal" title="Contract held at all, including after a repair turn.">Bench held</th>
          <th class="px-2 py-1.5 text-right font-normal" title="Guard findings per fixture run.">Bench guard</th>
          <th class="border-l border-line px-2 py-1.5 text-right font-normal">Live 1st</th>
          <th class="px-2 py-1.5 text-right font-normal" title="Share of production runs a repair turn had to rescue.">Live repaired</th>
          <th class="px-2 py-1.5 text-right font-normal">Live guard</th>
          <th class="px-2 py-1.5 text-right font-normal" title="Runs in the window. Below {thresholds.minObservedRuns} runs a gap is sampling noise and is not reported as a divergence.">Runs</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as id (id)}
          {@const t = testedOf.get(id)}
          {@const o = liveOf.get(id)}
          <tr class="border-b border-line-subtle last:border-0">
            <td class="py-1.5 pr-3 font-mono text-[11px] text-fg">
              {label(id)}
              {#if !t}<Chip
                  class="ml-1.5"
                  title={skipOf.get(id) ??
                    'Production runs this harness and the last bench did not cover it; either it declares no fixtures, or the sweep did not reach it.'}
                >{skipOf.has(id) ? 'not testable here' : 'unbenched'}</Chip
                >{/if}
            </td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{t ? pct(t.contractRate) : dash}</td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{t ? pct(t.repairRate) : dash}</td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{t ? t.guardRate.toFixed(2) : dash}</td>
            <td class="border-l border-line px-2 py-1.5 text-right font-mono text-[11px] text-muted">{o ? pct(o.contractRate) : dash}</td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{o ? pct(o.repairedShare) : dash}</td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{o ? o.findingsPerRun.toFixed(2) : dash}</td>
            <td class="px-2 py-1.5 text-right font-mono text-[11px] text-muted">{o ? o.runs : dash}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  {#if observedModel}
    <!-- The two findings figures are kept apart deliberately: one has a run
         denominator and one does not, and adding them would produce a rate
         with no population. -->
    <p class="mt-2 max-w-prose font-sans text-xs text-muted">
      Across all harnesses in the last {thresholds.observedWindowDays} days this model ran {observedModel.harnessRuns} time(s) at
      {observedModel.harnessFindingsPerRun.toFixed(2)} findings per run, and the guard filed {observedModel.guardFindings} finding(s) against it
      overall; {observedModel.confabulation} of which were the model inventing something rather than repeating something it should not have.
    </p>
  {/if}
{/if}
