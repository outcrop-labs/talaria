<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { AlertTriangle, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, postJson } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import { p } from '@/router'
  import McpAddServerModal from '@/routes/app/McpAddServerModal.svelte'
  import McpMarketplaceModal from '@/routes/app/McpMarketplaceModal.svelte'
  import McpServerMark from '@/routes/app/McpServerMark.svelte'
  import { ALL_TOOLS, withoutMissing, type ProbeRow } from '@/routes/app/mcp'
  import ToolsetsControl from './ToolsetsControl.svelte'

  /** THIS AGENT'S MCP SERVERS — a view of the org registry scoped to one
   *  agent. Every server the agent carries, and why (org-wide, assigned here,
   *  a team grant, the Developer switch), a tool subset per assignment, and
   *  the ones it could carry with one toggle. Adding a server that is not in
   *  the registry yet goes through the same marketplace and custom form as
   *  Manage → MCP — installing AND assigning in one step for whoever may
   *  install (`agents.manage`); assigning what exists is the managers' call.
   *  Raw endpoints are never attached to an agent: every server here is
   *  reached through Talaria's gateway, so credentials, tool subsets and
   *  per-person rules hold. Leftover raw entries from before the gateway are
   *  listed to be removed. */
  interface AgentServer {
    id: string
    name: string
    label: string
    description: string | null
    domain: string | null
    builtin: boolean
    appSlug: string | null
    package: boolean
    workbench: boolean
    allAgents: boolean
    authMode: 'org' | 'per-user'
    oauthEnabled: boolean
    needsConnection: boolean
    probe: ProbeRow | null
    tools: Array<{ name: string; description?: string }>
    carried: boolean
    via: 'all-agents' | 'assigned' | 'team' | 'developer' | null
    /** Granted, but the rendered config leaves it out — and why. */
    withheld: 'owner-denied' | 'per-user-org-agent' | null
    assigned: boolean
    assignedTools: string[] | null
    /** Subset entries the server's catalog no longer offers (renamed or
     *  retired upstream) — the gateway can never let them through. */
    assignedToolsMissing: string[]
  }
  interface AgentMcp {
    agent: { id: string; model: string; slug: string; developer: boolean; personal: boolean }
    canInstall: boolean
    servers: AgentServer[]
    legacy: Array<{ name: string; url: string | null }>
  }

  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['agent-mcp', def.id],
    queryFn: (): Promise<AgentMcp> => getJson<AgentMcp>(`/api/fleet/defs/${def.id}/mcp`),
  }))
  let busy = $state(false)
  let adding = $state<null | 'marketplace' | 'custom'>(null)

  const invalidate = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['agent-mcp', def.id] }),
      qc.invalidateQueries({ queryKey: ['mcp-servers'] }),
      qc.invalidateQueries({ queryKey: ['mcp-agents'] }),
    ])

  const edit = async (body: { assign?: Array<{ serverId: string; tools: string[] | null }>; unassign?: string[]; removeLegacy?: string[] }) => {
    busy = true
    try {
      const r = await postJson<{ ok: true; applied: boolean; warning?: string }>(`/api/fleet/defs/${def.id}/mcp`, body)
      if (r.warning) toastError('Applied to the config; the roll did not finish', new Error(r.warning))
      await invalidate()
    } catch (e) {
      toastError('MCP change failed', e)
    } finally {
      busy = false
    }
  }

  const VIA: Record<NonNullable<AgentServer['via']>, { label: string; hint: string }> = {
    'all-agents': { label: 'org-wide', hint: 'Every agent carries this server; the toggle cannot remove it here (Manage → MCP governs it). A tool subset below narrows what this agent sees.' },
    assigned: { label: 'this agent', hint: 'Assigned to this agent. Turn it off to remove the assignment.' },
    team: { label: 'via team', hint: 'Granted by a team this agent is on. Turn it on to add a direct assignment; the team grant stays either way.' },
    developer: { label: 'developer', hint: 'The Workbench rides on the Developer Agent switch in this agent\'s config.' },
  }

  const toggle = (s: AgentServer, on: boolean) => {
    if (on) void edit({ assign: [{ serverId: s.id, tools: null }] })
    else void edit({ unassign: [s.id] })
  }
  const canToggle = (s: AgentServer) =>
    canManage && !s.workbench && !s.builtin && s.withheld !== 'owner-denied' && !(s.allAgents && !s.assigned && s.carried)
  const toggleHint = (s: AgentServer) =>
    s.workbench
      ? 'Granted by the Developer Agent switch'
      : s.builtin
        ? 'Every agent carries the toolkit'
        : s.withheld === 'owner-denied'
          ? "This assistant's owner is denied this server (Manage → MCP → access), so the agent does not carry it"
          : s.withheld === 'per-user-org-agent'
            ? 'A per-user server reaches an org agent only by a direct assignment — turn it on to carry it here'
            : s.allAgents && !s.assigned
              ? 'Org-wide: every agent carries it'
              : s.via === 'team' && !s.assigned
                ? 'Granted by a team; add a direct assignment to narrow tools'
                : undefined

  // The one-click fix for a stale subset: re-post the assignment without the
  // tools the server stopped offering. When nothing would remain, the subset
  // granted nothing usable anyway — remove the assignment (asked first) rather
  // than send an empty subset or a null that would widen it to every tool.
  const dropMissing = async (s: AgentServer) => {
    const kept = withoutMissing(s.assignedTools ?? [], s.assignedToolsMissing)
    if (kept) return void edit({ assign: [{ serverId: s.id, tools: kept }] })
    const ok = await confirm({
      title: 'Remove the assignment',
      message: `None of the tools picked for ${def.displayName} on "${s.label}" exist any more. Remove the assignment? Re-add it with the tools the server offers now.`,
      confirmLabel: 'Remove',
      danger: true,
    })
    if (ok) void edit({ unassign: [s.id] })
  }

  const carriedIds = $derived((query.data?.servers ?? []).filter((s) => s.carried).map((s) => s.id))
  const onThisAgent = $derived((query.data?.servers ?? []).filter((s) => s.carried))
  const available = $derived((query.data?.servers ?? []).filter((s) => !s.carried && !s.workbench))
