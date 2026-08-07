<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { cn } from '@/lib/cn'
  import CapabilityTags from './CapabilityTags.svelte'
  import FitnessCases from './FitnessCases.svelte'
  import FitnessObserved from './FitnessObserved.svelte'
  import { BAND_META, BAND_SEVERITY, BAND_TEXT, pct, type DetailPayload, type ModelRow } from './fitness'

  // One model, in full: what the run established, where it broke, and what
  // production says about the same model on the same definitions.
  let { detail, row }: { detail: DetailPayload; row: ModelRow | undefined } = $props()

  const record = $derived(detail.record)
  const harnessLabels = $derived(Object.fromEntries((record?.harnesses ?? []).map((h) => [h.id, h.label])))

  // Worst first. An admin opening a model wants the thing that would bite them,
  // not an alphabetical tour. `BAND_SEVERITY` and `BAND_TEXT` come from
  // `fitness.ts` rather than being restated here: three copies of the band
  // ordering and two of the colour table is exactly how one panel comes to call
  // a cell amber that another calls red.
  const slots = $derived([...(record?.report.slots ?? [])].sort((a, b) => BAND_SEVERITY[a.band] - BAND_SEVERITY[b.band]))
</script>

<div class="space-y-4">
  <Panel>
    <div class="flex flex-wrap items-baseline gap-2">
      <span class="font-mono text-sm text-fg">{detail.model}</span>
      <CapabilityTags {row} />
      <span class="ml-auto font-mono text-[11px] text-muted">
        {#if record}
          tested {new Date(record.at).toLocaleString()} · {record.tiers.join(' + ')}
        {:else}
          never tested
        {/if}
      </span>
    </div>

    {#if !record}
      <EmptyState
        class="mt-3"
        variant="compact"
        icon="◇"
        title="No run on record for this model"
        hint="Everything below is production telemetry. Run the probes to fill in what this model can do, and the harness tier to fill in the matrix."
      />
    {:else}
      {#if record.sweep.state === 'stopped' || (record.sweep.total > 0 && record.sweep.done < record.sweep.total)}
        <p class="mt-2 max-w-prose font-sans text-xs text-warning">
          This sweep covered {record.sweep.done} of {record.sweep.total} fixtures. Everything it did not reach is Untested, not passing — start
          it again and it resumes where it stopped.
        </p>
      {/if}
      {#if !record.report.guarded}
        <p class="mt-2 max-w-prose font-sans text-xs text-warning">
          The guard was off for this run, so every guard rate below is zero because nothing was checked — not because nothing was found. No
          slot can be called Ready on that evidence.
        </p>
      {/if}
      {#if record.sweep.unfixtured.length > 0}
        <p class="mt-2 max-w-prose font-sans text-xs text-muted">
          {record.sweep.unfixtured.length} registered harness(es) declare no fixtures, so tier 2 says nothing about them: {record.sweep.unfixtured.join(', ')}.
        </p>
      {/if}
    {/if}
  </Panel>

  {#if record?.probes}
    <Panel>
      <SectionHeader
        title="Capabilities"
        info="Tier 1 — model-level facts, measured against fixed prompts. These are what the capability tags everywhere else in Models are made of, and what a role assignment is checked against."
      />
      {#if record.probes.ambiguous}
        <p class="mb-2 max-w-prose font-sans text-xs text-warning">
          This id resolves to {record.probes.ambiguous.length} endpoints, so nothing was recorded: a fact learned from one endpoint must never
          be credited to another. Re-run against {record.probes.ambiguous.join(' or ')} to record it.
        </p>
      {/if}
      <ul class="space-y-1.5">
        {#each record.probes.results as r (r.id)}
          <li class="flex flex-wrap items-baseline gap-2">
            <span class="w-40 shrink-0 font-mono text-[11px] text-fg">{r.label}</span>
            {#if r.outcome.kind === 'scored'}
              <Chip tone={r.outcome.verdict.value ? 'success' : 'danger'}>
                {r.outcome.verdict.value ? 'yes' : 'no'} · {pct(r.outcome.verdict.score)}
              </Chip>
              <span class="min-w-0 flex-1 font-sans text-xs text-muted">{r.outcome.verdict.detail}</span>
            {:else if r.outcome.kind === 'skipped'}
              <!-- Skipped writes NOTHING. Not a pass and not a failure — the
                   channel could not be opened, so no fact exists. -->
              <Chip>skipped</Chip>
              <span class="min-w-0 flex-1 font-sans text-xs text-muted">{r.outcome.reason}</span>
            {:else}
              <Chip tone="warn">errored</Chip>
              <span class="min-w-0 flex-1 font-sans text-xs text-muted">{r.outcome.reason} — that is the deployment, not the model.</span>
            {/if}
          </li>
        {/each}
      </ul>
      <div class="mt-3 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        {record.probes.wrote} fact(s) recorded · p50 {record.probes.latency.p50}ms · p95 {record.probes.latency.p95}ms
      </div>
    </Panel>
  {/if}

  {#if record}
    <Panel>
      <SectionHeader
        title="Per slot"
        info="One verdict per assignable slot. The reason names the harness and the assertion that decided it — a score on its own is not something an admin can act on."
      />
      <ul class="divide-y divide-line">
        {#each slots as s (`${s.slot.kind}:${s.slot.id}`)}
          <li class="py-2.5">
            <div class="flex flex-wrap items-center gap-2">
              <span class={cn('font-mono text-[13px]', BAND_TEXT[s.band])}>{BAND_META[s.band].glyph}</span>
              <span class="font-sans text-sm text-fg">{s.slot.label}</span>
              <Chip tone={BAND_META[s.band].tone}>{BAND_META[s.band].label}</Chip>
              {#if !s.slot.live}<Chip title="Reserved: this slot takes effect when its surface lands. The verdict is still real.">reserved</Chip>{/if}
              <span class="ml-auto font-mono text-[10px] text-muted">
                {#if s.contract}{s.contract.numerator}/{s.contract.denominator} first try{/if}
                {#if s.task} · task floor {pct(s.taskFloor)}{/if}
              </span>
            </div>
            {#each s.reasons as reason, i (i)}
              <p class={cn('mt-1 max-w-prose font-sans text-xs', BAND_TEXT[reason.band])}>
                {reason.detail}
                {#if reason.assertion}<span class="text-muted"> — {reason.assertion}</span>{/if}
              </p>
            {/each}
          </li>
        {/each}
      </ul>
      {#if record.report.unbound.length > 0}
        <div class="mt-3 border-t border-line-subtle pt-3">
          <!-- Harnesses whose model comes from the SUBJECT of the call: the
               owner's assistant, the agent on the ticket, the researching
               agent. Scored, because "can this model work a ticket" is the
               question when picking an agent's brain — they simply have no
               column an admin assigns. -->
          <Disclosure title="Harnesses with no assignable slot ({record.report.unbound.length})">
            <ul class="space-y-1 p-3">
              {#each record.report.unbound as v (v.harness)}
                <li class="flex flex-wrap items-baseline gap-2 font-mono text-[11px]">
                  <span class={cn('w-48 shrink-0', BAND_TEXT[v.band])}>{v.label}</span>
                  <span class="text-muted">{BAND_META[v.band].label}</span>
                  {#if v.cases > 0}<span class="text-muted">· {pct(v.contractRate)} first try over {v.cases} fixture(s)</span>{/if}
                  {#if v.reasons[0]}<span class="basis-full font-sans text-xs text-muted">{v.reasons[0].detail}</span>{/if}
                </li>
              {/each}
            </ul>
          </Disclosure>
        </div>
      {/if}
    </Panel>

    <Panel>
      <SectionHeader
        title="Failed assertions"
        info="The fixture's own reason, the exact prompt that was sent, and the exact reply that came back. This is what makes a red cell trustworthy instead of merely alarming."
      />
      <FitnessCases cases={record.cases} dropped={record.droppedCases} {harnessLabels} />
    </Panel>
  {/if}

  {#if record?.adversarial}
    {@const adv = record.adversarial}
    <Panel>
      <SectionHeader
        title="Adversarial"
        info="Tier 3 — safety provocations scored with the production guard rules, so the numbers are directly comparable to what the guard files in production. Stricter than the other tiers: there is no repair turn for a fabricated outage, it has already been read."
      />
      <div class="mb-3 flex flex-wrap items-center gap-2">
        <!-- Tier 3 bands are a SUBSET of the matrix's bands, not a second
             vocabulary, so this chip reads out of the same table as every
             other band on the page. -->
        <Chip tone={BAND_META[adv.band].tone}>{BAND_META[adv.band].label}</Chip>
        <span class="font-mono text-[11px] text-muted">
          resistance {adv.resistance === null ? 'unscorable' : pct(adv.resistance)} · {adv.silent} silent · {adv.errored} voided
        </span>
        {#if adv.escalation.adversary}
          <span class="font-mono text-[11px] text-muted">
            adversary {adv.escalation.adversary}: {adv.escalation.written}/{adv.escalation.attempted} turns written, {adv.escalation.fell} landed
          </span>
        {:else}
          <span class="font-sans text-xs text-muted">Seed corpus only — no escalation round ran.</span>
        {/if}
      </div>
      <ul class="space-y-1">
        {#each adv.rules as r (r.rule)}
          <li class="flex flex-wrap items-baseline gap-2 font-mono text-[11px]">
            <span class="w-40 shrink-0 text-fg">{r.rule}</span>
            <span class={r.elicited > 0 ? 'text-danger' : 'text-muted'}>{r.elicited}/{r.scored} elicited</span>
            <span class="text-muted">· {r.filed} would have been filed in production</span>
          </li>
        {/each}
      </ul>
      {@const fell = adv.cases.filter((c) => c.elicited && (c.prompt || c.raw))}
      {#if fell.length > 0}
        <div class="mt-3 space-y-2">
          {#each fell as c (c.id)}
            <Disclosure title="{c.target} · {c.id}{c.origin === 'adversary' ? ' (escalated)' : ''}">
              <div class="space-y-2 p-3">
                {#if c.prompt}<div class="whitespace-pre-wrap font-mono text-[11px] text-muted">{c.prompt}</div>{/if}
                {#if c.raw}<div class="whitespace-pre-wrap rounded-md border border-line-subtle p-2 font-mono text-[11px] text-fg">{c.raw}</div>{/if}
              </div>
            </Disclosure>
          {/each}
        </div>
      {/if}
    </Panel>
  {/if}

  <Panel>
    <SectionHeader
      title="Tested vs observed"
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
  </Panel>
</div>
