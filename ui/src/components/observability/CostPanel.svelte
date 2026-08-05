<script lang="ts">
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { fade, slide } from '@/lib/motion'
  import { agentLabel, formatCost, formatTokens, useCost, type CostTotals } from '@/lib/cost.svelte'
  import SplitPanel from './SplitPanel.svelte'
  import DailyBars from './DailyBars.svelte'

  // The token ledger: every agent generation (1:1 chat + channel replies) lands in
  // usage_events — real gateway-reported counts, or char-based estimates (~).
  const costQuery = useCost()
  const data = $derived(costQuery.data)
  const isLoading = $derived(costQuery.isLoading)
  const t = $derived(data?.totals)
  const perAgent = $derived(data?.perAgent ?? [])
  const perDay = $derived(data?.perDay ?? [])
  // "~" when the window is dominated by estimates rather than reported usage.
  const approx = $derived((t?.estimatedShare ?? 0) > 0.5 ? '~' : '')
  const total = (x?: CostTotals) => (x ? x.prompt + x.completion : 0)

  // `data` is undefined on rejection, and the old guard read
  // `!data || total(t?.month) === 0` — so a 500 on /api/cost rendered
  // "No usage recorded yet" over a ledger holding hundreds of usage_events.
  // That is a claim about SPEND made from a read that failed. Split the three
  // answers apart: broke, still loading, genuinely zero.
  const hardFailure = $derived(costQuery.isError && data === undefined)
  // A failed BACKGROUND refetch (this query polls every 60s) keeps the last
  // good ledger on screen — blanking real numbers over a blip is the worse
  // lie — but it must not pass stale totals off as current.
  const staleFailure = $derived(costQuery.isError && data !== undefined)
</script>

<div>
  <div class="space-y-8">
    {#if staleFailure}
      <div transition:slide={{ duration: 150 }}>
        <QueryError
          variant="inline"
          title="Usage may be out of date"
          error={costQuery.error}
          onRetry={() => void costQuery.refetch()}
        />
      </div>
    {/if}

    {#if hardFailure}
      <QueryError
        title="Could not load usage"
        error={costQuery.error}
        onRetry={() => void costQuery.refetch()}
      />
    {:else if isLoading || !data}
      <div class="grid grid-cols-3 gap-4">
        {#each [0, 1, 2] as i (i)}
          <SkeletonCard delay={i * 0.1} />
        {/each}
      </div>
      <SkeletonRows rows={5} class="mt-2" />
    {:else if total(t?.month) === 0}
      <div in:fade={{ duration: 150 }}>
        <EmptyState
          icon="⌗"
          title="No usage recorded yet"
          hint="Every chat turn and channel reply lands here once agents start talking."
        />
      </div>
    {:else}
      <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <StatCard
          label="Tokens today"
          value={approx + formatTokens(total(t?.today))}
          sub={`${formatTokens(t?.today.prompt ?? 0)} in · ${formatTokens(t?.today.completion ?? 0)} out`}
        />
        <StatCard
          label="Tokens · 7 days"
          value={approx + formatTokens(total(t?.week))}
          sub={`${formatTokens(t?.week.prompt ?? 0)} in · ${formatTokens(t?.week.completion ?? 0)} out`}
        />
        <StatCard
          label="Tokens · 30 days"
          value={approx + formatTokens(total(t?.month))}
          sub={`${formatTokens(t?.month.prompt ?? 0)} in · ${formatTokens(t?.month.completion ?? 0)} out`}
        />
        <StatCard
          label="Cloud spend · 30 days"
          value={formatCost(t?.month.cost ?? 0)}
          sub={`today ${formatCost(t?.today.cost ?? 0)} · ${t?.month.generations ?? 0} generations`}
        />
      </div>

      <!-- Missing pricing is attention (gold), not failure — orange stays
           reserved for real failures (spec §1 semantics). -->
      {#if (t?.unpricedCloudTokens ?? 0) > 0}
        <p transition:slide={{ duration: 150 }} class="text-xs text-warning">
          {formatTokens(t!.unpricedCloudTokens)} cloud tokens have no price configured. Set per-model pricing on
          the Models page so spend is complete.
        </p>
      {/if}

      {#if t && t.split.local + t.split.cloud + t.split.other > 0}
        <SplitPanel split={t.split} perModel={data.perModel} />
      {/if}

      {#if perDay.length > 1}
        <Panel>
          <SectionHeader title="Tokens per day · last 14 days">
            {#snippet action()}
              <span class="inline-flex items-center gap-1.5">
                <span class="h-2 w-2 rounded-full" style:background="var(--theme-success)"></span> self-hosted
              </span>
              <span class="inline-flex items-center gap-1.5">
                <span class="h-2 w-2 rounded-full" style:background="var(--theme-chart-1)"></span> cloud
              </span>
            {/snippet}
          </SectionHeader>
          <DailyBars days={perDay} />
        </Panel>
      {/if}

      <Panel>
        <SectionHeader title="By agent · 30 days" action={String(perAgent.length).padStart(2, '0')} />
        <ul class="divide-y divide-line">
          {#each perAgent as a (a.agentModel)}
            {@const d = agentLabel(a.agentModel)}
            <li class="flex items-center gap-3 py-3 transition-colors hover:bg-hover">
              <Avatar name={d.label} class="h-6 w-6" />
              <span class="min-w-0 flex-1">
                <span class="block truncate font-sans text-sm text-fg">{d.label}</span>
                <span class="block truncate font-sans text-xs text-muted">{d.role}</span>
              </span>
              <span class="w-20 text-right font-mono text-[11px] text-muted">{formatTokens(a.prompt)} in</span>
              <span class="w-20 text-right font-mono text-[11px] text-muted">{formatTokens(a.completion)} out</span>
              <span class="w-16 text-right font-mono text-[11px] text-success">
                {a.localShare === null ? '—' : `${Math.round(a.localShare * 100)}% self-hosted`}
              </span>
              <span class="w-20 text-right font-mono text-sm text-fg">{formatTokens(a.prompt + a.completion)}</span>
              <span class="w-16 text-right font-mono text-[11px] text-muted">{formatCost(a.cost)}</span>
              <span class="w-16 text-right font-mono text-[11px] text-muted">{a.generations} gen</span>
              <span class="w-20 text-right font-mono text-[11px] text-muted">{relativeTime(a.lastUsed)}</span>
            </li>
          {/each}
        </ul>
      </Panel>

      {#if (t?.estimatedShare ?? 0) > 0}
        <p class="text-xs text-muted">
          ~{Math.round((t?.estimatedShare ?? 0) * 100)}% of generations are character-based estimates. The
          gateway didn't report token usage for them.
        </p>
      {/if}
    {/if}
  </div>
</div>
