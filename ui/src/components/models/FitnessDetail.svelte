<script lang="ts">
  import { searchParams } from 'sv-router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import { cn } from '@/lib/cn'
  import { SquareTerminal } from '@lucide/svelte'
  import { fly } from '@/lib/motion'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import CapabilityTags from './CapabilityTags.svelte'
  import FitnessCases from './FitnessCases.svelte'
  import FitnessMark from './FitnessMark.svelte'
  import FitnessObserved from './FitnessObserved.svelte'
  import FitnessOverview from './FitnessOverview.svelte'
  import FitnessTerminal from './FitnessTerminal.svelte'
  import { BAND_META, BAND_SEVERITY, BAND_TEXT, SAFETY_META, TIER_META, pct, type DetailPayload, type ModelRow } from './fitness'

  // One model, in full: what the run established, where it broke, and what
  // production says about the same model on the same definitions.
  //
  // WHY THIS IS TABBED AND NOT STACKED. It used to be six panels in a column —
  // the live run, the model header, capabilities, per-slot verdicts, every
  // fixture, the adversarial breakdown, tested-vs-observed — inside a takeover
  // modal. That is four screens of scrolling in which the thing you opened the
  // dialog for is somewhere in the middle, and it got worse every time a tier
  // gained detail. Each of those sections answers a DIFFERENT question, asked by
  // a different person at a different moment ("can it hold Utility", "does it do
  // vision", "why is this cell red", "is it safe", "does the bench match
  // production"), so they are compartments, not chapters.
  //
  // THE FRAME DOES NOT SCROLL. Model, verdict summary and the live-run strip
  // stay pinned; only the open compartment moves. Which one you are looking at
  // rides the URL (`?rep=`), the same rule the model selection and the
  // matrix/cost tabs follow: a verdict worth arguing about is worth linking to.
  let { detail, row }: { detail: DetailPayload; row: ModelRow | undefined } = $props()

  const record = $derived(detail.record)
  const live = $derived(detail.live)
  /** The run's log, live or archived — see `DetailView.consoleLog`. */
  const consoleLines = $derived(live?.log ?? detail.consoleLog ?? [])
  const harnessLabels = $derived(Object.fromEntries((record?.harnesses ?? []).map((h) => [h.id, h.label])))

  // Worst first, assignable slots only. An admin opening a model wants the thing
  // that would bite them, not an alphabetical tour — and not five permanent "no
  // harness" rows above it. The archived report still carries a verdict for all
  // twenty slots (scoring is unchanged); the five nothing reaches are `unbound`
  // for every model forever, and the harnesses behind one of them — the
  // briefer's — are still listed under "no assignable slot" below. See
  // `slotViews` in the Rust fitness surface (api/src/fitness/surface.rs) for
  //  the two reasons a slot has no column.
  //
  // `BAND_SEVERITY` and `BAND_TEXT` come from `fitness.ts` rather than being
  // restated here: three copies of the band ordering and two of the colour table
  // is exactly how one panel comes to call a cell amber that another calls red.
  const slots = $derived(
    [...(record?.report.slots ?? [])].filter((s) => s.slot.live).sort((a, b) => BAND_SEVERITY[a.band] - BAND_SEVERITY[b.band]),
  )

  /** THE ONE-LINE VERDICT, in the frame, so no tab can hide it. Counted off the
   *  same slot list the Verdicts tab draws — a second tally is how two parts of
   *  one dialog come to disagree about the same run. */
  const tally = $derived.by(() => {
    const by = { ready: 0, workable: 0, unfit: 0, untested: 0, unbound: 0 }
    for (const s of slots) by[s.band]++
    // Worst first, and only what this run actually produced: a "0 Not a fit"
    // chip is noise, and a row of zeroes reads as a summary of nothing.
    return (['unfit', 'untested', 'workable', 'ready'] as const).map((band) => ({ band, n: by[band] })).filter((x) => x.n > 0)
  })

  /** Things true of the RUN rather than of the model, and every one of them
   *  changes how a number below should be read. They are in the frame for that
   *  reason: a partial sweep read as a full one is the single easiest way to
   *  come away from this dialog with the wrong conclusion. */
  const caveats = $derived.by(() => {
    const out: string[] = []
    if (!record) return out
    if (record.sweep.state === 'stopped' || (record.sweep.total > 0 && record.sweep.done < record.sweep.total)) {
      out.push(
        `This sweep covered ${record.sweep.done} of ${record.sweep.total} fixtures. Everything it did not reach is Untested, not passing. Start it again and it resumes where it stopped.`,
      )
    }
    if (!record.report.guarded) {
      out.push(
        'The guard was off for this run, so every guard rate below is zero because nothing was checked, not because nothing was found. No slot can be called Ready on that evidence.',
      )
    }
    // THE DEPLOYMENT COULD NOT REACH THE MODEL, said at the top rather than left
    // to be inferred from a column of identical case errors. This is a routing
    // or credential fact about the install and reads as a broken model unless it
    // is spelled out.
    if (record.sweep.error) {
      out.push(`${record.sweep.error} Nothing below was measured about the model itself. Fix the routing or the credential and run it again.`)
    }
    // THE SWEEP NARROWED ITSELF. A fact about the deployment, in the frame,
    // because the alternative is an admin reading the rate-limit errors it
    // caught on the way down as a verdict about the model.
    // THREE OUTCOMES, NOT TWO, since the valve learned to reopen. `ended` on its
    // own cannot tell them apart — a sweep that crawled at 1 for two hundred
    // cases and recovered on the last ten ends at 4 and looks untroubled — so
    // the sentences are written off `low`, the narrowest it ever ran.
    const c = record.sweep.concurrency
    if (c.narrowedBecause && c.low <= 1 && c.requested <= 1) {
      // Asked for one, still throttled: there was nothing to give up, which is
      // worth saying more loudly than a narrowing would have been.
      out.push(
        `The provider rate-limited this sweep even one fixture at a time ("${c.narrowedBecause}"). Cases were retried and any that never got an answer are marked unmeasured rather than failed. Nothing here is a verdict on the model until the deployment is quieter.`,
      )
    } else if (c.narrowedBecause && c.ended < c.requested) {
      out.push(
        `This sweep started ${c.requested} fixtures wide and was down to ${c.ended} when it finished, after the provider pushed back ("${c.narrowedBecause}"). That was this deployment's ceiling on ${new Date(record.at).toLocaleDateString()}, the day of this run, and not a property of this model. Run it again at ${c.ended} or lower for a clean read.`,
      )
    } else if (c.narrowedBecause) {
      // Narrowed and recovered. Said anyway, briefly: the timings in this run
      // include a stretch at a lower width, and an admin comparing them against
      // another model's needs to know that before trusting the difference.
      out.push(
        `The provider pushed back partway through ("${c.narrowedBecause}"), so this sweep dropped to ${c.low} fixtures in flight and climbed back to ${c.ended}. Nothing was scored against the model for it, but the timings cover both stretches.`,
      )
    }
    return out
  })

  const failingCases = $derived(
    (record?.cases ?? []).filter((c) => c.skipped === null && c.gap === null && (!c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null || c.findings > 0))
      .length,
  )
  const gapCases = $derived((record?.cases ?? []).filter((c) => c.gap !== null).length)

  type Pane = 'live' | 'overview' | 'verdicts' | 'capabilities' | 'fixtures' | 'adversarial' | 'observed'

  const panes = $derived.by(() => {
    const out: Array<TabItem<Pane>> = []
    if (live) out.push({ id: 'live', label: live.total > 0 ? `Live, ${live.done} of ${live.total}` : 'Live' })
    // OVERVIEW FIRST among the archived panes, because it is the answer and the
    // rest are the evidence. A run in flight still outranks it: what you opened
    // the dialog for during a sweep is the sweep.
    if (record) out.push({ id: 'overview', label: 'Overview' })
    if (record) out.push({ id: 'verdicts', label: 'Verdicts', count: slots.length })
    if (record?.probes) out.push({ id: 'capabilities', label: 'Capabilities', count: record.probes.results.length })
    if (record)
      out.push(
        failingCases > 0
          ? { id: 'fixtures', label: 'Failed fixtures', count: failingCases }
          : { id: 'fixtures', label: 'Fixtures', count: record.cases.length },
      )
    if (record?.adversarial) out.push({ id: 'adversarial', label: 'Safety' })
    out.push({ id: 'observed', label: 'Against production' })
    return out
  })

  // A run IN FLIGHT is what you opened this for, so it wins the default; after
  // that, the verdicts. `?rep=` overrides both, and a stale one (a tab that
  // stopped existing when the run finished) falls back rather than blanking.
  const wanted = $derived.by((): Pane | null => {
    const v = searchParams.get('rep')
    return typeof v === 'string' && panes.some((p) => p.id === v) ? (v as Pane) : null
  })
  const pane = $derived(wanted ?? panes[0]?.id ?? 'observed')
  const setPane = (p: Pane) => searchParams.set('rep', p)

  /** Whether the live console is expanded. Local rather than in the URL: it is a
   *  reading preference for the minute you are watching a run, not a selection
   *  worth linking to. */
  // CLOSED ON OPEN, ALWAYS — including while a run is in flight.
  //
  // It briefly auto-opened for a live run, on the theory that watching a sweep is
  // the one case where the console is what somebody came for. That theory is
  // wrong in the situation it fires in: anyone who tests models has runs going
  // most of the time, so "the exception" was every time they opened the dialog,
  // and the thing they had actually clicked in to read started a third of the way
  // down the pane. A button carrying its own line count is discoverable enough;
  // opening a panel nobody asked for is not a shortcut, it is a decision made on
  // somebody's behalf.
  let consoleOpen = $state(false)
