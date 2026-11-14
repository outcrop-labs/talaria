<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { agentLabel } from '@/lib/cost.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'
  import { toastError } from '@/lib/toast.svelte'
  import {
    OUTCOMES,
    OUTCOME_META,
    RETENTION_URL,
    WINDOWS,
    errorRate,
    formatLatency,
    isOutcome,
    isWindow,
    retentionLabel,
    retentionOptions,
    serverCalls,
    useMcpCalls,
    useMcpCallsRetention,
    useMcpCallsSummary,
    type McpCallCounts,
    type McpCallsWindow,
  } from './mcp-calls'
  import StatStripSkeleton from './StatStripSkeleton.svelte'
  import { table } from './grammar'

  // The MCP gateway's call record: every tools/call an agent made through the
  // gateway, and every refusal, with who, which server and tool, how it went
  // and how long it took. Never the arguments or the result. THE URL IS THE
  // FILTER (?server= ?agent= ?outcome= ?w=), so a server card's line can link
  // straight to its own calls and a filtered view can be shared.
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')

  const param = (k: string): string | null => {
    const v = searchParams.get(k)
    return v ? String(v) : null
  }
  const server = $derived(param('server'))
  const agent = $derived(param('agent'))
  const outcomeRaw = $derived(param('outcome'))
  const outcome = $derived(isOutcome(outcomeRaw) ? outcomeRaw : null)
  const windowRaw = $derived(param('w'))
  const win = $derived<McpCallsWindow>(isWindow(windowRaw) ? windowRaw : '24h')
  const setParam = (k: string, v: string | null) => {
    if (v) searchParams.set(k, v)
    else searchParams.delete(k)
  }
  const filtered = $derived(!!(server || agent || outcome))

  const summaryQuery = useMcpCallsSummary(() => win, () => isAdmin)
  const callsQuery = useMcpCalls(() => ({ server, agent, outcome, window: win }), () => isAdmin)

  // The strip speaks for the server filter when there is one (the summary is
  // per server); the agent and outcome filters narrow the table only.
  const selected = $derived(server ? serverCalls(summaryQuery.data, server) : null)
  const strip = $derived.by((): McpCallCounts | null => {
    const s = summaryQuery.data
    if (!s) return null
    if (!server) return s.totals
    return selected ?? { calls: 0, errors: 0, denied: 0, notConnected: 0 }
  })
  const serverOptions = $derived.by(() => {
    const names = (summaryQuery.data?.servers ?? []).map((s) => s.serverName)
    return server && !names.includes(server) ? [server, ...names] : names
  })
  const agentOptions = $derived.by(() => {
    const names = (summaryQuery.data?.agents ?? []).map((a) => a.agent)
    return agent && !names.includes(agent) ? [agent, ...names] : names
  })
  const WINDOW_NAMES: Record<McpCallsWindow, string> = { '1h': 'last hour', '24h': 'last 24 hours', '7d': 'last 7 days' }
  const windowLabel = $derived(WINDOW_NAMES[win])
  const fullTime = (iso: string) => new Date(iso).toLocaleString()
  const number = (n: number) => n.toLocaleString('en-US')

  // How long rows are kept (app setting mcp_calls_retention_days). Applies
  // on change; the next six-hourly prune enforces it.
  const qc = useQueryClient()
  const retentionQuery = useMcpCallsRetention(() => isAdmin)
  let savingRetention = $state(false)
  // The one field on this tab, and only an admin has it.
  const autosave = useAutosave(() => Boolean(retentionQuery.data))
  const setRetention = async (days: number) => {
    savingRetention = true
    try {
      await putJson(RETENTION_URL, { days })
      autosave.flash()
      await qc.invalidateQueries({ queryKey: ['mcp-calls-retention'] })
    } catch (e) {
      toastError('Could not change how long tool calls are kept', e)
    } finally {
      savingRetention = false
    }
  }

  // A cell that filters the page to its own value.
  const filterLink =
    'rounded text-left transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent'
</script>

