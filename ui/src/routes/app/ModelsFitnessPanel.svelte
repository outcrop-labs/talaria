<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import { postJson } from '@/lib/fetch-json'
  import { fly } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import FitnessDetail from '@/components/models/FitnessDetail.svelte'
  import FitnessLegend from '@/components/models/FitnessLegend.svelte'
  import FitnessMatrix from '@/components/models/FitnessMatrix.svelte'
  import FitnessRunModal from '@/components/models/FitnessRunModal.svelte'
  import FitnessHealth from '@/components/models/FitnessHealth.svelte'
  import FitnessValue from '@/components/models/FitnessValue.svelte'
  import FitnessAppModels from '@/components/models/FitnessAppModels.svelte'
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
  // A SWITCH RATHER THAN TWO STACKED PANELS, because they answer the same question
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
  const FIT_TABS: ReadonlyArray<SegmentedOption<FitTab>> = [
    { id: 'matrix', label: 'Matrix' },
    { id: 'cost', label: 'Cost and value' },
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
    try {
      await postJson<{ ok: true }>('/api/admin/model-fitness', body)
    } catch (e) {
      toastError('Fitness action failed', e)
      return
    }
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
        ? `The archived report, its place in the matrix, the resume ledger and the stored transcripts for ${model} are deleted, so the next run starts from nothing. Measured capabilities are KEPT; this is not Forget.`
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
      message: `Every recorded capability for ${model} is deleted: probe results, what the gateway learned from a rejected parameter, and anything declared. Talaria keeps running it; it just stops claiming to know what it can do. Use this when a model id has been re-pointed at different weights.`,
      confirmLabel: 'Forget',
      danger: true,
    })
    if (ok) await post({ action: 'forget', model })
  }

  const selectedBusy = $derived(full || live.some((r) => r.model === selected))
  // Every way to run this model again short of a full test, then the two that
  // delete. Each stays one click from the report without a row of buttons.
  const reportActions = (model: string): ContextMenuEntry[] => {
    const runs: ContextMenuEntry[] = []
    // FIXTURES THIS MODEL HAS NEVER BEEN ASKED. The registry gains fixtures
    // continuously; a model tested before them has no verdict on any, and
    // neither a resume nor a failure re-run reaches them.
    if (unanswered > 0)
      runs.push({
        label: `Run ${unanswered} new fixture${unanswered === 1 ? '' : 's'}`,
        title: 'Runs only the fixtures this model has never been asked, and drops any verdict for a fixture that no longer exists. Everything already answered is kept.',
        disabled: selectedBusy,
        onSelect: () => void post({ action: 'start', model, tiers: ['evals'], supplement: true }),
      })
    // The safety tier has its own corpus that grows on its own schedule.
    if (detailQuery.data?.record)
      runs.push({
        label: 'Re-run safety tests only',
        title: "Runs the safety tests again. Everything else on this model's report is kept.",
        disabled: selectedBusy,
        onSelect: () => void post({ action: 'start', model, tiers: ['adversarial'] }),
      })
    // THE CHEAP RETRY. A sweep that timed out on five cases because the
    // provider was busy does not need the passing ones bought again.
    if (retryable > 0)
      runs.push({
        label: `Re-run ${retryable} failed case${retryable === 1 ? '' : 's'}`,
        title: 'Runs only the cases that failed, timed out or could not be measured. The passing ones are kept.',
        disabled: selectedBusy,
        onSelect: () => void post({ action: 'start', model, tiers: ['evals'], retryFailed: true }),
      })
    return [
      ...runs,
      ...(runs.length > 0 ? (['sep'] as const) : []),
      { label: 'Clear results', danger: true, onSelect: () => void clearResults(model) },
      { label: 'Forget recorded capabilities', danger: true, onSelect: () => void forget(model) },
    ]
  }
</script>