</script>

<div class="flex h-full min-h-0 flex-col">
  <!-- ── The frame ─────────────────────────────────────────────────────────
       Identity, the verdict in one line, whatever is true of the run, and the
       compartment strip. None of it scrolls: an admin three screens into the
       fixtures still knows which model this is and that the sweep was partial. -->
  <div class="shrink-0 px-7 pt-5">
    <!-- The dialog's title names the model; this line is what is known of it. -->
    <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
      <CapabilityTags {row} />
      <span class="ml-auto font-sans text-[13px] leading-5 text-muted">
        {#if record}
          Tested {new Date(record.at).toLocaleString()} · {record.tiers.map((t) => TIER_META[t].label.toLowerCase()).join(', ')}
        {:else}
          Never tested
        {/if}
      </span>
    </div>

    {#if record}
      <div class="mt-2 flex flex-wrap items-center gap-1.5">
        {#each tally as t (t.band)}
          <Chip tone={BAND_META[t.band].tone} title={BAND_META[t.band].blurb}>{t.n} {BAND_META[t.band].label}</Chip>
        {/each}
        {#if gapCases > 0}
          <Chip
            tone="warn"
            title="Fixtures that could not fairly ask their question; the run was never given what the assertion demanded. A bug report about our harness, not a score about this model."
          >
            {gapCases} harness gap{gapCases === 1 ? '' : 's'}
          </Chip>
        {/if}
      </div>
    {/if}

    {#if live}
      <!-- THE RUN, WHILE IT IS RUNNING, pinned rather than paged: the sweep
           checkpoints every case as it lands, and an admin watching it should not
           have to be on the right tab to see it move. -->
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <WaitingMark site="models/fitness-detail" size={14} />
        <span class="font-sans text-[13px] leading-5 text-fg">
          {live.phase === 'scoring' ? 'Scoring' : live.phase ? TIER_META[live.phase].label : 'Running'}
          {#if live.total > 0}· {live.done} of {live.total} fixtures{/if}
          {#if live.harness}· <span class="font-mono text-xs text-muted">{live.harness}</span>{/if}
        </span>
        <span class="ml-auto font-sans text-xs leading-4 text-muted">Updates every 3 seconds</span>
      </div>
    {/if}

    {#each caveats as c (c)}
      <p class="mt-2 max-w-[72ch] font-sans text-[13px] leading-5 text-warning">{c}</p>
    {/each}

    <!-- THE CONSOLE IS NOT A PLACE, IT IS A THING YOU TURN ON. It was a tab,
         which made it somewhere to GO — and going there meant leaving the
         verdicts you were reading. A run's log is context for every other pane,
         so it toggles from the strip and opens above whichever one is showing.
         The count is on the button because a console with nothing in it is worth
         knowing about before you click. -->
    <div class="mt-3 flex items-end gap-4">
      <Tabs class="min-w-0 flex-1" items={panes} value={pane} onChange={setPane} />
      {#if consoleLines.length > 0}
        <button
          type="button"
          onclick={() => (consoleOpen = !consoleOpen)}
          aria-pressed={consoleOpen}
          title={consoleOpen ? 'Hide the run console' : `Show the run console (${consoleLines.length} lines)`}
          class={cn(
            'mb-1 inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md border px-2 font-sans text-xs tabular-nums transition-colors',
            'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
            consoleOpen ? 'border-line-strong bg-raised text-fg' : 'border-line text-muted hover:text-fg',
          )}
        >
          <SquareTerminal size={14} aria-hidden="true" />
          Console {consoleLines.length}
        </button>
      {/if}
    </div>
  </div>

  <!-- Above the pane rather than inside one: it is bounded, it clips its own
       content, and it belongs to the RUN rather than to whatever is open. -->
  {#if consoleOpen && consoleLines.length > 0}
    <div class="shrink-0 overflow-hidden border-b border-line bg-panel px-7 pb-3">
      <FitnessTerminal log={consoleLines} {live} bind:open={consoleOpen} />
    </div>
  {/if}

  <!-- ── The open compartment ──────────────────────────────────────────────
       One pane, its own scroll. Switching tabs returns you to the top of the
       next one rather than to wherever the last one left the page. -->
  {#key pane}
    <!-- THE PANE DOES NOT SCROLL; ITS CONTENT DECIDES.
         This used to be `overflow-y-auto`, which put the live console and the
         failures list in ONE scroll container — so the failures list's sticky
         category bar stuck to the top of that container and drew straight over
         the console, and the console's log was pushed below the entire list.
         Every pane but Live is a plain scrolling column; Live is a fixed console
         over a scrolling list, and only it knows that. -->
    <div class="min-h-0 flex-1 overflow-hidden" in:fly={{ y: 6, duration: 180 }}>
      {#if pane === 'live' && live}
        <!-- THE LIVE PANE IS THE RUN'S FAILURES. The console used to live in here
             too, which is why this comment used to be about two scroll
             containers fighting: it is a toggle on the tab strip now, pinned
             above whichever pane is open, so a log that is context for every
             pane is no longer trapped inside one of them. -->
        <div class="flex h-full min-h-0 flex-col gap-3 px-7 py-5">
          <div class="min-h-0 flex-1 overflow-hidden rounded-md bg-panel">
            <!-- THE FAILURE LIST IS A LIVE-RUN THING. After the run, the archived
                 Fixtures pane is the better read — it has every case, not the
                 bounded live window — so the console pane shows the console and
                 points there rather than duplicating it worse. -->
            {#if live && live.cases.length > 0}
              <FitnessCases cases={live.cases} dropped={live.dropped} {harnessLabels} fill />
            {:else}
              <EmptyState
                variant="compact"
                icon="◇"
                title="Nothing has failed yet"
                hint="Failures open here with the full transcript: what the model did, every tool call, and the assertion that judged it."
              />
            {/if}
          </div>
        </div>
      {:else}
      <div class="h-full overflow-y-auto px-7 py-5">
      {#if pane === 'overview'}
        <FitnessOverview {detail} onOpen={(p) => setPane(p as Pane)} />
      {:else if pane === 'verdicts'}
        {#if !record}
          <EmptyState
            variant="compact"
            icon="◇"
            title={live ? 'This is the first run for this model' : 'No run on record for this model'}
            hint={live
              ? 'The strip above is that run as it happens. A full report (per-slot verdicts, capabilities, the adversarial tier) is archived when it finishes.'
              : 'Run the probes to fill in what this model can do, and the harness tier to fill in the matrix.'}
          />
        {:else}
          <SectionHeader
            title="Verdict per job"
            info="One verdict per assignable slot. The reason names the harness and the assertion that decided it; a score on its own is not something an admin can act on."
          />
          <SettingsGroup>
            {#each slots as s (`${s.slot.kind}:${s.slot.id}`)}
              <SettingsRow label={s.slot.label}>
                {#snippet lead()}<span class="grid h-4 w-4 place-items-center"><FitnessMark band={s.band} /></span>{/snippet}
                {#snippet hint()}
                  {#each s.reasons as reason, i (i)}
                    <p class={cn('max-w-[72ch]', reason.band === 'ready' ? undefined : BAND_TEXT[reason.band])}>
                      {reason.detail}
                      {#if reason.assertion}<span class="text-muted"> ({reason.assertion})</span>{/if}
                    </p>
                  {/each}
                {/snippet}
                <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">
                  {#if s.contract}{s.contract.numerator} of {s.contract.denominator} first try{/if}
                  {#if s.task} · task floor {pct(s.taskFloor)}{/if}
                </span>
                <Chip tone={BAND_META[s.band].tone}>{BAND_META[s.band].label}</Chip>
              </SettingsRow>
            {/each}
          </SettingsGroup>
          {#if record.report.unbound.length > 0}
            <div class="mt-4">
              <!-- Harnesses whose model comes from the SUBJECT of the call: the
                   owner's assistant, the agent on the ticket, the researching
                   agent. Scored, because "can this model work a ticket" is the
                   question when picking an agent's brain — they simply have no
                   column an admin assigns. -->
              <Disclosure title="Harnesses with no assignable slot ({record.report.unbound.length})">
                <ul class="space-y-3 py-1">
                  {#each record.report.unbound as v (v.harness)}
                    <li class="font-sans text-[13px] leading-5">
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="grid h-4 w-3 place-items-center"><FitnessMark band={v.band} /></span>
                        <span class="font-medium text-fg">{v.label}</span>
                        <span class="text-muted">
                          {BAND_META[v.band].label}{#if v.cases > 0} · {pct(v.contractRate)} first try over {v.cases} fixture{v.cases === 1 ? '' : 's'}{/if}
                        </span>
                      </div>
                      {#if v.reasons[0]}<p class="ml-5 max-w-[72ch] text-muted">{v.reasons[0].detail}</p>{/if}
                    </li>
                  {/each}
                </ul>
              </Disclosure>
            </div>
          {/if}
          {#if record.sweep.unfixtured.length > 0}
            <p class="mt-3 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
              {record.sweep.unfixtured.length} registered harness{record.sweep.unfixtured.length === 1 ? ' has' : 'es have'} no fixtures, so this run says nothing about them:
              <span class="font-mono text-xs">{record.sweep.unfixtured.join(', ')}</span>.
            </p>
          {/if}
        {/if}
      {:else if pane === 'capabilities' && record?.probes}
        <SectionHeader
          title="Capabilities"
          info="Tier 1: model-level facts, measured against fixed prompts. These are what the capability tags everywhere else in Models are made of, and what a role assignment is checked against."
        />
        {#if record.probes.ambiguous}
          <p class="mb-3 max-w-[72ch] font-sans text-[13px] leading-5 text-warning">
            This id resolves to {record.probes.ambiguous.length} endpoints, so nothing was recorded: a fact learned from one endpoint must never
            be credited to another. Re-run against {record.probes.ambiguous.join(' or ')} to record it.
          </p>
        {/if}
        <SettingsGroup>
          {#each record.probes.results as r (r.id)}
            <SettingsRow label={r.label}>
              {#snippet hint()}
                {#if r.outcome.kind === 'scored'}
                  {r.outcome.verdict.detail}
                {:else if r.outcome.kind === 'known'}
                  <!-- ALREADY MEASURED, so no call was made and the standing
                       fact is shown. NOT the same as skipped: a fact exists. -->
                  {r.outcome.verdict.detail} Measured {new Date(r.outcome.at).toLocaleDateString()}, not run again.
                {:else if r.outcome.kind === 'skipped'}
                  {r.outcome.reason}
                {:else}
                  {r.outcome.reason}. That is the deployment, not the model.
                {/if}
              {/snippet}
              {#if r.outcome.kind === 'scored' || r.outcome.kind === 'known'}
                <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">{pct(r.outcome.verdict.score)}</span>
                <Chip tone={r.outcome.verdict.value ? 'success' : 'warn'}>{r.outcome.verdict.value ? 'Yes' : 'No'}</Chip>
              {:else if r.outcome.kind === 'skipped'}
                <!-- Skipped writes NOTHING. Not a pass and not a failure — the
                     channel could not be opened, so no fact exists. -->
                <Chip>Skipped</Chip>
              {:else}
                <Chip tone="warn">Errored</Chip>
              {/if}
            </SettingsRow>
          {/each}
        </SettingsGroup>
        <p class="mt-3 font-sans text-[13px] leading-5 tabular-nums text-muted">
          {record.probes.wrote} fact{record.probes.wrote === 1 ? '' : 's'} recorded · median {record.probes.latency.p50} ms · 95th percentile {record.probes.latency.p95} ms
        </p>
      {:else if pane === 'fixtures' && record}
        <SectionHeader
          title="Fixtures"
          info="Every fixture this run touched, by category. Open one for the fixture's own reason, what the model actually DID (every tool call, its arguments and what came back), the whole turn history where there was one, and the exact prompt and reply. This is what tells you whether a failure is the model's or our harness's."
        />
        <FitnessCases cases={record.cases} dropped={record.droppedCases} {harnessLabels} />
      {:else if pane === 'adversarial' && record?.adversarial}
        {@const adv = record.adversarial}
        <SectionHeader
          title="Safety"
          info="Tier 3: safety provocations, scored TWICE. `resistance` is the model on its own, with the guard's grounding deliberately omitted; `after guardrails` is what production would actually have filed, because Talaria runs guardrails.ts over every harness that declares them. The seeds are built to be hard and strong models land in the eighties on the first number; the gap between the two is the layer the platform adds."
        />
        <div class="mb-3 flex flex-wrap items-center gap-2">
          <!-- Tier 3 bands are a SUBSET of the matrix's bands, not a second
               vocabulary, so this chip reads out of the same table as every
               other band on the page. -->
          <Chip tone={SAFETY_META[adv.band].tone}>{SAFETY_META[adv.band].label}</Chip>
          <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">
            Model alone {adv.resistance === null ? 'could not be scored' : pct(adv.resistance)}
            {#if adv.guardedResistance !== null}
              · <span class="text-success">after guardrails {pct(adv.guardedResistance)}</span>
            {/if}
            · {adv.silent} silent · {adv.errored} voided
          </span>
          <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">
            {#if adv.escalation.adversary}
              Adversary <span class="font-mono text-xs">{adv.escalation.adversary}</span> wrote {adv.escalation.written} of {adv.escalation.attempted} turns, {adv.escalation.fell} landed.
            {:else}
              Seed corpus only. No escalation round ran.
            {/if}
          </span>
        </div>
        <!-- SAID IN WORDS, ONCE, because the two percentages above are the whole
             point of this pane and a reader who takes the first for the answer
             comes away with the wrong conclusion about every model on the page. -->
        <p class="mb-4 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
          {SAFETY_META[adv.band].blurb}
          <strong class="font-medium text-fg">Model alone</strong> is the weights with the guard's grounding off.
          <strong class="font-medium text-fg">After guardrails</strong> is the same run as production would have recorded it.
        </p>
        <div class="overflow-x-auto rounded-lg border border-line">
          <table class="w-full border-collapse text-left">
            <thead>
              <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
                <th class="px-5 py-2.5 font-medium">Rule</th>
                <th class="px-3 py-2.5 text-right font-medium">Took the bait</th>
                <th class="px-3 py-2.5 text-right font-medium">Filed in production</th>
                <th class="py-2.5 pl-3 pr-5 font-medium">Grounding</th>
              </tr>
            </thead>
            <tbody>
              {#each adv.rules as r (r.rule)}
                <tr class="h-11 border-b border-line-subtle last:border-0">
                  <td class="px-5 font-mono text-xs text-fg">{r.rule}</td>
                  <td class={cn('px-3 text-right font-sans text-[13px] tabular-nums', r.elicited > 0 ? 'text-danger' : 'text-muted')}>{r.elicited} of {r.scored}</td>
                  <td class="px-3 text-right font-sans text-[13px] tabular-nums text-fg">{r.filed}</td>
                  <td class="pl-3 pr-5 font-sans text-[13px] tabular-nums text-muted">
                    {r.filed === r.elicited ? 'No change' : `${r.filed < r.elicited ? 'Dropped' : 'Added'} ${Math.abs(r.elicited - r.filed)}`}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {@const fell = adv.cases.filter((c) => c.elicited && (c.prompt || c.raw))}
        {#if fell.length > 0}
          <div class="mt-4 space-y-2">
            {#each fell as c (c.id)}
              <Disclosure title="{c.target} · {c.id}{c.origin === 'adversary' ? ' (escalated)' : ''}">
                <div class="space-y-3 py-1">
                  {#if c.prompt}<div class="whitespace-pre-wrap font-mono text-xs leading-5 text-muted">{c.prompt}</div>{/if}
                  {#if c.raw}<div class="whitespace-pre-wrap border-t border-line-subtle pt-3 font-mono text-xs leading-5 text-fg">{c.raw}</div>{/if}
                </div>
              </Disclosure>
            {/each}
          </div>
        {/if}
      {:else}
        <SectionHeader
          title="Tested against production"
          info="The bench on the left, the last {detail.thresholds.observedWindowDays} days of production on the right, computed by the same definitions. A model that benched Ready and is repairing 12% of its production runs is the alert no external benchmark can give you."
        />
        <FitnessObserved
          tested={record?.harnesses ?? []}
          observed={detail.observed}
          observedModel={detail.observedModel}
          divergences={detail.divergences}
          thresholds={detail.thresholds}
          {harnessLabels}
        />
      {/if}
      </div>
      {/if}
    </div>
  {/key}
</div>
