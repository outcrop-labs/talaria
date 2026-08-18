<script lang="ts">
  import { p } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { fade } from '@/lib/motion'
  import ActivityList from './ActivityList.svelte'
  import FleetPulse from './FleetPulse.svelte'
  import type { HomeQuery } from './home'

  // Fleet (admin): status, alerts, spend, and the org pulse.
  let { home }: { home: HomeQuery } = $props()

  // Resolved-with-nothing is impossible here (a 200 always carries the
  // summary), so an absent payload is either in flight or broken — and those
  // two must not look alike.
</script>

{#if !home.data && home.isError}
  <div in:fade={{ duration: 150 }}>
    <QueryError error={home.error} title="Could not load fleet status" onRetry={() => void home.refetch()} />
  </div>
{:else if !home.data}
  <div class="grid gap-6 lg:grid-cols-2">
    <div class="space-y-4">
      <Panel>
        <SkeletonRows rows={2} />
      </Panel>
      <div class="grid grid-cols-2 gap-4">
        {#each [0, 1] as i (i)}
          <Panel>
            <Skeleton class="mb-3 h-2.5 w-14 rounded-full" />
            <Skeleton class="h-5 w-16 rounded-full" />
          </Panel>
        {/each}
      </div>
    </div>
    <Panel>
      <Skeleton class="mb-4 h-3 w-12 rounded-full" />
      <SkeletonRows rows={8} />
    </Panel>
  </div>
{:else}
  {@const org = home.data.org}
  {@const fleet = home.data.fleet}
  <div class="grid gap-6 lg:grid-cols-2">
    <div class="space-y-4">
      <div class="px-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{org.name || 'Your org'}</div>
      <Panel>
        <div class="flex items-center gap-3">
          <StatusDot status={fleet.down.length ? 'warn' : 'ok'} />
          <span class="font-sans text-sm font-semibold text-fg">Fleet</span>
          <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">
            {fleet.online}/{fleet.total} online{fleet.down.length > 0 ? ` · ${fleet.down.slice(0, 2).join(', ')} down` : ''}
          </span>
          <a
            href={p('/agents')}
            class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-accent hover:underline"
          >
            Manage →
          </a>
        </div>
      </Panel>
      <div class="grid grid-cols-2 gap-3">
        <a href={`${p('/observability')}/alerts`} class="block">
          <Panel class="p-4">
            <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Alerts</div>
            <div
              class={cn(
                'mt-1 font-sans text-2xl font-semibold',
                (org.alerts ?? 0) > 0 ? 'text-warning' : 'text-success',
              )}
            >
              {org.alerts ?? 0}
            </div>
          </Panel>
        </a>
        <a href={`${p('/observability')}/cost`} class="block">
          <Panel class="p-4">
            <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Spend today</div>
            <div class="mt-1 truncate font-sans text-2xl font-semibold text-fg">
              {org.costToday ? `$${org.costToday.usd.toFixed(2)}` : '—'}
            </div>
            {#if org.costToday}
              <div class="font-mono text-[10px] tracking-[0.05em] text-muted">{(org.costToday.tokens / 1000).toFixed(0)}k tokens</div>
            {/if}
          </Panel>
        </a>
      </div>
      <ActivityList kinds={['fleet']} title="Fleet activity" />
    </div>
    <FleetPulse activity={org.activity} />
  </div>
{/if}