{#if !isAdmin}
  <EmptyState
    icon="⛨"
    title="Admins only"
    hint="The tool call record names which agent called which tool on which server. An admin reads it here."
  />
{:else}
  <div class="space-y-8">
    <div class="space-y-4">
      <!-- The window, then the three narrowing selects, all one height. -->
      <div class="flex flex-wrap items-center gap-2">
        <Segmented
          size="md"
          options={WINDOWS.map((w) => ({ id: w.id, label: w.label, title: `Calls in the ${WINDOW_NAMES[w.id]}` }))}
          value={win}
          onChange={(w) => setParam('w', w === '24h' ? null : w)}
        />
        <Select size="sm" class="w-48" aria-label="Server" value={server ?? ''} onchange={(e) => setParam('server', e.currentTarget.value || null)}>
          <option value="">All servers</option>
          {#each serverOptions as name (name)}<option value={name}>{name}</option>{/each}
        </Select>
        <Select size="sm" class="w-44" aria-label="Agent" value={agent ?? ''} onchange={(e) => setParam('agent', e.currentTarget.value || null)}>
          <option value="">All agents</option>
          {#each agentOptions as name (name)}<option value={name}>{agentLabel(name).label}</option>{/each}
        </Select>
        <Select size="sm" class="w-44" aria-label="Outcome" value={outcome ?? ''} onchange={(e) => setParam('outcome', e.currentTarget.value || null)}>
          <option value="">All outcomes</option>
          {#each OUTCOMES as o (o)}<option value={o} title={OUTCOME_META[o].blurb}>{OUTCOME_META[o].label}</option>{/each}
        </Select>
        {#if filtered}
          <Button
            variant="ghost"
            size="md"
            onclick={() => {
              setParam('server', null)
              setParam('agent', null)
              setParam('outcome', null)
            }}
          >
            Clear filters
          </Button>
        {/if}
      </div>

      <!-- The window's counts, or one server's when the page is filtered to it. -->
      <QueryState query={summaryQuery} errorTitle="Could not load the tool call summary" errorVariant="compact">
        {#snippet skeleton()}
          <StatStripSkeleton />
        {/snippet}
        {#snippet children()}
          <StatStrip>
            <Stat label={server ? `Calls to ${server}` : 'Calls'} value={number(strip?.calls ?? 0)} sub={`In the ${windowLabel}`} />
            <Stat label="Errors" value={number(strip?.errors ?? 0)} sub={strip ? `${errorRate(strip)} of calls` : undefined} />
            <Stat label="Denied" value={number(strip?.denied ?? 0)} sub="Refused by the gateway" />
            <Stat label="Not connected" value={number(strip?.notConnected ?? 0)} sub="Granted, no account linked" />
          </StatStrip>
        {/snippet}
      </QueryState>
    </div>

    {#if !server && (summaryQuery.data?.servers.length ?? 0) > 0}
      <SettingsSection title="By server" description={`How each server answered in the ${windowLabel}; pick one to see its calls.`}>
        <SettingsGroup>
          <div class="overflow-x-auto">
            <table class={table.root}>
              <thead>
                <tr>
                  <th class={table.th}>Server</th>
                  <th class={cn(table.th, table.num)}>Calls</th>
                  <th class={cn(table.th, table.num)}>Errors</th>
                  <th class={cn(table.th, table.num)}>Denied</th>
                  <th class={cn(table.th, table.num)}>Not connected</th>
                  <th class={cn(table.th, table.num)}>Median</th>
                  <th class={cn(table.th, table.num)}>p95</th>
                </tr>
              </thead>
              <tbody>
                {#each summaryQuery.data!.servers as s (s.serverId)}
                  <tr class={table.tr}>
                    <td class={cn(table.td, 'whitespace-nowrap')}>
                      <button type="button" class={cn(filterLink, 'font-medium')} title="Show this server's calls" onclick={() => setParam('server', s.serverName)}>
                        {s.serverName}
                      </button>
                    </td>
                    <td class={cn(table.td, table.num)}>{number(s.calls)}</td>
                    <td class={cn(table.td, table.num, s.errors > 0 ? 'font-medium text-danger' : 'text-muted')}>{number(s.errors)}</td>
                    <td class={cn(table.td, table.num, s.denied > 0 ? 'font-medium text-warning' : 'text-muted')}>{number(s.denied)}</td>
                    <td class={cn(table.td, table.num, s.notConnected > 0 ? 'text-fg' : 'text-muted')}>{number(s.notConnected)}</td>
                    <td class={cn(table.td, table.num, 'text-muted')}>{formatLatency(s.p50Ms)}</td>
                    <td class={cn(table.td, table.num, 'text-muted')}>{formatLatency(s.p95Ms)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </SettingsGroup>
      </SettingsSection>
    {/if}

    {#if selected && selected.topTools.length > 0}
      <SettingsSection title="Most called tools" description={`What agents asked ${selected.serverName} for in the ${windowLabel}.`}>
        <SettingsGroup>
          <div class="overflow-x-auto">
            <table class={table.root}>
              <thead>
                <tr>
                  <th class={table.th}>Tool</th>
                  <th class={cn(table.th, table.num)}>Calls</th>
                  <th class={cn(table.th, table.num)}>Errors</th>
                </tr>
              </thead>
              <tbody>
                {#each selected.topTools as t (t.tool)}
                  <tr class={table.tr}>
                    <td class={cn(table.td, table.id)}>{t.tool}</td>
                    <td class={cn(table.td, table.num)}>{number(t.calls)}</td>
                    <td class={cn(table.td, table.num, t.errors > 0 ? 'font-medium text-danger' : 'text-muted')}>{number(t.errors)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </SettingsGroup>
      </SettingsSection>
    {/if}

    <SettingsSection
      title="Calls"
      description={callsQuery.data?.length ? `${number(callsQuery.data.length)} in the ${windowLabel}, newest first.` : undefined}
      info="Newest first, up to 200. A row never carries the call's arguments or its result."
    >
      <QueryState query={callsQuery} errorTitle="Could not load tool calls">
        {#snippet skeleton()}
          <div aria-hidden="true" class="rounded-lg border border-line bg-panel px-5 py-4"><SkeletonRows rows={6} /></div>
        {/snippet}
        {#snippet empty()}
          <EmptyState
            icon="⇄"
            title={filtered ? 'No calls match' : 'No tool calls yet'}
            hint={filtered
              ? `Nothing in the ${windowLabel} matches these filters.`
              : `Every call an agent makes to a tool server in the ${windowLabel} lands here. Arguments and results are never kept.`}
          />
        {/snippet}
        {#snippet children(calls)}
          <SettingsGroup class={cn('transition-opacity', callsQuery.isPlaceholderData && 'opacity-60')}>
            <div class="overflow-x-auto">
              <table class={table.root}>
                <thead>
                  <tr>
                    <th class={table.th}>Time</th>
                    <th class={table.th}>Agent</th>
                    {#if !server}<th class={table.th}>Server</th>{/if}
                    <th class={table.th}>Tool</th>
                    <th class={table.th}>Outcome</th>
                    <th class={cn(table.th, table.num)}>Latency</th>
                    <th class={cn(table.th, 'pl-6')}>Error</th>
                  </tr>
                </thead>
                <tbody>
                  {#each calls as c (c.id)}
                    {@const meta = OUTCOME_META[c.outcome]}
                    <tr class={table.tr}>
                      <td class={cn(table.td, 'whitespace-nowrap text-[13px] tabular-nums text-muted')} title={fullTime(c.at)}>{relativeTime(c.at)}</td>
                      <td class={cn(table.td, 'whitespace-nowrap')}>
                        <button type="button" class={cn(filterLink, 'font-medium')} title="Show this agent's calls" onclick={() => setParam('agent', c.agent)}>
                          {agentLabel(c.agent).label}
                        </button>
                        {#if c.userName}<span class="ml-1.5 text-[13px] text-muted">for {c.userName}</span>{/if}
                      </td>
                      {#if !server}
                        <td class={cn(table.td, 'whitespace-nowrap')}>
                          <button type="button" class={filterLink} title="Show this server's calls" onclick={() => setParam('server', c.serverName)}>
                            {c.serverName}
                          </button>
                        </td>
                      {/if}
                      <td class={cn(table.td, table.id, 'whitespace-nowrap')}>{c.tool ?? '—'}</td>
                      <td class={cn(table.td, 'whitespace-nowrap text-[13px]')} title={meta?.blurb}>
                        <span class="flex items-center gap-2">
                          <StatusDot status={meta?.dot ?? 'idle'} />{meta?.label ?? c.outcome}
                        </span>
                      </td>
                      <td class={cn(table.td, table.num, 'text-muted')}>{formatLatency(c.latencyMs)}</td>
                      <td class={cn(table.td, 'w-[28%] max-w-0 truncate pl-6 text-[13px] text-muted')} title={c.error ?? undefined}>{c.error ?? ''}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </SettingsGroup>
        {/snippet}
      </QueryState>
    </SettingsSection>

    {#if retentionQuery.data}
      <SettingsSection title="History">
        <SettingsGroup>
          <SettingsRow as="label" label="Keep calls for" hint="Older calls are deleted every six hours.">
            <Select
              size="sm"
              class="w-[280px]"
              aria-label="Keep tool calls for"
              value={String(retentionQuery.data.days)}
              disabled={savingRetention}
              onchange={(e) => void setRetention(Number(e.currentTarget.value))}
            >
              {#each retentionOptions(retentionQuery.data.days) as d (d)}<option value={String(d)}>{retentionLabel(d)}</option>{/each}
            </Select>
          </SettingsRow>
        </SettingsGroup>
      </SettingsSection>
    {/if}
  </div>
{/if}
