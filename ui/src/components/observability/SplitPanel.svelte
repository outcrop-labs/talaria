<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { formatCost, formatTokens, type CostOverview } from '@/lib/cost.svelte'

  /** Segment colors: hue family = class (green local, chart-blue cloud — gold
   *  stays reserved for action/attention, spec §1), lightness step = model
   *  within the family. Identity is never color-alone — every segment has a
   *  legend chip with the model name and exact tokens. */
  const segmentColor = (cls: 'local' | 'cloud' | null, idxInClass: number): string => {
    if (cls === null) return 'var(--theme-line)'
    const base = cls === 'local' ? 'var(--theme-success)' : 'var(--theme-chart-1)'
    const lighten = Math.min(idxInClass * 22, 66)
    return lighten ? `color-mix(in oklab, ${base}, white ${lighten}%)` : base
  }

  /** The local-vs-cloud share, segmented by serving model. The three class
   *  totals (local + cloud + unattributed) always reconcile with the 30-day
   *  token tile. */
  let {
    split,
    perModel,
  }: {
    split: { local: number; cloud: number; other: number }
    perModel: CostOverview['perModel']
  } = $props()

  const total = $derived(split.local + split.cloud + split.other)
  const attributed = $derived(split.local + split.cloud)
  const segments = $derived.by(() => {
    // Per-model segments in class order; lightness index counted within class.
    let li = 0
    let ci = 0
    return perModel
      .filter((m) => m.tokens > 0)
      .map((m) => ({
        ...m,
        color: segmentColor(m.endpointClass, m.endpointClass === 'local' ? li++ : m.endpointClass === 'cloud' ? ci++ : 0),
        label: m.llmModel ?? 'unattributed',
      }))
  })
</script>

<Panel>
  <SectionHeader class="mb-2" title="Self-hosted vs cloud · 30 days" />
  <p class="mb-4 font-sans text-xs text-muted">
    {formatTokens(split.local)} on your own hardware · {formatTokens(split.cloud)} on cloud APIs{split.other > 0
      ? ` · ${formatTokens(split.other)} unattributed`
      : ''}{attributed > 0 ? ` · ${Math.round((split.local / total) * 100)}% self-hosted` : ''}
  </p>
  <div class="flex h-3 gap-0.5 overflow-hidden rounded-full" role="img" aria-label="Token share by serving model">
    {#each segments as s (s.label)}
      <div
        title={`${s.label}: ${formatTokens(s.tokens)} tokens (${s.endpointClass ?? 'unattributed'})`}
        style:width="{(s.tokens / total) * 100}%"
        style:background={s.color}
        style:min-width={s.tokens > 0 ? '4px' : '0'}
      ></div>
    {/each}
  </div>
  <div class="mt-3 flex flex-wrap gap-x-4 gap-y-1.5 font-mono text-[11px] text-muted">
    {#each segments as s (s.label)}
      <span class="inline-flex items-center gap-1.5">
        <StatusDot color={s.color} class="h-2 w-2" />
        <span class="text-fg">{s.label}</span>
        <span>
          {formatTokens(s.tokens)} · {s.endpointClass ?? 'unattributed'}{s.endpointClass === 'cloud'
            ? s.cost === null
              ? ' · unpriced'
              : ` · ${formatCost(s.cost)}`
            : ''}
        </span>
      </span>
    {/each}
  </div>
</Panel>
