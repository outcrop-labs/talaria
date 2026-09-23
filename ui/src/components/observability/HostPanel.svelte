<script lang="ts">
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useHost, type HostMount } from './host'

  // The machine, not the fleet. Mounts arrive fullest-first so the one
  // filling is the first row, and the bar colour says how close it is.
  const query = useHost()
  const host = $derived(query.data)
  const failed = $derived(query.isError && host === undefined)
  const stale = $derived(query.isError && host !== undefined)

  const bar = (p: HostMount['pressure']) =>
    p === 'danger' ? 'bg-danger' : p === 'warn' ? 'bg-warning' : 'bg-accent'
  const tone = (p: HostMount['pressure']) =>
    p === 'danger' ? 'text-danger' : p === 'warn' ? 'text-warning' : 'text-fg'
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

<div class="space-y-8">
  {#if stale}
    <div transition:slide={{ duration: 150 }}>
      <QueryError variant="inline" title="Host numbers may be out of date" error={query.error} onRetry={() => void query.refetch()} />
    </div>
  {/if}

  {#if failed}
    <QueryError title="Could not load host metrics" error={query.error} onRetry={() => void query.refetch()} />
  {:else if !host}
    <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
      {#each [0, 1, 2, 3] as i (i)}
        <SkeletonCard />
      {/each}
    </div>
  {:else if !host.available}
    <EmptyState variant="compact" icon="◇" title="Host metrics are not visible" hint={host.note ?? 'This process cannot see the machine it runs on.'} />
  {:else}
    <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
      <StatCard
        label="CPU"
        sub={host.cpu
          ? `${host.cpu.cores} cores${host.cpu.iowaitPercent >= 1 ? ` · iowait ${pct(host.cpu.iowaitPercent)}` : ''}`
          : undefined}
      >
        {#snippet value()}
          {#if host.cpu}{pct(host.cpu.percent)}{:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
        {/snippet}
      </StatCard>
      <StatCard
        label="Load"
        sub={host.load && host.cpu
          ? `${host.load.one > host.cpu.cores ? 'above ' : ''}${host.cpu.cores} cores · 5m ${host.load.five.toFixed(2)} · 15m ${host.load.fifteen.toFixed(2)}`
          : undefined}
      >
        {#snippet value()}
          {#if host.load}{host.load.one.toFixed(2)}{:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
        {/snippet}
      </StatCard>
      <StatCard
        label="Memory"
        sub={host.memory ? `${bytes(host.memory.used)} of ${bytes(host.memory.total)}` : undefined}
      >
        {#snippet value()}
          {#if host.memory}{pct(host.memory.percent)}{:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
        {/snippet}
      </StatCard>
      <StatCard
        label="Swap"
        sub={host.swap ? (host.swap.present ? `${bytes(host.swap.used)} of ${bytes(host.swap.total)}` : 'not configured') : undefined}
      >
        {#snippet value()}
          {#if host.swap}{host.swap.present ? pct(host.swap.percent) : 'none'}{:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
        {/snippet}
      </StatCard>
    </div>

    <section>
      <SectionHeader
        title="Disk"
        info="Fullest first. The bar is df's percent: 80% is a warning, 90% is critical. Bind mounts of the same disk are one row."
        action="5s"
      />
      {#if host.note}
        <p class="mb-3 font-sans text-xs text-warning">{host.note}</p>
      {/if}
      {#if host.mounts.length === 0}
        <EmptyState variant="compact" title="No mounts could be read" hint="The process can see the host, but not a filesystem it can measure." />
      {:else}
        <Panel class="space-y-4">
          {#each host.mounts as m (m.mount)}
            <div>
              <div class="mb-1.5 flex items-baseline gap-3">
                <span class="min-w-0 flex-1 truncate font-mono text-sm text-fg">{m.mount}</span>
                <span class="hidden min-w-0 truncate font-mono text-[11px] text-muted sm:block">{m.source} · {m.fstype}</span>
                <span class={cn('w-12 shrink-0 text-right font-mono text-sm', tone(m.pressure))}>{pct(m.percent)}</span>
              </div>
              <div
                class="h-1.5 overflow-hidden rounded-full bg-line"
                role="meter"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(m.percent)}
                aria-label="{m.mount} {pct(m.percent)} full"
              >
                <div class={cn('h-full rounded-full', bar(m.pressure))} style:width="{Math.min(100, m.percent)}%"></div>
              </div>
              <div class="mt-1 font-mono text-[11px] text-muted">{bytes(m.used)} of {bytes(m.total)} · {bytes(m.available)} free</div>
            </div>
          {/each}
        </Panel>
      {/if}
    </section>

    <section>
      <SectionHeader
        title="Processes"
        info="Top consumers on the host, by CPU then memory. Names are the kernel's comm, not the command line."
      />
      {#if host.processes.length === 0}
        <EmptyState variant="compact" title="No userspace processes sampled" />
      {:else}
        <Panel class="space-y-2">
          {#each host.processes as p (p.pid)}
            <div class="flex items-center gap-3">
              <span class="w-14 shrink-0 font-mono text-[11px] text-muted">{p.pid}</span>
              <span class="min-w-0 flex-1 truncate font-mono text-sm text-fg">{p.name}</span>
              <span class="w-14 shrink-0 text-right font-mono text-[11px] text-muted">{p.cpuPercent.toFixed(1)}%</span>
              <span class="w-20 shrink-0 text-right font-mono text-[11px] text-muted">{bytes(p.rss)}</span>
            </div>
          {/each}
        </Panel>
      {/if}
    </section>
  {/if}
</div>
