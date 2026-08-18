<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { listStagger } from '@/lib/motion'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { getList } from '@/lib/fetch-json'

  type Severity = 'critical' | 'warning' | 'info'

  interface Alert {
    severity: Severity
    title: string
    detail: string
    href: string
  }

  const SEV: Record<Severity, { label: string; color: string; icon: string }> = {
    critical: { label: 'Critical', color: 'var(--theme-danger)', icon: '▲' },
    warning: { label: 'Warning', color: 'var(--theme-warning)', icon: '◆' },
    info: { label: 'Info', color: 'var(--theme-accent)', icon: '●' },
  }

  // Derived health signals — computed live from container state, the gateway,
  // the token ledger, and stuck tickets. Nothing to configure, nothing stored.
  const query = createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
</script>

<div>
  <div class="space-y-8">
    <!-- "All clear" is the single most dangerous thing this app can say
         wrongly — it is the sentence an operator uses to decide NOT to look.
         It may only render off a 200. -->
    <QueryState {query} errorTitle="Could not check for alerts">
      {#snippet skeleton()}
        <SkeletonRows rows={4} avatar />
      {/snippet}
      {#snippet empty()}
        <EmptyState
          icon="△"
          title="All clear"
          hint="Agents running, gateway answering, ledger priced, no stuck work."
        />
      {/snippet}
      {#snippet children(alerts)}
        <Panel class="p-0">
          <div class="divide-y divide-line" use:listStagger>
            {#each alerts as a, i (i)}
              <button data-dither-fill
                type="button"
                onclick={() => void navigateHref(a.href)}
                class="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors"
              >
                <span class="mt-0.5 w-5 shrink-0 text-center font-mono text-xs" style:color={SEV[a.severity].color}>
                  {SEV[a.severity].icon}
                </span>
                <div class="min-w-0 flex-1">
                  <div class="flex items-baseline gap-2">
                    <span class="truncate font-sans text-sm font-medium text-fg">{a.title}</span>
                    <span
                      class="shrink-0 rounded border border-line px-1 font-mono text-[10px] uppercase tracking-[0.05em]"
                      style:color={SEV[a.severity].color}
                    >
                      {SEV[a.severity].label}
                    </span>
                  </div>
                  <div class="font-sans text-sm text-muted">{a.detail}</div>
                </div>
              </button>
            {/each}
          </div>
        </Panel>
      {/snippet}
    </QueryState>
  </div>
</div>