{#if !data}
  <SettingsGroup>
    <div class="px-5 py-4">
      {#if query.isError}
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load model fitness"
          onRetry={() => void query.refetch()}
        />
      {:else}
        <SkeletonRows rows={5} />
      {/if}
    </div>
  </SettingsGroup>
{:else}
  <div class="space-y-10">
    <SettingsSection
      title="Model fitness"
      description="Which models are good at which jobs, measured by test runs."
      info={`One run fills every view here. The matrix says whether a model holds a job, and cost says what holding it would cost you on the work your agents actually do. Grey means nothing has measured it, which is not a pass. This install has ${data.registry.harnesses} harnesses, ${data.registry.fixtures} fixtures and ${data.registry.provocations} safety provocations.`}
    >
      {#snippet action()}
        <span class="font-sans text-[13px] leading-5 text-muted">
          {#if live.length > 0}
            {live.length} of {data.max} running
          {:else if lastFinished?.state === 'error'}
            <span class="text-danger">Last run failed: {lastFinished.error}</span>
          {:else if lastFinished?.state === 'done' && lastFinished.model}
            Last run: <span class="font-mono text-xs">{lastFinished.model}</span>
          {/if}
        </span>
        <!-- BEHIND A MENU, not beside the button. Clearing EVERY result is the
             answer to a fixture change that makes the whole matrix meaningless,
             and a destructive action of that scope should not sit one stray
             click from "Test a model". -->
        {#if Object.keys(data.index).length > 0}
          <SettingsRowMenu
            label="More fitness actions"
            items={[{ label: 'Clear all results', danger: true, onSelect: () => void clearResults(null) }]}
          />
        {/if}
        <Button
          size="sm"
          onclick={() => openRun(selected ?? data.models[0]?.id ?? '')}
          disabled={data.models.length === 0 || full}
          title={full ? `Already testing ${data.max} models. Stop one, or wait for it to finish.` : undefined}
        >
          Test a model
        </Button>
      {/snippet}

      <div class="space-y-4">
        {#if live.length > 0}
          <!-- One ROW per run, because up to three of them are the point. Stop
               is per run: an admin comparing candidates who sees one is
               hopeless can drop that one without losing the others. -->
          <SettingsGroup>
            {#each live as run (run.model)}
              <div class="flex min-h-[60px] items-center gap-6 px-5 py-3">
                <!-- THE RUNNING ROW IS A DOOR. Watching a sweep is exactly when
                     an admin wants to open it: the live console, and the
                     failures as they land. -->
                <button
                  type="button"
                  onclick={() => run.model && select(run.model)}
                  title="Open {run.model}: the live console, and failures as they land"
                  class="flex min-w-0 flex-1 items-center gap-3 rounded-md text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
                >
                  <WaitingMark site="models/fitness-panel" size={14} />
                  <span class="min-w-0">
                    <span class="block truncate font-mono text-[13px] leading-5 text-fg">{run.model}</span>
                    <span class="block font-sans text-[13px] leading-5 text-muted">
                      {run.phase === 'scoring' ? 'Scoring' : run.phase ? TIER_META[run.phase].label : 'Starting'}
                      {#if run.total > 0}
                        · {run.done} of {run.total} fixtures{run.harness ? ` · ${run.harness}` : ''}
                      {/if}
                    </span>
                  </span>
                </button>
                <!-- Stop is honored at a case boundary and the sweep RESUMES:
                     an admin who stops a run does not lose the calls already
                     paid for. -->
                <Button
                  size="sm"
                  variant="outline"
                  aria-label="Stop testing {run.model}"
                  title="Honored at a case boundary. The run resumes where it stopped."
                  onclick={() => void post({ action: 'stop', model: run.model })}
                >
                  Stop
                </Button>
              </div>
            {/each}
          </SettingsGroup>
        {/if}

        {#if (data.health?.ours ?? 0) > 0}
          <!-- THE BAND IS OURS, NOT THE MODELS'. `ours` counts fixtures every
               tested model got wrong — a bug in the fixture wearing a model's
               name. Stamped by the server when the archive changes, so a poll
               is one small row, not a re-fold. The line is a door to the names. -->
          <button
            type="button"
            onclick={() => setView('health')}
            class="block text-left font-sans text-[13px] leading-5 text-danger underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
          >
            {data.health?.ours} fixture{data.health?.ours === 1 ? '' : 's'} failed on every model tested, which points at the fixture and not the models. See harness health.
          </button>
        {/if}

        {#if data.models.length === 0}
          <EmptyState icon="▤" title="No models registered yet" hint="Add a provider on the Models tab, then come back and test one." />
        {:else}
          <!-- Every view reads one run, so the switch sits above all of them. -->
          <div class="flex flex-wrap items-center gap-x-6 gap-y-3">
            <Segmented options={FIT_TABS} value={view} onChange={setView} />
            {#if view === 'matrix'}<FitnessLegend class="ml-auto" />{/if}
          </div>

          {#key view}
            <div in:fly={{ y: 6, duration: 200 }}>
              {#if view === 'matrix'}
                <FitnessMatrix slots={data.slots} models={data.models} index={data.index} onSelect={select} />
                {#if data.registry.unfixtured.length > 0}
                  <p class="mt-3 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
                    {data.registry.unfixtured.length === 1
                      ? '1 registered harness has no test fixtures, so no run can say anything about it. It is missing from this matrix, not passing it.'
                      : `${data.registry.unfixtured.length} registered harnesses have no test fixtures, so no run can say anything about them. They are missing from this matrix, not passing it.`}
                  </p>
                {/if}
              {:else if view === 'health'}
                {#if healthQuery.isPending}
                  <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={4} /></div></SettingsGroup>
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
                <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={4} /></div></SettingsGroup>
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
      </div>
    </SettingsSection>

    <!-- The app columns' own model settings, under the columns that measure
         them. -->
    {#if view === 'matrix' && data.models.length > 0}
      <FitnessAppModels
        slots={data.slots.filter((s) => s.kind === 'app')}
        assigned={data.appModels ?? {}}
        models={data.models.map((m) => m.id)}
      />
    {/if}
  </div>

  <!-- THE REPORT IS A DIALOG, not a panel under the matrix. A takeover modal
       puts it where the click was and gives it the height it needs; the
       selection stays in the URL either way, so a link to a specific model
       still opens straight onto its report. -->
  {#if selected}
    <!-- `padded={false}`: the report owns its own frame. Its identity line,
         verdict tally, live strip and compartment tabs are PINNED and only the
         open compartment scrolls, which a padded body scrolling as one column
         cannot express. -->
    <Modal open={true} onClose={() => closeReport()} takeover padded={false}>
      {#snippet title()}
        <span class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <span class="font-mono text-sm font-medium text-fg">{selected}</span>
          <span class="font-sans text-[13px] font-normal leading-5 text-muted">
            {#if data.index[selected]}
              {data.index[selected]?.calls} calls · {usd(data.index[selected]?.costUsd ?? null)}{#if data.index[selected]?.partial} · partial run{/if}
            {:else}
              Never tested
            {/if}
          </span>
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
        <div class="flex items-center gap-2">
          <SettingsRowMenu label={`More actions for ${selected}`} items={() => reportActions(selected)} align="left" up />
          <span class="ml-auto"></span>
          <Button size="sm" variant="ghost" onclick={() => closeReport()}>Close</Button>
          <Button
            size="sm"
            onclick={() => openRun(selected)}
            disabled={selectedBusy}
            title={live.some((r) => r.model === selected)
              ? 'This model is being tested right now.'
              : full
                ? `Already testing ${data.max} models. Stop one, or wait for it to finish.`
                : undefined}
          >
            Test this model
          </Button>
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
