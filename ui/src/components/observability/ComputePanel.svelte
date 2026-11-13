<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import { p } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { formatTokens } from '@/lib/cost.svelte'
  import { fade } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { useInference } from './inference'
  import LiveSection from './LiveSection.svelte'
  import ShareBar from './ShareBar.svelte'
  import StatStripSkeleton from './StatStripSkeleton.svelte'
  import { sectionLink, table } from './grammar'

  // The inference plane, live: what's generating right now, the gateway's pulse,
  // fleet container temperature, then the self-hosted backends underneath.
  const session = useSession()
  const query = useInference()
  const data = $derived(query.data)
  const isLoading = $derived(query.isLoading)
  const backends = $derived(data?.backends ?? [])
  const live = $derived(data?.live)
  const failed = $derived(query.isError && data === undefined)
  const perModel = $derived(data?.usage.perModel ?? [])
  const servedTokens = $derived(perModel.reduce((n, m) => n + m.tokens, 0))
</script>

<div class="space-y-10">
  <!-- "0 generating / no errors / all healthy" over a dead endpoint is the
       same lie as an empty list, so a failed read gets its own panel. -->
  {#if failed}
    <QueryError error={query.error} title="Could not load the inference plane" onRetry={() => void query.refetch()} />
  {:else if live}
    <LiveSection {live} />
  {:else}
    <StatStripSkeleton />
  {/if}

  <!-- The failure already took the surface above, so it says nothing twice. -->
  {#if !failed}
    <SettingsSection
      title="Self-hosted backends"
      description={data && backends.length > 0
        ? `${formatTokens(data.usage.today)} tokens today, ${formatTokens(data.usage.month)} in the last 30 days, across ${data.usage.generations.toLocaleString('en-US')} generations.`
        : undefined}
    >
      {#snippet action()}
        {#if session.data?.role === 'admin'}
          <a href={p('/models')} class={sectionLink}>Configure in Models<ChevronRight size={14} /></a>
        {/if}
      {/snippet}
      {#if isLoading}
        <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={2} /></div></SettingsGroup>
      {:else if backends.length === 0}
        <div in:fade={{ duration: 150 }}>
          <EmptyState
            icon="▦"
            title="No self-hosted backends"
            hint="Add a self-hosted provider (Ollama, vLLM, a router) in Models. LAN and loopback URLs count as self-hosted."
          />
        </div>
      {:else}
        <SettingsGroup>
          {#each backends as b (b.id)}
            <SettingsRow label={b.name}>
              {#snippet meta()}
                {#if b.baseUrl}<span class="min-w-0 truncate font-mono text-xs text-muted">{b.baseUrl}</span>{/if}
              {/snippet}
              {#snippet hint()}
                {#if !b.health.ok}
                  {b.health.note}
                {:else if b.health.servingNow.length}
                  Serving <span class="font-mono text-xs">{b.health.servingNow.join(', ')}</span>
                {:else}
                  No model reported as loaded
                {/if}
              {/snippet}
              <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
                <StatusDot status={b.health.ok ? 'ok' : 'danger'} />
                {#if b.health.ok}
                  Up{b.health.latencyMs != null ? ` · ${b.health.latencyMs} ms` : ''}
                {:else}
                  Unreachable
                {/if}
              </span>
            </SettingsRow>
          {/each}
        </SettingsGroup>
      {/if}
    </SettingsSection>

    {#if backends.length > 0 && perModel.length > 0}
      <SettingsSection title="Self-hosted models" description="Tokens each model served on your own hardware in the last 30 days.">
        <SettingsGroup>
          <div class="overflow-x-auto">
            <table class={table.root}>
              <thead>
                <tr>
                  <th class={table.th}>Model</th>
                  <th class={cn(table.th, 'w-[45%]')}>Share of tokens</th>
                  <th class={cn(table.th, table.num)}>Tokens</th>
                </tr>
              </thead>
              <tbody>
                {#each perModel as m (m.llmModel ?? '?')}
                  <tr class={table.tr}>
                    <td class={cn(table.td, m.llmModel ? table.id : 'text-muted')}>{m.llmModel ?? 'Unattributed'}</td>
                    <td class={table.td}><ShareBar share={servedTokens ? m.tokens / servedTokens : 0} /></td>
                    <td class={cn(table.td, table.num)}>{formatTokens(m.tokens)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </SettingsGroup>
      </SettingsSection>
    {/if}
  {/if}
</div>
