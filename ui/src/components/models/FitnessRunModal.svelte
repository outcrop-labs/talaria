<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getJson } from '@/lib/fetch-json'
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
      const res = await fetch('/api/admin/model-fitness', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          action: 'start',
          model,
          tiers,
          adversaryModel: wantsAdversarial && adversary ? adversary : null,
          reprobe,
          concurrency,
        }),
      })
      if (!res.ok) {
        const body = (await res.json().catch(() => ({}))) as { error?: string }
        failure = body.error ?? (res.status === 409 ? 'A fitness run is already in flight.' : 'Could not start the run.')
        return
      }
      onStarted()
    } finally {
      starting = false
    }
  }
</script>

<Modal {open} {onClose} title="Test a model" width="max-w-2xl">
  <div class="space-y-5">
    <div>
      <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Candidate</div>
      <Select size="sm" class="w-full" bind:value={model}>
        {#each models as m (m.id)}<option value={m.id}>{m.id}</option>{/each}
      </Select>
      <div class="mt-2 flex flex-wrap items-center gap-1">
        <CapabilityTags {row} />
      </div>
      {#if row?.pooled}
        <p class="mt-2 max-w-prose font-sans text-xs text-warning">
          This id is served by {row.endpoints.length} endpoints, and what a model can do is a property of the endpoint serving it. The run
          will happen, but no capability facts get recorded — pick one of the <span class="font-mono">endpoint/model</span> ids to record them.
        </p>
      {/if}
    </div>

    <div>
      <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Tiers</div>
      <div class="space-y-2 rounded-xl border border-line-subtle bg-surface p-3">
        {#each Object.entries(TIER_META) as [id, meta] (id)}
          <div>
            <Checkbox
              class="gap-2 text-sm text-fg"
              checked={tiers.includes(id as TierId)}
              onChange={() => toggle(id as TierId)}
              label={meta.label}
            />
            <p class="ml-6 max-w-prose font-sans text-xs text-muted">{meta.blurb}</p>
            <!-- Sub-option of the tier it modifies rather than a separate
                 section: it is meaningless without tier 1 ticked, and a control
                 that can be set while doing nothing is a control that misleads. -->
            {#if id === 'probes' && wantsProbes}
              <div class="ml-6 mt-1.5" transition:slide={{ duration: 150 }}>
                <Checkbox
                  class="gap-2 text-xs text-muted"
                  checked={reprobe}
                  onChange={() => (reprobe = !reprobe)}
                  label="Re-measure capabilities already probed"
                />
                <p class="ml-6 max-w-prose font-sans text-xs text-ink-dim">
                  {#if reprobe}
                    Every probe runs again and overwrites what we recorded. Use it when this model id has been re-pointed at different weights.
                  {:else if (estimate?.tiers.find((t) => t.tier === 'probes')?.calls ?? 0) === 0 && estimateQuery.data}
                    Every capability was already measured on this endpoint, so tier 1 will make no calls at all.
                  {:else}
                    Capabilities an earlier run already established are reused, not re-bought — a probe fact belongs to the endpoint serving this
                    model and does not go stale on its own.
                  {/if}
                </p>
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </div>

    {#if tiers.includes('evals')}
      <div transition:slide={{ duration: 150 }}>
        <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Fixtures at once</div>
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
        <p class="mt-1.5 max-w-prose font-sans text-xs text-muted">
          {#if concurrency === 1}
            One at a time. Slow, and the only setting where p50 latency means what a single call costs.
          {:else}
            The sweep halves this by itself if the provider answers with rate limits, and the report says it did — a 429 is a fact about your
            deployment, never about the model. Latency is reported alongside the width it was measured at.
          {/if}
        </p>
      </div>
    {/if}

    {#if wantsAdversarial}
      <div transition:slide={{ duration: 150 }}>
        <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Adversary</div>
        <Select size="sm" class="w-full" bind:value={adversary}>
          <option value="">No escalation round (seed corpus only)</option>
          <!-- The candidate is not offered: a model grading its own resistance
               is the who-judges-the-judge regress with the stakes raised. -->
          {#each models.filter((m) => m.id !== model) as m (m.id)}<option value={m.id}>{m.id}</option>{/each}
        </Select>
        {#if estimateQuery.data}
          <p class="mt-1.5 max-w-prose font-sans text-xs text-muted">{estimateQuery.data.adversaryRequirement.note}</p>
        {/if}
      </div>
    {/if}

    <!-- THE PRICE, BEFORE THE BUTTON. -->
    <div class="rounded-xl border border-line-subtle bg-surface p-3">
      {#if tiers.length === 0}
        <p class="font-sans text-xs text-muted">Pick at least one tier.</p>
      {:else if estimateQuery.isPending}
        <Skeleton class="h-3 w-64 rounded-full" />
        <Skeleton class="mt-2 h-2.5 w-40 rounded-full" delay={0.1} />
      {:else if !estimate}
        <p class="font-sans text-xs text-danger">Could not price this run. Nothing has started.</p>
      {:else}
        <div class="font-sans text-sm text-fg">{estimateSentence(estimate)}</div>
        <ul class="mt-2 space-y-1">
          {#each estimate.tiers as t (t.tier)}
            <li class="flex flex-wrap items-baseline gap-2 font-mono text-[11px] text-muted">
              <span class="w-40 shrink-0 text-fg">{TIER_META[t.tier].label}</span>
              <span>{t.calls} call{t.calls === 1 ? '' : 's'}</span>
              <span>·</span>
              <span>{t.promptTokens.toLocaleString()} in / {t.completionTokens.toLocaleString()} out</span>
              <span>·</span>
              <span>{usd(t.usd)}</span>
              <span class="basis-full font-sans text-[11px] text-muted/80">{t.note}</span>
            </li>
          {/each}
        </ul>
        {#if estimate.unmeasuredHarnesses > 0}
          <Chip tone="warn" class="mt-2" title="Tokens for a harness are taken from the last time it actually ran. Harnesses that have never run contribute nothing, so the total is a floor.">
            floor, not a total
          </Chip>
        {/if}
      {/if}
    </div>

    {#if failure}
      <p transition:slide={{ duration: 150 }} class="font-sans text-xs text-danger">{failure}</p>
    {/if}
  </div>

  {#snippet footer()}
    <div class="flex items-center gap-2">
      <span class="font-sans text-xs text-muted">
        The run charges your provider. Tier 2 can be stopped at any case boundary and resumes where it left off.
      </span>
      <span class="ml-auto"></span>
      <Button size="sm" variant="ghost" onclick={onClose}>Cancel</Button>
      <!-- Armed only once the estimate is on screen: that is the confirmation
           step for an action that spends real money. -->
      <Button size="sm" onclick={() => void start()} disabled={starting || !estimate || tiers.length === 0}>
        {estimate ? `Start — ${estimate.calls} calls, ${usd(estimate.usd)}` : 'Start'}
      </Button>
    </div>
  {/snippet}
</Modal>
