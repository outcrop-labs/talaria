<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { formatTokens, modelCostLabel, type CostOverview } from '@/lib/cost.svelte'
  import { SERIES, SERVED_ORDER } from './series'
  import ShareBar from './ShareBar.svelte'
  import { table } from './grammar'

  /** Where the last 30 days of tokens were served: the two classes as one
   *  bar, then every model with its class, its share and what it cost. The
   *  three class totals (local + cloud + unattributed) always reconcile with
   *  the 30-day token tile. */
  let {
    split,
    perModel,
  }: {
    split: { local: number; cloud: number; other: number }
    perModel: CostOverview['perModel']
  } = $props()

  const total = $derived(split.local + split.cloud + split.other)
  const parts = $derived(SERVED_ORDER.filter((k) => split[k] > 0))
  const percent = (n: number) => `${Math.round((n / total) * 100)}%`

  const models = $derived(perModel.filter((m) => m.tokens > 0))
  const modelTokens = $derived(models.reduce((n, m) => n + m.tokens, 0))
</script>

<SettingsSection
  title="Self-hosted and cloud"
  description="Where the last 30 days of tokens were served, and what each model cost."
  info="Dollars are the provider's reported charge when it sends one. Otherwise they come from the provider's published price (the serving endpoint's own on OpenRouter when the call names it). Where nothing is published, they come from a price an admin set, or their note that the model is included in a plan ($0, labelled)."
>
  <SettingsGroup>
    <div class="px-5 py-4">
      <div class="flex flex-wrap items-center gap-x-6 gap-y-1 font-sans text-[13px] leading-5">
        {#each parts as k (k)}
          <span class="flex items-center gap-2 text-muted">
            <StatusDot color={SERIES[k].color} class="h-2 w-2" />
            {SERIES[k].label}
            <span class="tabular-nums text-fg">{formatTokens(split[k])} tokens</span>
            <span class="tabular-nums">{percent(split[k])}</span>
          </span>
        {/each}
      </div>
      <div
        class="mt-3 flex h-2.5 gap-0.5 overflow-hidden rounded-full"
        role="img"
        aria-label={`Tokens by where they were served: ${parts.map((k) => `${SERIES[k].label} ${percent(split[k])}`).join(', ')}`}
      >
        {#each parts as k (k)}
          <div
            title={`${SERIES[k].label}: ${formatTokens(split[k])} tokens, ${percent(split[k])}`}
            class="min-w-1"
            style:width="{(split[k] / total) * 100}%"
            style:background={SERIES[k].color}
          ></div>
        {/each}
      </div>
    </div>
    {#if models.length > 0}
      <div class="overflow-x-auto">
        <table class={table.root}>
          <thead>
            <tr>
              <th class={table.th}>Model</th>
              <th class={table.th}>Served by</th>
              <th class={cn(table.th, 'w-[30%]')}>Share of tokens</th>
              <th class={cn(table.th, table.num)}>Tokens</th>
              <th class={cn(table.th, table.num)}>Cost</th>
            </tr>
          </thead>
          <tbody>
            {#each models as m (m.llmModel ?? '?')}
              {@const series = SERIES[m.endpointClass ?? 'other']}
              <tr class={table.tr}>
                <td class={cn(table.td, m.llmModel ? table.id : 'text-muted')}>{m.llmModel ?? 'Unattributed'}</td>
                <td class={cn(table.td, 'whitespace-nowrap text-[13px] text-muted')}>{series.label}</td>
                <td class={table.td}><ShareBar share={m.tokens / modelTokens} color={series.color} /></td>
                <td class={cn(table.td, table.num)}>{formatTokens(m.tokens)}</td>
                <td class={cn(table.td, table.num, m.endpointClass !== 'cloud' && 'text-muted')}>
                  {m.endpointClass === 'cloud' ? modelCostLabel(m) : '—'}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </SettingsGroup>
</SettingsSection>
