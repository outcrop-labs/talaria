<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { cn } from '@/lib/cn'
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

  // The MCP gateway's call record: every tools/call an agent made through the
  // gateway — and every refusal — with who, which server and tool, how it went
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
  const strip = $derived.by((): McpCallCounts | null => {
    const s = summaryQuery.data
    if (!s) return null
    if (!server) return s.totals
    const one = serverCalls(s, server)
    return one ?? { calls: 0, errors: 0, denied: 0, notConnected: 0 }
  })
  const serverOptions = $derived.by(() => {
    const names = (summaryQuery.data?.servers ?? []).map((s) => s.serverName)
    return server && !names.includes(server) ? [server, ...names] : names
  })
  const agentOptions = $derived.by(() => {
    const names = (summaryQuery.data?.agents ?? []).map((a) => a.agent)
    return agent && !names.includes(agent) ? [agent, ...names] : names
  })
  const windowLabel = $derived(
    win === '1h' ? 'last hour' : win === '24h' ? 'last 24 hours' : 'last 7 days',
  )
  const fullTime = (iso: string) => new Date(iso).toLocaleString()

  // How long rows are kept (app setting mcp_calls_retention_days). Applies
  // on change; the next six-hourly prune enforces it.
  const qc = useQueryClient()
  const retentionQuery = useMcpCallsRetention(() => isAdmin)
  let savingRetention = $state(false)
  const setRetention = async (days: number) => {
    savingRetention = true
    try {
      await putJson(RETENTION_URL, { days })
      await qc.invalidateQueries({ queryKey: ['mcp-calls-retention'] })
    } catch (e) {
      toastError('Could not change how long MCP calls are kept', e)
    } finally {
      savingRetention = false
    }
  }
</script>

