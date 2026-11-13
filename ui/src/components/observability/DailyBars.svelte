<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { formatTokens, type CostTotals } from '@/lib/cost.svelte'
  import { sectionLink } from './grammar'
  import { SERIES, SERVED_ORDER, type ServedBy } from './series'

  /** Tokens per day, stacked by where they were served: self-hosted at the
   *  baseline, cloud above it, anything unattributed on top. The title and
   *  the axis come from the days it is given, whatever their number. */
  let {
    days,
    href,
  }: {
    days: Array<CostTotals & { day: string; local: number; cloud: number }>
    /** The chart is a summary on another tab: a shorter plot, and a link to
     *  the Cost tab that owns it. */
    href?: string
  } = $props()
  const plot = $derived(href ? 'h-24' : 'h-40')

  // `local` and `cloud` count cache tokens and `prompt + completion` does not,
  // so a day's height is whichever reading is larger. The stack never
  // overshoots the scale and the remainder is never negative.
  const rows = $derived(
    days.map((d) => {
      const total = Math.max(d.prompt + d.completion + (d.cache ?? 0), d.local + d.cloud)
      return { day: d.day, generations: d.generations, total, local: d.local, cloud: d.cloud, other: total - d.local - d.cloud }
    }),
  )
  const sum = (k: ServedBy | 'total') => rows.reduce((n, r) => n + r[k], 0)
  const shown = $derived(SERVED_ORDER.filter((k) => sum(k) > 0))

  // The scale tops out on a round number whose half is round too.
  const STEPS = [1, 1.5, 2, 3, 4, 5, 6, 8, 10]
  const top = $derived.by(() => {
    const peak = Math.max(...rows.map((r) => r.total), 10)
    const magnitude = 10 ** Math.floor(Math.log10(peak))
    return STEPS.find((s) => s * magnitude >= peak)! * magnitude
  })
  const height = (n: number) => `${(n / top) * 100}%`

  const date = (day: string, weekday = false) =>
    new Date(`${day}T00:00:00`).toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      ...(weekday ? { weekday: 'short' as const } : {}),
    })
  // Every day gets a date when they fit, otherwise five evenly spaced ones
  // that always include the first and the last.
  const labelled = $derived.by(() => {
    const last = rows.length - 1
    if (last < 8) return new Set(rows.map((_, i) => i))
    return new Set([0, 1, 2, 3, 4].map((i) => Math.round((i * last) / 4)))
  })

  let hovered = $state<number | null>(null)
  const focus = $derived(hovered === null ? null : (rows[hovered] ?? null))
  const range = $derived(rows.length ? `${date(rows[0]!.day)} to ${date(rows[rows.length - 1]!.day)}` : '')
  const describe = (r: (typeof rows)[number]) =>
    `${date(r.day, true)}: ${formatTokens(r.total)} tokens, ${shown.map((k) => `${formatTokens(r[k])} ${SERIES[k].label.toLowerCase()}`).join(', ')}, ${r.generations.toLocaleString('en-US')} generations`
</script>

<SettingsSection title="Tokens per day" description={`${range}, stacked by where the tokens were served.`}>
  {#snippet action()}
    {#if href}<a {href} class={sectionLink}>Cost details<ChevronRight size={14} /></a>{/if}
  {/snippet}
  <SettingsGroup>
    <figure class="px-5 pb-4 pt-4">
      <figcaption class="flex flex-wrap items-center gap-x-5 gap-y-1 font-sans text-[13px] leading-5">
        <span class="mr-auto font-medium text-fg">
          {focus ? date(focus.day, true) : `${rows.length} days`}
          <span class="font-normal tabular-nums text-muted">· {formatTokens(focus ? focus.total : sum('total'))} tokens</span>
        </span>
        {#each shown as k (k)}
          <span class="flex items-center gap-2 text-muted">
            <StatusDot color={SERIES[k].color} class="h-2 w-2" />
            {SERIES[k].label}
            <span class="tabular-nums text-fg">{formatTokens(focus ? focus[k] : sum(k))}</span>
          </span>
        {/each}
      </figcaption>

      <div class="mt-5 flex gap-3">
        <div class={cn('relative w-9 shrink-0 font-sans text-xs tabular-nums leading-4 text-muted', plot)} aria-hidden="true">
          {#each [top, top / 2, 0] as tick, i (i)}
            <span class="absolute right-0 -translate-y-1/2" style:top="{i * 50}%">{formatTokens(tick)}</span>
          {/each}
        </div>
        <div class="min-w-0 flex-1">
          <div
            class={cn('relative border-b border-line', plot)}
            role="img"
            aria-label={`Tokens per day, ${range}. ${formatTokens(sum('total'))} tokens in all.`}
            onpointerleave={() => (hovered = null)}
          >
            <div class="absolute inset-x-0 top-0 border-t border-line-subtle"></div>
            <div class="absolute inset-x-0 top-1/2 border-t border-line-subtle"></div>
            <div class="absolute inset-0 flex items-stretch">
              {#each rows as r, i (r.day)}
                <div
                  role="presentation"
                  class={cn('flex min-w-0 flex-1 flex-col items-center justify-end rounded-t-sm transition-colors', hovered === i && 'bg-hover')}
                  title={describe(r)}
                  onpointerenter={() => (hovered = i)}
                >
                  {#each [...shown].reverse().filter((k) => r[k] > 0) as k, j (k)}
                    <!-- The 2px line in the surface colour is the gap between
                         segments. It sits inside the height, so the stack
                         still ends where the scale says. -->
                    <div
                      class={cn('w-[62%] max-w-6', j === 0 ? 'rounded-t-[3px]' : 'border-t-2 border-panel')}
                      style:height={height(r[k])}
                      style:background={SERIES[k].color}
                    ></div>
                  {/each}
                </div>
              {/each}
            </div>
          </div>
          <div class="mt-2 flex h-4 font-sans text-xs leading-4 text-muted" aria-hidden="true">
            {#each rows as r, i (r.day)}
              <div class="relative min-w-0 flex-1">
                {#if labelled.has(i)}
                  <span
                    class={cn(
                      'absolute whitespace-nowrap',
                      i === 0 ? 'left-0' : i === rows.length - 1 ? 'right-0' : 'left-1/2 -translate-x-1/2',
                    )}
                  >
                    {date(r.day)}
                  </span>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      </div>
    </figure>
  </SettingsGroup>
</SettingsSection>