</script>

{#snippet serverRow(s: AgentServer)}
  {@const via = s.via ? VIA[s.via] : null}
  {@const toolOptions = s.tools.map((t) => ({ value: t.name, label: t.name }))}
  {@const narrowable = canManage && s.carried && (s.assigned || s.allAgents) && !s.workbench && s.tools.length > 0}
  <div class="flex items-start gap-3 py-2.5">
    <McpServerMark title={s.label} domain={s.domain} size={30} />
    <div class="min-w-0 flex-1">
      <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <span class="truncate text-sm font-medium text-fg">{s.label}</span>
        {#if s.builtin}<Chip title="The Talaria toolkit, attached to every managed agent automatically">built-in</Chip>
        {:else if s.appSlug}<Chip title={`Published by the "${s.appSlug}" app`}>app</Chip>
        {:else if s.package}<Chip title="Runs as a hardened container on this deployment">pkg</Chip>{/if}
        {#if via}
          <span class="rounded border border-accent/40 bg-accent/5 px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-accent" title={via.hint}>{via.label}</span>
        {/if}
        {#if s.needsConnection}
          <span
            class="rounded border border-warning/40 px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-warning"
            title={s.authMode === 'per-user'
              ? 'A per-user server: the agent carries it now, and its calls work once its owner connects their account under Settings → Connections. Until then the agent is told where to connect.'
              : s.probe?.state === 'reconnect'
                ? 'The provider refused to refresh the org account, so the grant is gone. Reconnect on the MCP page; until then the agent is told an admin must connect it.'
                : 'The org account is not connected yet — Connect on the MCP page. Until then the agent is told an admin must connect it.'}
          >
            {s.authMode === 'per-user' ? 'owner must connect' : s.probe?.state === 'reconnect' ? 'reconnect needed' : 'not connected'}
          </span>
        {:else if s.probe && s.probe.state !== 'ok'}
          <span class="rounded border border-danger/40 px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-danger" title={s.probe.detail}>
            {s.probe.state === 'auth-required' ? 'needs credentials' : s.probe.state === 'reconnect' ? 'reconnect needed' : s.probe.state}
          </span>
        {/if}
        {#if s.assignedToolsMissing.length}
          <Chip
            tone="warn"
            title={`The server no longer offers ${s.assignedToolsMissing.join(', ')}. This agent's tool subset still names ${s.assignedToolsMissing.length === 1 ? 'it' : 'them'}, and calls to ${s.assignedToolsMissing.length === 1 ? 'it' : 'them'} can only fail.`}
          >
            {s.assignedToolsMissing.length} missing tool{s.assignedToolsMissing.length === 1 ? '' : 's'}
          </Chip>
        {/if}
      </div>
      {#if s.description}<div class="truncate font-sans text-xs text-muted">{s.description}</div>{/if}
      <div class="mt-1 flex flex-wrap items-center gap-1.5">
        {#if narrowable}
          <Combobox
            options={[{ value: ALL_TOOLS, label: 'All tools' }, ...toolOptions]}
            selected={s.assignedTools ?? [ALL_TOOLS]}
            onChange={(sel) => {
              const picked = sel.filter((t) => t !== ALL_TOOLS)
              // Picking "All tools" after a subset, or clearing the subset,
              // is null; a fresh pick is the subset.
              const tools = sel.includes(ALL_TOOLS) && s.assignedTools !== null ? null : picked.length ? picked : null
              void edit({ assign: [{ serverId: s.id, tools }] })
            }}
            multiple
            size="sm"
            placeholder="All tools"
            class="w-72"
          />
        {:else if s.tools.length}
          <span class="font-mono text-[10px] text-ink-dim">
            {s.assignedTools ? `${s.assignedTools.length} of ${s.tools.length} tools` : `${s.tools.length} tools`}
          </span>
        {:else}
          <span class="font-mono text-[10px] text-ink-dim">no tools discovered yet</span>
        {/if}
        {#if s.assignedToolsMissing.length && canManage && s.assigned}
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            onclick={() => void dropMissing(s)}
            title={`Remove ${s.assignedToolsMissing.join(', ')} from this agent's tool subset`}
          >
            Drop missing tools
          </Button>
        {/if}
      </div>
    </div>
    <div class="shrink-0 pt-1" title={toggleHint(s)}>
      <Toggle checked={s.carried} disabled={busy || !canToggle(s)} onChange={(on) => toggle(s, on)} label="" />
    </div>
  </div>
{/snippet}

<div class="space-y-4">
  {#if busy}
    <Generating site="fleet/mcp-apply" label={`Applying MCP change to ${def.displayName}: it reaches the running agent within a few minutes, without a restart`} lines={2} />
  {/if}
  <QueryState {query} errorTitle="Could not load MCP servers" errorVariant="compact" isEmpty={() => false}>
    {#snippet skeleton()}<SkeletonRows rows={3} class="py-2" />{/snippet}
    {#snippet children(data)}
      <div class="flex items-center gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">On this agent</span>
        <InfoTip text="Every server here is reached through Talaria's gateway: credentials stay on the server, tool subsets and per-person rules are enforced per call. The toggle assigns or removes a server; the picker narrows its tools for this agent." />
        <span class="flex-1"></span>
        {#if data.canInstall}
          <Button size="sm" variant="ghost" onclick={() => (adding = 'custom')}>Custom server</Button>
          <Button size="sm" onclick={() => (adding = 'marketplace')}><Plus size={14} /> Add from marketplace</Button>
        {:else if canManage}
          <span class="font-sans text-xs text-muted" title="Registering new servers needs the agents.manage permission; you can assign any server already in the registry.">
            Registry: <a href={p('/mcp')} class="text-accent hover:underline">Manage → MCP</a>
          </span>
        {/if}
      </div>
      {#if onThisAgent.length === 0}
        <EmptyState variant="inline" class="py-2" title="No MCP servers on this agent yet. Turn one on below, or add one from the marketplace." />
      {:else}
        <div class="divide-y divide-line" use:listStagger>
          {#each onThisAgent as s (s.id)}{@render serverRow(s)}{/each}
        </div>
      {/if}

      {#if available.length}
        <div class="mt-5 flex items-center gap-2">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Available in the registry</span>
          <InfoTip text="Registered org-wide but not on this agent. Turn one on to assign it; a per-user server also needs the owner's own connection." />
        </div>
        <div class={cn('divide-y divide-line', !canManage && 'opacity-80')} use:listStagger>
          {#each available as s (s.id)}{@render serverRow(s)}{/each}
        </div>
      {/if}

      {#if data.legacy.length}
        <div class="mt-5 rounded-lg border border-warning/40 bg-warning/5 p-3">
          <div class="flex items-center gap-2 text-xs font-medium text-fg">
            <AlertTriangle size={13} class="text-warning" /> Raw endpoints in this agent's config
            <InfoTip text="These were attached directly, before the gateway. They bypass credentials, tool subsets and per-person rules, and the agent dials them itself. Register the server on the MCP page (or from the marketplace above), assign it, then remove the raw entry." />
          </div>
          <div class="mt-2 divide-y divide-line-subtle">
            {#each data.legacy as l (l.name)}
              <div class="flex items-center gap-3 py-1.5 text-sm">
                <span class="w-32 shrink-0 truncate font-medium text-fg">{l.name}</span>
                <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">{l.url ?? ''}</span>
                {#if canManage}
                  <Button
                    variant="ghost"
                    size="xs"
                    class="shrink-0 hover:text-danger"
                    disabled={busy}
                    onclick={async () => {
                      if (await confirm({ title: 'Remove raw MCP entry', message: `Remove "${l.name}" from ${def.displayName}'s config?`, confirmLabel: 'Remove', danger: true }))
                        void edit({ removeLegacy: [l.name] })
                    }}
                  >
                    Remove
                  </Button>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}
    {/snippet}
  </QueryState>

  <!-- The agent's own harness tools, below the servers it is lent. Outside the
       QueryState deliberately: it reads the fleet defs, not the MCP registry,
       so an MCP read that fails must not take the toolsets with it. -->
  <ToolsetsControl {def} {canManage} />

  {#if adding === 'marketplace'}
    <McpMarketplaceModal
      title={`Add MCP servers to ${def.displayName}`}
      carriedServerIds={carriedIds}
      onClose={() => (adding = null)}
      onCustom={() => (adding = 'custom')}
      onInstalled={(serverId) => void edit({ assign: [{ serverId, tools: null }] })}
      onPickInstalled={(serverId) => void edit({ assign: [{ serverId, tools: null }] })}
    />
  {/if}
  {#if adding === 'custom'}
    <McpAddServerModal onClose={() => (adding = null)} onCreated={(serverId) => void edit({ assign: [{ serverId, tools: null }] })} />
  {/if}
</div>