{#if !isAdmin}
  <EmptyState
    icon="⛨"
    title="Admins only"
    hint="The MCP call record names which agent called which tool on which server; an admin reads it here."
  />
{:else}
  <div class="space-y-6">
    <!-- Filters: the window, then the three narrowing selects. -->
    <div class="flex flex-wrap items-center gap-2">
      <Segmented
        options={WINDOWS.map((w) => ({ id: w.id, label: w.label, title: `Calls in the ${w.label === '1h' ? 'last hour' : `last ${w.label}`}` }))}
        value={win}
        onChange={(w) => setParam('w', w === '24h' ? null : w)}
      />
      <Select size="sm" class="w-44" aria-label="Server" value={server ?? ''} onchange={(e) => setParam('server', e.currentTarget.value || null)}>
        <option value="">All servers</option>
        {#each serverOptions as name (name)}<option value={name}>{name}</option>{/each}
      </Select>
      <Select size="sm" class="w-44" aria-label="Agent" value={agent ?? ''} onchange={(e) => setParam('agent', e.currentTarget.value || null)}>
        <option value="">All agents</option>
        {#each agentOptions as name (name)}<option value={name}>{name}</option>{/each}
      </Select>
      <Select size="sm" class="w-40" aria-label="Outcome" value={outcome ?? ''} onchange={(e) => setParam('outcome', e.currentTarget.value || null)}>
        <option value="">Every outcome</option>
        {#each OUTCOMES as o (o)}<option value={o} title={OUTCOME_META[o].blurb}>{OUTCOME_META[o].label}</option>{/each}
      </Select>
      {#if filtered}
        <Button
          variant="ghost"
          size="xs"
          onclick={() => {
            setParam('server', null)
            setParam('agent', null)
            setParam('outcome', null)
          }}
        >
          Clear filters
        </Button>
      {/if}
      {#if retentionQuery.data}
        <label class="ml-auto flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted" title="Older calls are deleted every six hours">
          Keep for
          <Select
            size="sm"
            class="w-28"
            aria-label="Keep MCP calls for"
            value={String(retentionQuery.data.days)}
            disabled={savingRetention}
            onchange={(e) => void setRetention(Number(e.currentTarget.value))}
          >
            {#each retentionOptions(retentionQuery.data.days) as d (d)}<option value={String(d)}>{retentionLabel(d)}</option>{/each}
          </Select>
        </label>
      {/if}
    </div>

    <!-- The summary strip: the window's counts (or one server's), then the
         per-server readout when more than one server was called. -->
    <QueryState query={summaryQuery} errorTitle="Could not load the MCP call summary" errorVariant="compact">
      {#snippet skeleton()}
        <div aria-hidden="true" class="grid grid-cols-2 gap-4 sm:grid-cols-4">
          {#each [0, 1, 2, 3] as i (i)}
            <div class="rounded-lg border border-line bg-panel p-5">
              <Skeleton class="h-2.5 w-20 rounded-full" />
              <Skeleton class="mt-2.5 h-6 w-14 rounded" />
              <Skeleton class="mt-2 h-2.5 w-24 rounded-full" />
            </div>
          {/each}
        </div>
      {/snippet}
      {#snippet children(summary)}
        <div class="space-y-6">
          <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <StatCard label={server ? `Calls · ${server}` : 'Calls'} value={(strip?.calls ?? 0).toLocaleString('en-US')} sub={windowLabel} />
            <StatCard
              label="Errors"
              value={(strip?.errors ?? 0).toLocaleString('en-US')}
              sub={strip ? `${errorRate(strip)} of calls` : undefined}
            />
            <StatCard label="Denied" value={(strip?.denied ?? 0).toLocaleString('en-US')} sub="refused by the gateway" />
            <StatCard label="Not connected" value={(strip?.notConnected ?? 0).toLocaleString('en-US')} sub="granted, account missing" />
          </div>
          {#if !server && summary.servers.length > 0}
            <Panel class="p-0">
              <div class="px-6 pt-5"><SectionHeader title="By server" action={String(summary.servers.length).padStart(2, '0')} /></div>
              <div class="overflow-x-auto">
                <table class="w-full text-sm">
                  <thead class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                    <tr>
                      <th class="px-6 py-2 text-left font-medium">Server</th>
                      <th class="px-3 py-2 text-right font-medium">Calls</th>
                      <th class="px-3 py-2 text-right font-medium">Errors</th>
                      <th class="px-3 py-2 text-right font-medium">Denied</th>
                      <th class="px-3 py-2 text-right font-medium">Not conn.</th>
                      <th class="px-3 py-2 text-right font-medium">p50</th>
                      <th class="px-3 py-2 text-right font-medium">p95</th>
                      <th class="px-6 py-2 text-left font-medium">Top tools</th>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-line-subtle">
                    {#each summary.servers as s (s.serverId)}
                      <tr>
                        <td class="px-6 py-2">
                          <Button
                            variant="link"
                            size="xs"
                            class="text-left font-medium text-fg"
                            title="Show this server's calls"
                            onclick={() => setParam('server', s.serverName)}
                          >
                            {s.serverName}
                          </Button>
                        </td>
                        <td class="px-3 py-2 text-right font-mono text-[12px] text-fg">{s.calls.toLocaleString('en-US')}</td>
                        <td class={cn('px-3 py-2 text-right font-mono text-[12px]', s.errors > 0 ? 'text-danger' : 'text-muted')}>{s.errors}</td>
                        <td class={cn('px-3 py-2 text-right font-mono text-[12px]', s.denied > 0 ? 'text-warning' : 'text-muted')}>{s.denied}</td>
                        <td class="px-3 py-2 text-right font-mono text-[12px] text-muted">{s.notConnected}</td>
                        <td class="px-3 py-2 text-right font-mono text-[12px] text-muted">{formatLatency(s.p50Ms)}</td>
                        <td class="px-3 py-2 text-right font-mono text-[12px] text-muted">{formatLatency(s.p95Ms)}</td>
                        <td class="px-6 py-2">
                          <span class="flex flex-wrap gap-1">
                            {#each s.topTools as t (t.tool)}
                              <span
                                class="rounded border border-line-subtle px-1.5 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted"
                                title={`${t.calls} calls · ${t.errors} errors`}
                              >
                                {t.tool} <span class="text-ink-dim">{t.calls}</span>
                              </span>
                            {/each}
                          </span>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </Panel>
          {/if}
        </div>
      {/snippet}
    </QueryState>

    <!-- The record, newest first. -->
    <QueryState query={callsQuery} errorTitle="Could not load MCP calls">
      {#snippet skeleton()}
        <div aria-hidden="true" class="rounded-lg border border-line bg-panel p-6">
          <div class="divide-y divide-line">
            {#each [0, 1, 2, 3, 4, 5] as i (i)}
              <div class="flex items-center gap-3 py-3">
                <Skeleton class="h-3 w-14 rounded" />
                <Skeleton class={`h-3 rounded-full ${['w-24', 'w-20', 'w-28'][i % 3]}`} />
                <Skeleton class="h-3 w-20 rounded-full" />
                <Skeleton class="h-3 w-16 rounded-full" />
                <span class="ml-auto"><Skeleton class="h-3 w-12 rounded" /></span>
              </div>
            {/each}
          </div>
        </div>
      {/snippet}
      {#snippet empty()}
        <EmptyState
          icon="⇄"
          title={filtered ? 'No calls match' : 'No MCP calls yet'}
          hint={filtered
            ? `Nothing in the ${windowLabel} matches these filters.`
            : `Every tool call an agent makes through the MCP gateway in the ${windowLabel} lands here — the arguments and results are never kept.`}
        />
      {/snippet}
      {#snippet children(calls)}
        <Panel class={cn('p-0 transition-opacity', callsQuery.isPlaceholderData && 'opacity-60')}>
          <div class="px-6 pt-5">
            <SectionHeader
              title="Calls"
              info="Newest first, up to 200. A row never carries the call's arguments or its result."
              action={String(calls.length).padStart(2, '0')}
            />
          </div>
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                <tr>
                  <th class="px-6 py-2 text-left font-medium">Time</th>
                  <th class="px-3 py-2 text-left font-medium">Agent</th>
                  <th class="px-3 py-2 text-left font-medium">Server</th>
                  <th class="px-3 py-2 text-left font-medium">Tool</th>
                  <th class="px-3 py-2 text-left font-medium">Outcome</th>
                  <th class="px-3 py-2 text-right font-medium">Latency</th>
                  <th class="px-6 py-2 text-left font-medium">Error</th>
                </tr>
              </thead>
              <tbody class="divide-y divide-line-subtle">
                {#each calls as c (c.id)}
                  <tr class="align-top">
                    <td class="whitespace-nowrap px-6 py-2 font-mono text-[11px] text-muted" title={fullTime(c.at)}>{relativeTime(c.at)}</td>
                    <td class="px-3 py-2">
                      <Button
                        variant="link"
                        size="xs"
                        class="text-left text-fg"
                        title="Show this agent's calls"
                        onclick={() => setParam('agent', c.agent)}
                      >
                        {c.agent}
                      </Button>
                      {#if c.userName}<div class="font-sans text-[11px] text-muted">for {c.userName}</div>{/if}
                    </td>
                    <td class="px-3 py-2">
                      <Button
                        variant="link"
                        size="xs"
                        class="text-left font-mono text-[12px] text-fg"
                        title="Show this server's calls"
                        onclick={() => setParam('server', c.serverName)}
                      >
                        {c.serverName}
                      </Button>
                    </td>
                    <td class="px-3 py-2 font-mono text-[12px] text-fg">{c.tool ?? '—'}</td>
                    <td class="px-3 py-2">
                      <Chip tone={OUTCOME_META[c.outcome]?.tone ?? 'neutral'} title={OUTCOME_META[c.outcome]?.blurb}>
                        {OUTCOME_META[c.outcome]?.label ?? c.outcome}
                      </Chip>
                    </td>
                    <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[12px] text-muted">{formatLatency(c.latencyMs)}</td>
                    <td class="max-w-[28rem] px-6 py-2 font-sans text-xs text-muted">
                      {#if c.error}<span class="line-clamp-2 break-words" title={c.error}>{c.error}</span>{/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </Panel>
      {/snippet}
    </QueryState>
  </div>
{/if}
