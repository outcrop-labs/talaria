<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import { Square } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { fly } from '@/lib/motion'
  import FitnessDetail from '@/components/models/FitnessDetail.svelte'
  import FitnessMatrix from '@/components/models/FitnessMatrix.svelte'
  import FitnessRunModal from '@/components/models/FitnessRunModal.svelte'
  import FitnessHealth from '@/components/models/FitnessHealth.svelte'
  import FitnessValue from '@/components/models/FitnessValue.svelte'
  import { TIER_META, usd, valueVersion, worthRetrying } from '@/components/models/fitness'
  import { useFitnessDetail, useFitnessHealth, useFitnessValue, useModelFitness } from '@/components/models/fitness-queries'

  // ── Model fitness — "can I swap this model in", per role, from the UI ───────
  //
  // The whole point of the harness layer: Talaria has to work on a 7-14B
  // self-host and excel on a frontier model, and an admin has to be able to
  // tell WHICH, per role, without a week of production surprises. The matrix is
  // the answer at a glance; the run fills it; the drill-down is what makes it
  // trustworthy.
  const qc = useQueryClient()
  const query = useModelFitness()
  const data = $derived(query.data)

  // The selected model lives in the URL (UI-CONVENTIONS: any selection worth
  // sharing or revisiting is a search param, not component state) so an admin
  // can send a colleague the exact verdict they are arguing about.
  // sv-router auto-parses query values (numbers, bare flags). A model id must
  // stay a string — `gpt-4` is fine but a bare numeric id would arrive as a
  // number and never match a row.
  const selected = $derived.by((): string | null => {
    const v = searchParams.get('model')
    return v == null || v === true ? null : String(v)
  })
  const select = (model: string) => searchParams.set('model', model)
  // The report's open compartment rides the URL too (`?rep=`), so closing the
  // dialog has to take it with it — a stale `rep` left behind would decide which
  // tab the NEXT model opens on, which is not a choice anyone made.
  const closeReport = () => {
    searchParams.delete('rep')
    searchParams.delete('model')
  }
  const detailQuery = useFitnessDetail(() => selected)

  // ── The two views of one run ───────────────────────────────────────────────
  //
  // TABS RATHER THAN TWO STACKED PANELS, because they answer the same question
  // in two units and an admin reads one at a time: the matrix says WHETHER a
  // model holds a slot, cost says WHAT THAT COSTS. Stacked, the second sat
  // below a 21-column table nobody scrolls past, and the run strip — the thing
  // that fills BOTH — belonged to neither.
  //
  // The strip therefore stays above the tabs, and the sub-tab rides the URL
  // (`?fit=cost`) for the same reason the model selection does: a verdict worth
  // arguing about is worth linking to.
  //
  // THE THIRD TAB IS ABOUT US, NOT ABOUT A MODEL. The matrix says which model
  // holds a slot and cost says what that costs; neither can say whether the
  // FIXTURE is right, and a red cell is equally consistent with both readings.
  // It sits beside them rather than inside a model's report because the evidence
  // for it is cross-model by construction — one run can never show it.
  type FitTab = 'matrix' | 'cost' | 'health'
  const FIT_TABS: ReadonlyArray<TabItem<FitTab>> = [
    { id: 'matrix', label: 'Fitness matrix' },
    { id: 'cost', label: 'Cost & value' },
    { id: 'health', label: 'Harness health' },
  ]
  const view = $derived.by((): FitTab => {
    const v = searchParams.get('fit')
    return v === 'cost' || v === 'health' ? v : 'matrix'
  })
  const setView = (v: FitTab) => (v === 'matrix' ? searchParams.delete('fit') : searchParams.set('fit', v))

  // Fetched only once the tab is opened — it costs a telemetry query plus a
  // price lookup per tested model, and an admin who never opens it should never
  // pay for it — and refetched whenever a run ARCHIVES. The matrix already polls
  // while a sweep is in flight, so its payload carries the new archive within
  // three seconds of a run landing; keying off it is what makes the cost tab
  // update itself instead of showing pre-run numbers until something else
  // happened to invalidate it.
  const valueQuery = useFitnessValue(
    () => view === 'cost',
    () => valueVersion(data?.index ?? {}),
  )
  const healthQuery = useFitnessHealth(
    () => view === 'health',
    () => valueVersion(data?.index ?? {}),
  )

  let running = $state(false)
  let candidate = $state('')
  // UP TO THREE AT ONCE. Comparing a shortlist is the job, and one-at-a-time
  // turned a fifteen-minute sweep into an afternoon. `full` comes off the wire
  // rather than being recounted here — the cap is the server's rule.
  const live = $derived((data?.runs ?? []).filter((r) => r.state === 'running'))
  const lastFinished = $derived((data?.runs ?? []).find((r) => r.state !== 'running'))
  const full = $derived(data?.full ?? false)
  const row = $derived(data?.models.find((m) => m.id === selected))
  /** Cases in the archived report that left a hole. Counted with the SAME
   *  predicate the sweep uses (`worthRetrying`), so the button's number and the
   *  set it re-runs cannot disagree. */
  const retryable = $derived((detailQuery.data?.record?.cases ?? []).filter(worthRetrying).length)
  /** Fixtures the registry declares that this model's archive has no verdict on.
   *  `registry.fixtures` is the live count and the record holds what was asked,
   *  so the difference is what a supplemental pass would run. */
  const unanswered = $derived(
    detailQuery.data?.record ? Math.max(0, (data?.registry.fixtures ?? 0) - detailQuery.data.record.cases.length) : 0,
  )

  const openRun = (model: string) => {
    candidate = model
    running = true
  }
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['model-fitness'] })
    await qc.invalidateQueries({ queryKey: ['model-capabilities'] })
    await qc.invalidateQueries({ queryKey: ['model-fitness-detail'] })
    await qc.invalidateQueries({ queryKey: ['model-fitness-value'] })
  }
  const post = async (body: unknown) => {
    await fetch('/api/admin/model-fitness', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    await refresh()
  }
  /** CLEAR IS NOT FORGET, and the two dialogs say so. Clear drops what a RUN
   *  FOUND so the model can be swept again from nothing; Forget drops what we
   *  know it CAN DO, which is nine probe calls somebody already paid for. An
   *  admin who has just fixed a fixture wants the first and emphatically not the
   *  second. */
  const clearResults = async (model: string | null) => {
    const ok = await confirm({
      title: model ? `Clear recorded results for ${model}` : 'Clear every recorded result',
      message: model
        ? `The archived report, its place in the matrix, the resume ledger and the stored transcripts for ${model} are deleted, so the next run starts from nothing. Measured capabilities are KEPT — this is not Forget.`
        : 'Every tested model loses its archived report, its matrix row, its resume ledger and its stored transcripts. Measured capabilities are kept. Use this when a fixture change makes the recorded numbers meaningless.',
      confirmLabel: 'Clear results',
      danger: true,
    })
    if (ok) await post({ action: 'clear', model })
  }

  const forget = async (model: string) => {
    // Audit 1.2's release valve. Destructive enough to confirm: it drops probe
    // results an admin paid for, not just the gateway's learned guesses.
    const ok = await confirm({
      title: 'Forget what we know about this model',
      message: `Every recorded capability for ${model} is deleted — probe results, what the gateway learned from a rejected parameter, and anything declared. Talaria keeps running it; it just stops claiming to know what it can do. Use this when a model id has been re-pointed at different weights.`,
      confirmLabel: 'Forget',
      danger: true,
    })
    if (ok) await post({ action: 'forget', model })
  }
</script>

{#if !data}
  <Panel>
    {#if query.isError}
      <QueryError
        variant="compact"
        error={query.error}
        title="Could not load model fitness"
        onRetry={() => void query.refetch()}
      />
    {:else}
      <Skeleton class="mb-4 h-4 w-32 rounded-full" />
      <SkeletonRows rows={5} />
    {/if}
  </Panel>
{:else}
  <div class="space-y-4">
    <Panel>
      <SectionHeader
        title="Model fitness"
        info="One run fills both views. The matrix says whether a model holds a slot; cost says what holding it would cost you, weighed on the work your fleet actually does. Grey means nothing has measured it — which is not a pass."
        action={`${data.registry.harnesses} harnesses · ${data.registry.fixtures} fixtures · ${data.registry.provocations} provocations`}
      />

      <!-- The run strip: the ReindexStatus grammar (retrieval), because
           Talaria has one long-run mechanism and this follows it — one ROW per
           run, because up to three of them are the point. Stop is per run: an
           admin comparing three candidates who sees one is hopeless should be
           able to drop that one without losing the other two. -->
      <div class="mb-4 space-y-2 rounded-md border border-line p-3">
        {#each live as run (run.model)}
          <div class="flex flex-wrap items-center gap-3">
            <!-- THE RUNNING ROW IS A DOOR. Watching a sweep is exactly when an
                 admin wants to open it — the live console, the failures as they
                 land — and until this it was the one place on the page that
                 named a model and could not be clicked to see it. -->
            <button
              type="button"
              onclick={() => run.model && select(run.model)}
              title="Open {run.model} — the live console, and failures as they land"
              class={cn(
                'group/run flex items-center gap-1.5 rounded px-1 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg',
                focusGold,
              )}
            >
              <GeneratingBars bars={3} variant="breathe" step={0.2} />
              <span class="underline decoration-dotted decoration-line underline-offset-2 group-hover/run:decoration-accent">{run.model}</span>
              · {run.phase === 'scoring' ? 'scoring' : run.phase ? TIER_META[run.phase].label : ''}
              {#if run.total > 0}
                · {run.done}/{run.total} fixtures{run.harness ? ` · ${run.harness}` : ''}
              {/if}
              <span class="text-ink-dim opacity-0 transition-opacity group-hover/run:opacity-100">open →</span>
            </button>
            <span class="ml-auto"></span>
            <!-- Stop is honored at a case boundary and the sweep RESUMES: an
                 admin who stops a run does not lose the calls already paid for. -->
            <!-- The icon, not the word: this strip carries one row per running
                 candidate and up to eight of them, so the label was repeated
                 down the panel where the model id is the thing being read.
                 `aria-label` and the tooltip keep it nameable. -->
            <Button
              size="sm"
              variant="outline"
              aria-label="Stop testing {run.model}"
              title="Stop testing {run.model} — honored at a case boundary; the sweep resumes where it stopped."
              onclick={() => void post({ action: 'stop', model: run.model })}
            >
              <Square size={12} fill="currentColor" />
            </Button>
          </div>
        {/each}

        <div class="flex flex-wrap items-center gap-3">
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            {#if live.length > 0}
              {live.length} of {data.max} running
            {:else if lastFinished?.state === 'error'}
              <span class="text-danger">last run failed: {lastFinished.error}</span>
            {:else if lastFinished?.state === 'done' && lastFinished.model}
              last run: {lastFinished.model} · {lastFinished.tiers.join(' + ')}
            {:else}
              no run yet
            {/if}
          </span>
          <span class="ml-auto"></span>
          <!-- Clearing EVERY result is a panel-level action, not a per-model
               one: the reason to reach for it is a fixture change that makes the
               whole matrix meaningless, not one bad run. -->
          {#if Object.keys(data.index).length > 0}
            <DangerLink onClick={() => void clearResults(null)}>Clear all results</DangerLink>
          {/if}
          <Button
            size="sm"
            onclick={() => openRun(selected ?? data.models[0]?.id ?? '')}
            disabled={data.models.length === 0 || full}
            title={full ? `Already testing ${data.max} models — stop one, or wait for it to finish.` : undefined}
          >
            Test a model
          </Button>
        </div>
      </div>

      {#if data.models.length === 0}
        <EmptyState icon="▤" title="No models registered yet" hint="Add a provider on the Models tab, then come back and test one." />
      {:else}
        <!-- Both views read one run, so the strip above them is shared and the
             tabs sit between it and the panes. -->
        <Tabs class="mb-3" items={FIT_TABS} value={view} onChange={setView} />

        {#key view}
          <div in:fly={{ y: 6, duration: 200 }}>
            {#if view === 'matrix'}
              <FitnessMatrix slots={data.slots} models={data.models} index={data.index} {selected} onSelect={select} />
              {#if data.registry.unfixtured.length > 0}
                <p class="mt-2 max-w-prose font-sans text-xs text-muted">
                  {data.registry.unfixtured.length} registered harness(es) declare no eval fixtures, so no run can say anything about them.
                  They are invisible to this matrix rather than passing it.
                </p>
              {/if}
            {:else if view === 'health'}
              {#if healthQuery.isPending}
                <SkeletonRows rows={4} />
              {:else if !healthQuery.data}
                <QueryError
                  variant="compact"
                  error={healthQuery.error}
                  title="Could not read harness health"
                  onRetry={() => void healthQuery.refetch()}
                />
              {:else}
                <FitnessHealth data={healthQuery.data} />
              {/if}
            {:else if valueQuery.isPending}
              <SkeletonRows rows={4} />
            {:else if !valueQuery.data}
              <QueryError
                variant="compact"
                error={valueQuery.error}
                title="Could not work out price against performance"
                onRetry={() => void valueQuery.refetch()}
              />
            {:else}
              <FitnessValue data={valueQuery.data} {selected} onSelect={select} />
            {/if}
          </div>
        {/key}
      {/if}
    </Panel>
  </div>

  <!-- THE REPORT IS A DIALOG, not a panel under the matrix.
       It used to expand below, which put a full report — probes, per-slot
       verdicts, the tested-vs-observed table, the adversarial breakdown — at the
       bottom of a page whose top is a 21-column table. Clicking a cell scrolled
       nothing, so the answer to "what did this model do" appeared off-screen.
       A takeover modal puts it where the click was and gives it the height it
       needs; the selection stays in the URL either way, so a link to a specific
       model still opens straight onto its report. -->
  {#if selected}
    <!-- `padded={false}`: the report owns its own frame now. Its identity line,
         verdict tally, live strip and compartment tabs are PINNED and only the
         open compartment scrolls, which a padded body scrolling as one column
         cannot express. -->
    <Modal open={true} onClose={() => closeReport()} takeover padded={false}>
      {#snippet title()}
        <span class="flex flex-wrap items-center gap-2">
          <Chip tone="accent">{selected}</Chip>
          {#if data.index[selected]}
            <span class="font-mono text-[11px] normal-case tracking-normal text-muted">
              {data.index[selected]?.calls} call(s) · {usd(data.index[selected]?.costUsd ?? null)}
              {#if data.index[selected]?.partial} · partial run{/if}
            </span>
          {:else}
            <span class="font-mono text-[11px] normal-case tracking-normal text-muted">never tested</span>
          {/if}
        </span>
      {/snippet}

      {#if detailQuery.isPending}
        <div class="p-7"><SkeletonRows rows={6} /></div>
      {:else if !detailQuery.data}
        <div class="p-7">
          <QueryError
            variant="compact"
            error={detailQuery.error}
            title="Could not load this model's report"
            onRetry={() => void detailQuery.refetch()}
          />
        </div>
      {:else}
        <FitnessDetail detail={detailQuery.data} {row} />
      {/if}

      {#snippet footer()}
        <!-- Forget sits beside Test, not at the bottom of a scroll: it is the
             destructive twin of the button next to it and an admin should see
             both without hunting. -->
        <div class="flex flex-wrap items-center gap-2">
          <DangerLink onClick={() => void clearResults(selected)}>Clear results</DangerLink>
          <DangerLink onClick={() => void forget(selected)}>Forget recorded capabilities</DangerLink>
          <span class="ml-auto"></span>
          <!-- THE CHEAP RETRY. A sweep that timed out on five cases because the
               provider was busy does not need the other two hundred and forty-two
               bought again — and until this button existed the only options were
               resume (which has nothing pending, every case is recorded) and
               restart (which re-buys the lot). -->
          <!-- FIXTURES THIS MODEL HAS NEVER BEEN ASKED. The registry gains
               fixtures continuously; a model tested before them has no verdict
               on any, and neither Resume (nothing pending) nor Re-run failures
               (they never failed — they never ran) reaches them. -->
          {#if unanswered > 0}
            <Button
              size="sm"
              variant="outline"
              disabled={full || live.some((r) => r.model === selected)}
              title="Run only the fixtures this model has never been asked, and drop any verdict for a fixture that no longer exists. Everything already answered is kept."
              onclick={() => void post({ action: 'start', model: selected, tiers: ['evals'], supplement: true })}
            >
              Run {unanswered} new fixture{unanswered === 1 ? '' : 's'}
            </Button>
          {/if}
          <!-- RE-RUN TIER 3 ALONE. It has its own corpus that grows on its own
               schedule, and until the record merged rather than replaced, doing
               this wiped every tier-2 verdict on the model. -->
          {#if detailQuery.data?.record}
            <Button
              size="sm"
              variant="outline"
              disabled={full || live.some((r) => r.model === selected)}
              title="Re-run the safety provocations only. Everything else on this model's report is kept."
              onclick={() => void post({ action: 'start', model: selected, tiers: ['adversarial'] })}
            >
              Re-run adversarial
            </Button>
          {/if}
          {#if retryable > 0}
            <Button
              size="sm"
              variant="outline"
              disabled={full || live.some((r) => r.model === selected)}
              title="Re-run only the cases that failed, timed out, or could not be measured. The passing ones are kept."
              onclick={() => void post({ action: 'start', model: selected, tiers: ['evals'], retryFailed: true })}
            >
              Re-run {retryable} failure{retryable === 1 ? '' : 's'}
            </Button>
          {/if}
          <Button
            size="sm"
            variant="outline"
            onclick={() => openRun(selected)}
            disabled={full || live.some((r) => r.model === selected)}
            title={live.some((r) => r.model === selected)
              ? 'This model is being tested right now.'
              : full
                ? `Already testing ${data.max} models — stop one, or wait for it to finish.`
                : undefined}
          >
            Test this model
          </Button>
          <Button size="sm" variant="ghost" onclick={() => closeReport()}>Close</Button>
        </div>
      {/snippet}
    </Modal>
  {/if}

  {#if running}
    <FitnessRunModal
      open={running}
      models={data.models}
      bind:model={candidate}
      onClose={() => (running = false)}
      onStarted={() => {
        running = false
        void refresh()
      }}
    />
  {/if}
{/if}
