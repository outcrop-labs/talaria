<script lang="ts">
  import { p } from '@/router'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { formatTokens } from '@/lib/cost.svelte'
  import { fade } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { useInference } from './inference'
  import LiveSection from './LiveSection.svelte'

  // The inference plane, live: what's generating right now, the gateway's pulse,
  // fleet container temperature — then the self-hosted backends underneath.
  const session = useSession()
  const query = useInference()
  const data = $derived(query.data)
  const isLoading = $derived(query.isLoading)
  const backends = $derived(data?.backends ?? [])
  const live = $derived(data?.live)
  const failed = $derived(query.isError && data === undefined)
</script>

<div>
  <div class="space-y-8">
    <div class="flex items-center gap-3">
      {#if session.data?.role === 'admin'}
        <a href={p('/models')} class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
          Configure on Models →
        </a>
      {/if}
    </div>

    <!-- "0 generating / no errors / all healthy" over a dead endpoint is the
         same lie as an empty list — a failed read gets its own panel. -->
    {#if failed}
      <QueryError error={query.error} title="Could not load the inference plane" onRetry={() => void query.refetch()} />
    {:else if live}
      <LiveSection {live} />
    {:else}
      <div class="grid grid-cols-4 gap-4">
        {#each [0, 1, 2, 3] as i (i)}
          <SkeletonCard delay={i * 0.1} />
        {/each}
      </div>
    {/if}

    <SectionHeader class="mb-0" title="Self-hosted compute" />

    {#if failed}
      <!-- The failure already took the surface above — say nothing twice. -->
    {:else if isLoading}
      <SkeletonCard />
    {:else if backends.length === 0}
      <div in:fade={{ duration: 150 }}>
        <EmptyState
          icon="▦"
          title="No self-hosted backends"
          hint="Add a self-hosted provider (Ollama, vLLM, a router) on the Models page. LAN and loopback URLs are classed self-hosted automatically."
        />
      </div>
    {:else}
      <div class="grid grid-cols-3 gap-4">
        <StatCard label="Self-hosted tokens · today" value={formatTokens(data?.usage.today ?? 0)} />
        <StatCard label="Self-hosted tokens · 30 days" value={formatTokens(data?.usage.month ?? 0)} />
        <StatCard label="Generations · 30 days" value={String(data?.usage.generations ?? 0)} />
      </div>

      {#each backends as b (b.id)}
        <Panel>
          <div class="mb-4 flex items-center gap-3">
            <span
              class="h-[7px] w-[7px] shrink-0 rounded-full"
              style:background={b.health.ok ? 'var(--theme-success)' : 'var(--theme-danger)'}
            ></span>
            <span class="font-sans text-sm font-semibold text-fg">{b.name}</span>
            <span class="min-w-0 truncate font-mono text-[11px] text-muted">{b.baseUrl}</span>
            <span class={`ml-auto shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] ${b.health.ok ? 'text-muted' : 'text-danger'}`}>
              {b.health.ok ? `up · ${b.health.latencyMs}ms` : 'unreachable'}
            </span>
          </div>
          {#if b.health.ok}
            <div class="space-y-2.5">
              <div>
                <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Serving now</div>
                <div class="flex flex-wrap gap-1.5">
                  {#each b.health.servingNow.length ? b.health.servingNow : ['(none reported)'] as m (m)}
                    <span class="rounded border border-line px-2.5 py-0.5 font-mono text-[11px] text-fg">
                      {m}
                    </span>
                  {/each}
                </div>
              </div>
            </div>
          {:else}
            <div class="font-sans text-xs text-muted">{b.health.note}</div>
          {/if}
        </Panel>
      {/each}

      {#if (data?.usage.perModel.length ?? 0) > 0}
        <Panel>
          <SectionHeader title="Served self-hosted · 30 days" action={String(data!.usage.perModel.length).padStart(2, '0')} />
          <div class="divide-y divide-line">
            {#each data!.usage.perModel as m (m.llmModel ?? '?')}
              <div class="flex items-center gap-3 py-3 text-sm transition-colors hover:bg-hover">
                <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{m.llmModel ?? 'unattributed'}</span>
                <span class="shrink-0 font-mono text-[11px] text-muted">{formatTokens(m.tokens)} tokens</span>
              </div>
            {/each}
          </div>
        </Panel>
      {/if}
    {/if}
  </div>
</div>
