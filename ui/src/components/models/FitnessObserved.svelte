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
  // header of api/src/fitness/observed.rs): if they diverge it must be because
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

  const num = 'px-3 text-right font-sans text-[13px] tabular-nums text-muted'
</script>

{#if divergences.length > 0}
  <ul class="mb-4 space-y-1">
    {#each divergences as d (`${d.harness}:${d.metric}`)}
      <li class={cn('max-w-[72ch] font-sans text-[13px] leading-5', d.worse ? 'text-warning' : 'text-muted')}>{d.note}</li>
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
  <div class="overflow-x-auto rounded-lg border border-line bg-panel">
    <table class="w-full border-collapse text-left">
      <thead>
        <tr class="font-sans text-xs leading-4 text-muted">
          <th class="px-5 pt-2.5 font-medium"></th>
          <th colspan="3" class="px-3 pt-2.5 text-center font-medium">Tested</th>
          <th colspan="4" class="border-l border-line-subtle pl-3 pr-5 pt-2.5 text-center font-medium">In production</th>
        </tr>
        <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
          <th class="px-5 py-2.5 font-medium">Harness</th>
          <th class="px-3 py-2.5 text-right font-medium" title="Contract held on the first attempt, on the fixtures.">First try</th>
          <th class="px-3 py-2.5 text-right font-medium" title="Contract held at all, including after a repair turn.">Held</th>
          <th class="px-3 py-2.5 text-right font-medium" title="Guard findings per fixture run.">Guard</th>
          <th class="border-l border-line-subtle px-3 py-2.5 text-right font-medium">First try</th>
          <th class="px-3 py-2.5 text-right font-medium" title="Share of production runs a repair turn had to rescue.">Repaired</th>
          <th class="px-3 py-2.5 text-right font-medium">Guard</th>
          <th class="py-2.5 pl-3 pr-5 text-right font-medium" title="Runs in the window. Below {thresholds.minObservedRuns} runs a gap is sampling noise and is not reported as a divergence.">Runs</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as id (id)}
          {@const t = testedOf.get(id)}
          {@const o = liveOf.get(id)}
          <tr class="h-11 border-b border-line-subtle last:border-0">
            <td class="px-5 font-sans text-[13px] text-fg">
              {label(id)}
              {#if !t}<Chip
                  class="ml-2"
                  title={skipOf.get(id) ??
                    'Production runs this harness and the last bench did not cover it; either it declares no fixtures, or the sweep did not reach it.'}
                >{skipOf.has(id) ? 'Not testable here' : 'Not tested'}</Chip
                >{/if}
            </td>
            <td class={num}>{t ? pct(t.contractRate) : dash}</td>
            <td class={num}>{t ? pct(t.repairRate) : dash}</td>
            <td class={num}>{t ? t.guardRate.toFixed(2) : dash}</td>
            <td class={cn(num, 'border-l border-line-subtle')}>{o ? pct(o.contractRate) : dash}</td>
            <td class={num}>{o ? pct(o.repairedShare) : dash}</td>
            <td class={num}>{o ? o.findingsPerRun.toFixed(2) : dash}</td>
            <td class={cn(num, 'pr-5')}>{o ? o.runs : dash}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  {#if observedModel}
    <!-- The two findings figures are kept apart deliberately: one has a run
         denominator and one does not, and adding them would produce a rate
         with no population. -->
    <p class="mt-3 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
      Across all harnesses in the last {thresholds.observedWindowDays} days this model ran {observedModel.harnessRuns} time{observedModel.harnessRuns === 1 ? '' : 's'} at
      {observedModel.harnessFindingsPerRun.toFixed(2)} findings per run. The guard filed {observedModel.guardFindings} finding{observedModel.guardFindings === 1 ? '' : 's'} against it,
      {observedModel.confabulation} of them the model inventing something rather than repeating something it should not have.
    </p>
  {/if}
{/if}
