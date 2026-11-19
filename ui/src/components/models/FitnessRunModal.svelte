<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import { errorMessage, getJson, HttpError, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import CapabilityTags from './CapabilityTags.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { DEFAULT_CONCURRENCY, estimateSentence, TIER_META, usd, type ModelRow, type RunEstimate, type TierId } from './fitness'

  // START A FITNESS RUN. A run spends real money on someone else's inference
  // bill, so this dialog IS the confirmation step (UI-CONVENTIONS: explicit
  // apply survives exactly where an action spends): the tiers are picked, the
  // price is fetched, and Start does not arm until the estimate is on screen.
  // Nobody starts a run whose cost they have not been shown.
  let {
    open,
    models,
    model = $bindable(),
    onClose,
    onStarted,
  }: {
    open: boolean
    models: ModelRow[]
    model: string
    onClose: () => void
    onStarted: () => void
  } = $props()

  let tiers = $state<TierId[]>(['probes', 'evals'])
  let adversary = $state('')
  // OFF BY DEFAULT, and that is the change: a probe fact is a property of an
  // `endpoint:model` and does not go stale on its own, so re-buying nine calls
  // on every sweep of a model tested last month was spend with no new
  // information behind it. Ticked, it re-measures — the softer twin of "Forget
  // recorded capabilities", which throws the facts away instead.
  let reprobe = $state(false)
  // HOW MANY FIXTURES RUN AT ONCE. A 247-fixture sweep one at a time is most of
  // an hour; four wide is minutes. It drops itself if the provider pushes back
  // (see `DEFAULT_CONCURRENCY`), so this is a ceiling rather than a promise.
  let concurrency = $state(DEFAULT_CONCURRENCY)
  let starting = $state(false)
  let failure = $state<string | null>(null)

  const wantsAdversarial = $derived(tiers.includes('adversarial'))
  const wantsProbes = $derived(tiers.includes('probes'))
  const tierParam = $derived([...tiers].sort().join(','))
  const adversaryParam = $derived(wantsAdversarial ? adversary : '')

  const estimateQuery = createQuery(() => ({
    queryKey: ['model-fitness-estimate', model, tierParam, adversaryParam, reprobe],
    enabled: open && tiers.length > 0 && model !== '',
    queryFn: (): Promise<{ estimate: RunEstimate; adversaryRequirement: { capabilities: string[]; note: string } }> =>
      getJson(
        `/api/admin/model-fitness?view=estimate&model=${encodeURIComponent(model)}&tiers=${tierParam}` +
          (adversaryParam ? `&adversary=${encodeURIComponent(adversaryParam)}` : '') +
          (reprobe ? '&reprobe=1' : ''),
      ),
  }))
  const estimate = $derived(estimateQuery.data?.estimate ?? null)
  const row = $derived(models.find((m) => m.id === model))

  const toggle = (tier: TierId) => {
    tiers = tiers.includes(tier) ? tiers.filter((t) => t !== tier) : [...tiers, tier]
  }

  const start = async () => {
    starting = true
    failure = null
    try {
      await postJson('/api/admin/model-fitness', {
        action: 'start',
        model,
        tiers,
        adversaryModel: wantsAdversarial && adversary ? adversary : null,
        reprobe,
        concurrency,
      })
      onStarted()
    } catch (e) {
      // 409 keeps its own sentence — "already in flight" tells the operator
      // what to do next; the generic envelope never would.
      failure = e instanceof HttpError && e.status === 409 ? 'A fitness run is already in flight.' : errorMessage(e)
    } finally {
      starting = false
    }
  }

  // The one sentence under each tier. `TIER_META`'s longer blurb is the tip.
  const TIER_HINT: Record<TierId, string> = {
    probes: 'What the model can do: JSON, tools, search, long context. Seconds and cents.',
    evals: "Every harness's own fixtures, replayed against this model. The slow one.",
    adversarial: 'Safety provocations, scored with the production guard rules.',
  }
</script>

<Modal {open} {onClose} title="Test a model" width="max-w-2xl">
  <div class="space-y-6">
    <SettingsField label="Model">
      <!-- A required pick: no Auto row — every option is a model. -->
      <ModelIdPicker models={models.map((m) => m.id)} value={model} onChange={(m) => m && (model = m)} class="w-full" />
      <CapabilityTags {row} />
      {#if row?.pooled}
        <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-warning">
          This id is served by {row.endpoints.length} endpoints, and what a model can do belongs to the endpoint serving it. The run will
          happen, but no capability facts get recorded. Pick one of the <span class="font-mono text-xs">endpoint/model</span> ids to record them.
        </p>
      {/if}
    </SettingsField>

    <section>
      <h3 class="mb-2 font-sans text-[13px] font-medium leading-5 text-fg">What to run</h3>
      <SettingsGroup>
        {#each Object.entries(TIER_META) as [id, meta] (id)}
          <SettingsRow label={meta.label} hint={TIER_HINT[id as TierId]} info={meta.blurb}>
            <Toggle checked={tiers.includes(id as TierId)} onChange={() => toggle(id as TierId)} title={meta.label} />
          </SettingsRow>
          <!-- A sub-option of the tier it modifies rather than a separate
               section: it is meaningless without tier 1 on, and a control that
               can be set while doing nothing is a control that misleads. -->
          {#if id === 'probes' && wantsProbes}
            <div transition:slide={{ duration: 150 }}>
              <SettingsRow label="Measure capabilities again">
                {#snippet hint()}
                  {#if reprobe}
                    Every probe runs again and overwrites what is recorded. Use it when this model id points at different weights.
                  {:else if (estimate?.tiers.find((t) => t.tier === 'probes')?.calls ?? 0) === 0 && estimateQuery.data}
                    Every capability is already measured on this endpoint, so probes will make no calls.
                  {:else}
                    Off reuses what an earlier run established. A probe fact does not go stale on its own.
                  {/if}
                {/snippet}
                <Toggle checked={reprobe} onChange={() => (reprobe = !reprobe)} title="Measure capabilities again" />
              </SettingsRow>
            </div>
          {/if}
        {/each}
      </SettingsGroup>
    </section>

    {#if tiers.includes('evals')}
      <div transition:slide={{ duration: 150 }}>
        <SettingsField
          label="Fixtures at once"
          hint={concurrency === 1
            ? 'One at a time. Slow, and the only setting where median latency means what a single call costs.'
            : 'The run halves this by itself if the provider answers with rate limits, and the report says it did.'}
        >
          <div>
            <Segmented
              options={[
                { id: '1', label: '1', title: 'Strictly sequential. Right for a self-hosted model behind one GPU, and the only setting where the latency figures mean "what one call costs".' },
                { id: '2', label: '2', title: 'Gentle. A good first try against a small self-hosted deployment.' },
                { id: '4', label: '4', title: 'The default. Unremarkable against a hosted gateway and roughly three times faster than sequential.' },
                { id: '8', label: '8', title: 'For a hosted gateway you know tolerates it. Remember this multiplies with the number of candidates you test at once.' },
              ]}
              value={String(concurrency)}
              onChange={(id) => (concurrency = Number(id))}
            />
          </div>
        </SettingsField>
      </div>
    {/if}

    {#if wantsAdversarial}
      <div transition:slide={{ duration: 150 }}>
        <!-- The candidate is not offered: a model grading its own resistance is
             the who-judges-the-judge regress with the stakes raised. -->
        <SettingsField label="Adversary" hint={estimateQuery.data?.adversaryRequirement.note}>
          <ModelIdPicker
            models={models.filter((m) => m.id !== model).map((m) => m.id)}
            value={adversary || null}
            onChange={(m) => (adversary = m ?? '')}
            emptyLabel="No escalation round"
            emptySub="Seed corpus only"
            class="w-full"
          />
        </SettingsField>
      </div>
    {/if}

    <!-- THE PRICE, BEFORE THE BUTTON. -->
    <section>
      <h3 class="mb-2 font-sans text-[13px] font-medium leading-5 text-fg">What it will cost</h3>
      {#if tiers.length === 0}
        <p class="font-sans text-[13px] leading-5 text-muted">Pick at least one thing to run.</p>
      {:else if estimateQuery.isPending}
        <Skeleton class="h-3 w-64 rounded-full" />
        <Skeleton class="mt-2 h-2.5 w-40 rounded-full" />
      {:else if !estimate}
        <p class="font-sans text-[13px] leading-5 text-danger" role="alert">Could not price this run. Nothing has started.</p>
      {:else}
        <p class="mb-3 font-sans text-sm leading-5 text-fg">{estimateSentence(estimate)}</p>
        <div class="overflow-x-auto rounded-lg border border-line">
          <table class="w-full border-collapse text-left">
            <thead>
              <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
                <th class="px-5 py-2.5 font-medium">Step</th>
                <th class="px-3 py-2.5 text-right font-medium">Calls</th>
                <th class="px-3 py-2.5 text-right font-medium">Tokens in</th>
                <th class="px-3 py-2.5 text-right font-medium">Tokens out</th>
                <th class="py-2.5 pl-3 pr-5 text-right font-medium">Cost</th>
              </tr>
            </thead>
            <tbody>
              {#each estimate.tiers as t (t.tier)}
                <tr class="border-b border-line-subtle font-sans text-[13px] tabular-nums last:border-0">
                  <td class="px-5 py-2.5">
                    <div class="text-fg">{TIER_META[t.tier].label}</div>
                    <div class="text-xs leading-4 text-muted">{t.note}</div>
                  </td>
                  <td class="px-3 text-right text-muted">{t.calls}</td>
                  <td class="px-3 text-right text-muted">{t.promptTokens.toLocaleString()}</td>
                  <td class="px-3 text-right text-muted">{t.completionTokens.toLocaleString()}</td>
                  <td class="pl-3 pr-5 text-right text-fg">{usd(t.usd)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if estimate.unmeasuredHarnesses > 0}
          <Chip tone="warn" class="mt-3" title="Tokens for a harness are taken from the last time it actually ran. Harnesses that have never run contribute nothing, so the total is a floor.">
            A floor, not a total
          </Chip>
        {/if}
      {/if}
    </section>

    {#if failure}
      <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{failure}</p>
    {/if}
  </div>

  {#snippet footer()}
    <div class="flex items-center gap-3">
      <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">The run charges your provider. You can stop it and resume later.</span>
      <Button size="sm" variant="ghost" onclick={onClose}>Cancel</Button>
      <!-- Armed only once the estimate is on screen: that is the confirmation
           step for an action that spends real money. -->
      <Button size="sm" onclick={() => void start()} disabled={starting || !estimate || tiers.length === 0}>
        {estimate ? `Start, ${estimate.calls} calls, ${usd(estimate.usd)}` : 'Start'}
      </Button>
    </div>
  {/snippet}
</Modal>
