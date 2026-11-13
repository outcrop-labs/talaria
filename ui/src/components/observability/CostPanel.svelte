<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { fade, slide } from '@/lib/motion'
  import { agentCostLabel, agentLabel, formatCost, formatTokens, useCost, type CostTotals } from '@/lib/cost.svelte'
  import { useSession } from '@/lib/session'
  import DailyBars from './DailyBars.svelte'
  import SplitPanel from './SplitPanel.svelte'
  import StatStripSkeleton from './StatStripSkeleton.svelte'
  import { table } from './grammar'
  import UnpricedPanel from './UnpricedPanel.svelte'

  // The token ledger: every agent generation (1:1 chat + channel replies) lands in
  // usage_events, as real gateway-reported counts or char-based estimates (~).
  const costQuery = useCost()
  // Admins get the unpriced panel (the fix is theirs to make); everyone else
  // keeps the one-line note.
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const data = $derived(costQuery.data)
  const t = $derived(data?.totals)
  // By total tokens, largest first: the column the table sets in medium weight.
  const perAgent = $derived([...(data?.perAgent ?? [])].sort((a, b) => b.prompt + b.completion - (a.prompt + a.completion)))
  const perDay = $derived(data?.perDay ?? [])
  // "~" when the window is dominated by estimates rather than reported usage.
  const approx = $derived((t?.estimatedShare ?? 0) > 0.5 ? '~' : '')
  // Every KIND of token, cache included, or the headline would quietly
  // understate exactly the spend cache multipliers change most.
  const total = (x?: CostTotals) => (x ? x.prompt + x.completion + (x.cache ?? 0) : 0)
  const inOut = (x?: CostTotals) => `${formatTokens(x?.prompt ?? 0)} in · ${formatTokens(x?.completion ?? 0)} out`

  // `data` is undefined on rejection, so a 500 on /api/cost must not render
  // "No usage recorded yet" over a ledger holding hundreds of usage_events.
  // That is a claim about SPEND made from a read that failed. Three answers,
  // kept apart: broke, still loading, genuinely zero.
  const hardFailure = $derived(costQuery.isError && data === undefined)
  // A failed BACKGROUND refetch (this query polls every 60s) keeps the last
  // good ledger on screen, but it must not pass stale totals off as current.
  const staleFailure = $derived(costQuery.isError && data !== undefined)

  // Where the 30-day dollars come from, said once under the tables.
  const notes = $derived.by(() => {
    if (!t) return []
    const lines: string[] = []
    if ((t.includedCloudTokens ?? 0) > 0) lines.push(`${formatTokens(t.includedCloudTokens!)} cloud tokens are included in a plan and priced at $0.`)
    if ((t.derivedCost ?? 0) > 0 && !t.providerReported) lines.push(`${formatCost(t.derivedCost!)} of the spend is estimated from published prices.`)
    if (t.estimatedShare > 0)
      lines.push(`About ${Math.round(t.estimatedShare * 100)}% of generations are character-based estimates, because the gateway reported no token usage for them.`)
    return lines
  })
</script>

