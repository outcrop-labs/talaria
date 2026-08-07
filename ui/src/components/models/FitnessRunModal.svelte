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
  import { estimateSentence, TIER_META, usd, type ModelRow, type RunEstimate, type TierId } from './fitness'

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
  let starting = $state(false)
  let failure = $state<string | null>(null)

  const wantsAdversarial = $derived(tiers.includes('adversarial'))
  const tierParam = $derived([...tiers].sort().join(','))
  const adversaryParam = $derived(wantsAdversarial ? adversary : '')

  const estimateQuery = createQuery(() => ({
    queryKey: ['model-fitness-estimate', model, tierParam, adversaryParam],
    enabled: open && tiers.length > 0 && model !== '',
    queryFn: (): Promise<{ estimate: RunEstimate; adversaryRequirement: { capabilities: string[]; note: string } }> =>
      getJson(
        `/api/admin/model-fitness?view=estimate&model=${encodeURIComponent(model)}&tiers=${tierParam}` +
          (adversaryParam ? `&adversary=${encodeURIComponent(adversaryParam)}` : ''),
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
      <div class="space-y-2 rounded-xl border border-line-subtle p-3">
        {#each Object.entries(TIER_META) as [id, meta] (id)}
          <div>
            <Checkbox
              class="gap-2 text-sm text-fg"
              checked={tiers.includes(id as TierId)}
              onChange={() => toggle(id as TierId)}
              label={meta.label}
            />
            <p class="ml-6 max-w-prose font-sans text-xs text-muted">{meta.blurb}</p>
          </div>
        {/each}
      </div>
    </div>

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
    <div class="rounded-xl border border-line-subtle p-3">
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
