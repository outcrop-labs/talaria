<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useHost, type HostMount } from './host'
  import Meter from './Meter.svelte'
  import StatStripSkeleton from './StatStripSkeleton.svelte'
  import { table } from './grammar'

  // The machine, not the fleet. Mounts arrive fullest-first so the one
  // filling is the first row, and the bar colour says how close it is.
  const query = useHost()
  const host = $derived(query.data)
  const failed = $derived(query.isError && host === undefined)
  const stale = $derived(query.isError && host !== undefined)

  const tone = (p: HostMount['pressure']) => (p === 'danger' ? 'text-danger' : p === 'warn' ? 'text-warning' : 'text-fg')
  // Disks are the scale `df` reports, so this goes up to TiB. The admin
  // storage formatter stops at GB and rounds differently; a host mount is
  // not a bucket object.
  const bytes = (n: number) => {
    if (n >= 2 ** 40) return `${(n / 2 ** 40).toFixed(1)} TiB`
    if (n >= 2 ** 30) return `${(n / 2 ** 30).toFixed(1)} GiB`
    if (n >= 2 ** 20) return `${(n / 2 ** 20).toFixed(n >= 10 * 2 ** 20 ? 0 : 1)} MiB`
    if (n >= 2 ** 10) return `${Math.round(n / 1024)} KiB`
    return `${n} B`
  }
  const pct = (n: number) => `${Math.round(n)}%`
</script>

<!-- A reading the first sample has not produced yet: CPU is a delta across two. -->
{#snippet unmeasured()}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/snippet}

{#snippet reading(text: string, percent: number, label: string)}
  {text}
  <Meter {percent} {label} class="mb-1.5 mt-2" />
{/snippet}

<div class="space-y-8">
  {#if stale}
    <div transition:slide={{ duration: 150 }}>
      <QueryError variant="inline" title="Host numbers may be out of date" error={query.error} onRetry={() => void query.refetch()} />
    </div>
  {/if}

  {#if failed}
    <QueryError title="Could not load host metrics" error={query.error} onRetry={() => void query.refetch()} />
  {:else if !host}
    <StatStripSkeleton />
  {:else if !host.available}
    <EmptyState variant="compact" icon="◇" title="Host metrics are not visible" hint={host.note ?? 'This process cannot see the machine it runs on.'} />
  {:else}
    {@const cpu = host.cpu}
    {@const load = host.load}
    {@const memory = host.memory}
    {@const swap = host.swap}
    <StatStrip>
      <Stat
        label="CPU"
        sub={cpu ? `${cpu.cores} cores${cpu.iowaitPercent >= 1 ? ` · iowait ${pct(cpu.iowaitPercent)}` : ''}` : undefined}
      >
        {#snippet value()}
          {#if cpu}{@render reading(pct(cpu.percent), cpu.percent, `CPU ${pct(cpu.percent)} busy`)}{:else}{@render unmeasured()}{/if}
        {/snippet}
      </Stat>
      <Stat
        label="Load, 1 min"
        sub={load
          ? `${cpu && load.one > cpu.cores ? `Above ${cpu.cores} cores · ` : ''}5 min ${load.five.toFixed(2)} · 15 min ${load.fifteen.toFixed(2)}`
          : undefined}
      >
        {#snippet value()}
          {#if load && cpu}
            {@render reading(load.one.toFixed(2), (load.one / cpu.cores) * 100, `Load ${load.one.toFixed(2)} on ${cpu.cores} cores`)}
          {:else if load}{load.one.toFixed(2)}{:else}{@render unmeasured()}{/if}
        {/snippet}
      </Stat>
      <Stat label="Memory" sub={memory ? `${bytes(memory.used)} of ${bytes(memory.total)}` : undefined}>
        {#snippet value()}
          {#if memory}{@render reading(pct(memory.percent), memory.percent, `Memory ${pct(memory.percent)} used`)}{:else}{@render unmeasured()}{/if}
        {/snippet}
      </Stat>
      <Stat
        label="Swap"
        sub={swap ? (swap.present ? `${bytes(swap.used)} of ${bytes(swap.total)}` : 'Not configured') : undefined}
      >
        {#snippet value()}
          {#if swap?.present}{@render reading(pct(swap.percent), swap.percent, `Swap ${pct(swap.percent)} used`)}
          {:else if swap}None{:else}{@render unmeasured()}{/if}
        {/snippet}
      </Stat>
    </StatStrip>

    <SettingsSection
      title="Disks"
      description="Fullest first, refreshed every 5 seconds."
      info="The bar is df's percent: 80% is a warning, 90% is critical. Bind mounts of the same disk are one row."
    >
      {#if host.note}
        <p class="mb-3 flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="warn" />{host.note}</p>
      {/if}
      {#if host.mounts.length === 0}
        <EmptyState variant="compact" title="No mounts could be read" hint="The process can see the host, but not a filesystem it can measure." />
      {:else}
        <SettingsGroup>
          <div class="overflow-x-auto">
            <table class={table.root}>
              <thead>
                <tr>
                  <th class={table.th}>Mount</th>
                  <th class={table.th}>Device</th>
                  <th class={cn(table.th, table.num)}>Used</th>
                  <th class={cn(table.th, table.num)}>Free</th>
                  <th class={cn(table.th, 'w-[32%]')}>Full</th>
                </tr>
              </thead>
              <tbody>
                {#each host.mounts as m (m.mount)}
                  <tr class={table.tr}>
                    <td class={cn(table.td, table.id)}>{m.mount}</td>
                    <td class={cn(table.td, table.id, 'text-muted')}>{m.source} · {m.fstype}</td>
                    <td class={cn(table.td, table.num)}>{bytes(m.used)} of {bytes(m.total)}</td>
                    <td class={cn(table.td, table.num, 'text-muted')}>{bytes(m.available)}</td>
                    <td class={table.td}>
                      <div class="flex items-center gap-3">
                        <Meter percent={m.percent} pressure={m.pressure} label="{m.mount} {pct(m.percent)} full" class="min-w-16 flex-1" />
                        <span class={cn('w-9 shrink-0 text-right font-medium tabular-nums', tone(m.pressure))}>{pct(m.percent)}</span>
                      </div>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </SettingsGroup>
      {/if}
    </SettingsSection>

    <SettingsSection
      title="Processes"
      description="The top consumers on the host, by CPU then memory."
      info="Names are the kernel's comm, not the command line."
    >
      {#if host.processes.length === 0}
        <EmptyState variant="compact" title="No userspace processes sampled" />
      {:else}
        <SettingsGroup>
          <div class="overflow-x-auto">
            <table class={table.root}>
              <thead>
                <tr>
                  <th class={cn(table.th, 'w-24')}>PID</th>
                  <th class={table.th}>Process</th>
                  <th class={cn(table.th, table.num)}>CPU</th>
                  <th class={cn(table.th, table.num, 'w-32')}>Memory</th>
                </tr>
              </thead>
              <tbody>
                {#each host.processes as p (p.pid)}
                  <tr class={table.tr}>
                    <td class={cn(table.td, table.id, 'text-muted')}>{p.pid}</td>
                    <td class={cn(table.td, table.id)}>{p.name}</td>
                    <td class={cn(table.td, table.num)}>{p.cpuPercent.toFixed(1)}%</td>
                    <td class={cn(table.td, table.num)}>{bytes(p.rss)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </SettingsGroup>
      {/if}
    </SettingsSection>
  {/if}
</div>