<div class="space-y-8">
  {#if staleFailure}
    <div transition:slide={{ duration: 150 }}>
      <QueryError variant="inline" title="Usage may be out of date" error={costQuery.error} onRetry={() => void costQuery.refetch()} />
    </div>
  {/if}

  {#if hardFailure}
    <QueryError title="Could not load usage" error={costQuery.error} onRetry={() => void costQuery.refetch()} />
  {:else}
    <!-- Skeleton to content as one motion: the sketch is the stat strip and
         one table card, and the resolved readout staggers in over it.
         `data === undefined`, not `isLoading`, so a paused first fetch also
         reads as "we don't know yet". -->
    <Materialize loading={data === undefined} count={1} class="space-y-8">
      {#snippet skeleton()}
        <StatStripSkeleton />
        <div aria-hidden="true" class="rounded-lg border border-line bg-panel px-5 py-4"><SkeletonRows rows={5} avatar /></div>
      {/snippet}
      {#if total(t?.month) === 0}
        <div in:fade={{ duration: 150 }}>
          <EmptyState
            icon="⌗"
            title="No usage recorded yet"
            hint="Every chat turn and channel reply lands here once agents start talking."
          />
        </div>
      {:else}
        <StatStrip>
          <Stat
            label="Cloud spend, 30 days"
            value={formatCost(t?.month.cost ?? 0)}
            sub={t?.providerReported
              ? `${t.providerReported.source}${t.providerReported.stale ? ' (stale)' : ''}${t.providerReported.fetchedAt ? ` · ${t.providerReported.fetchedAt}` : ''}`
              : `${formatCost(t?.today.cost ?? 0)} today · ${(t?.month.generations ?? 0).toLocaleString('en-US')} generations`}
          />
          <Stat label="Tokens today" value={approx + formatTokens(total(t?.today))} sub={inOut(t?.today)} />
          <Stat label="Tokens, 7 days" value={approx + formatTokens(total(t?.week))} sub={inOut(t?.week)} />
          <Stat label="Tokens, 30 days" value={approx + formatTokens(total(t?.month))} sub={inOut(t?.month)} />
        </StatStrip>

        <!-- Missing pricing is attention (gold), not failure. Orange stays
             reserved for real failures. -->
        {#if isAdmin}
          <UnpricedPanel />
        {:else if (t?.unpricedCloudTokens ?? 0) > 0}
          <p transition:slide={{ duration: 150 }} class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
            <StatusDot status="warn" />
            {formatTokens(t!.unpricedCloudTokens)} cloud tokens have no price yet, so they are missing from the spend.
          </p>
        {/if}

        {#if t && t.split.local + t.split.cloud + t.split.other > 0}
          <!-- `data!`: this branch renders only when Materialize's `loading`
               (data === undefined) is false, which TS cannot see across props. -->
          <SplitPanel split={t.split} perModel={data!.perModel} />
        {/if}

        {#if perDay.length > 1}
          <DailyBars days={perDay} />
        {/if}

        <SettingsSection title="By agent" description="Tokens and spend per agent in the last 30 days.">
          <SettingsGroup>
            <div class="overflow-x-auto">
              <table class={table.root}>
                <thead>
                  <tr>
                    <th class={table.th}>Agent</th>
                    <th class={cn(table.th, table.num)}>Input</th>
                    <th class={cn(table.th, table.num)}>Output</th>
                    <th class={cn(table.th, table.num)}>Total</th>
                    <th class={cn(table.th, table.num)}>Self-hosted</th>
                    <th class={cn(table.th, table.num)}>Cost</th>
                    <th class={cn(table.th, table.num)}>Generations</th>
                    <th class={cn(table.th, table.num)}>Last used</th>
                  </tr>
                </thead>
                <tbody>
                  {#each perAgent as a (a.agentModel)}
                    {@const d = agentLabel(a.agentModel)}
                    <tr class={table.tr}>
                      <td class={table.td}>
                        <span class="flex min-w-0 items-center gap-2.5">
                          <Avatar name={d.label} class="h-6 w-6 shrink-0" />
                          <span class="font-medium">{d.label}</span>
                          {#if d.role}<span class="min-w-0 truncate text-[13px] text-muted">{d.role}</span>{/if}
                        </span>
                      </td>
                      <td class={cn(table.td, table.num, 'text-muted')}>{formatTokens(a.prompt)}</td>
                      <td class={cn(table.td, table.num, 'text-muted')}>{formatTokens(a.completion)}</td>
                      <td class={cn(table.td, table.num, 'font-medium')}>{formatTokens(a.prompt + a.completion)}</td>
                      <td class={cn(table.td, table.num, 'text-muted')}>
                        {a.localShare === null ? '—' : `${Math.round(a.localShare * 100)}%`}
                      </td>
                      <td
                        class={cn(table.td, table.num)}
                        title={(a.includedTokens ?? 0) > 0 ? `${formatTokens(a.includedTokens ?? 0)} tokens included in a plan` : undefined}
                      >
                        {agentCostLabel(a)}
                      </td>
                      <td class={cn(table.td, table.num, 'text-muted')}>{a.generations.toLocaleString('en-US')}</td>
                      <td class={cn(table.td, table.num, 'text-muted')}>{relativeTime(a.lastUsed)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </SettingsGroup>
          {#if notes.length > 0}
            <ul class="mt-3 space-y-1 font-sans text-[13px] leading-5 text-muted">
              {#each notes as line (line)}<li>{line}</li>{/each}
            </ul>
          {/if}
        </SettingsSection>
      {/if}
    </Materialize>
  {/if}
</div>
