<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { AlertTriangle, Plus } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, postJson } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import { noteStillApplying } from '@/routes/app/agents.svelte'
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
    // Before the call: the roster's note reads only a roll that began after
    // the request went out as this change's.
    const sentAt = Date.now()
    try {
      const r = await postJson<{ ok: true; applied: boolean; warning?: string; pending?: boolean }>(`/api/fleet/defs/${def.id}/mcp`, body)
      if (r.warning) toastError('Applied to the config; the roll did not finish', new Error(r.warning))
      // A 202: the change is written, and its apply (a render, a roll) was
      // still going when the route stopped waiting. It goes on — never a
      // failure — and the roster notes the agent `still applying`.
      if (r.pending) {
        noteStillApplying(def.id, sentAt)
        pushToast({
          title: `${def.displayName} · Tools change saved, still applying`,
          body: 'Applying it is taking longer than this tab waits (a roll’s recreate and health check), and it goes on: the roster shows the agent when it lands.',
        })
      }
      await invalidate()
    } catch (e) {
      toastError('Tools change failed', e)
    } finally {
      busy = false
    }
  }

  // A chip marks the exception: a server this agent carries for a reason its
  // own switch does not show. One assigned here is the ordinary case, and its
  // switch says so.
  const VIA: Partial<Record<NonNullable<AgentServer['via']>, { label: string; hint: string }>> = {
    'all-agents': { label: 'Org-wide', hint: 'Every agent carries this server, and the Tools page governs it. A tool subset narrows what this agent sees.' },
    team: { label: 'Via team', hint: 'Granted by a team this agent is on. Turn it on to add a direct assignment; the team grant stays either way.' },
    developer: { label: 'Developer', hint: 'The Workbench rides on the Developer Agent switch in this agent\'s config.' },
  }

  const toggle = (s: AgentServer, on: boolean) => {
    if (on) void edit({ assign: [{ serverId: s.id, tools: null }] })
    else void edit({ unassign: [s.id] })
  }
  const canToggle = (s: AgentServer) =>
    canManage && !s.workbench && !s.builtin && s.withheld !== 'owner-denied' && !(s.allAgents && !s.assigned && s.carried)
  // A server every agent carries has no switch to show: the row says so.
  const alwaysOn = (s: AgentServer) => s.carried && (s.builtin || (s.allAgents && !s.assigned))
  const toggleHint = (s: AgentServer) =>
    s.workbench
      ? 'Granted by the Developer Agent switch'
      : s.builtin
        ? 'Every agent carries the toolkit'
        : s.withheld === 'owner-denied'
          ? "This assistant's owner is denied this server on the Tools page, so the agent does not carry it"
          : s.withheld === 'per-user-org-agent'
            ? 'A per-user server reaches an org agent only by a direct assignment. Turn it on to carry it here.'
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

  // A state is a dot and a word. Chips on these rows are kinds.
  const stateWord = 'flex items-center gap-1.5 whitespace-nowrap font-sans text-[13px] leading-5 text-muted'
  const connectionLabel = (s: AgentServer) =>
    s.authMode === 'per-user' ? 'Owner must connect' : s.probe?.state === 'reconnect' ? 'Reconnect needed' : 'Not connected'
  const probeLabel = (state: string) =>
    state === 'auth-required' ? 'Needs credentials' : state === 'reconnect' ? 'Reconnect needed' : state.charAt(0).toUpperCase() + state.slice(1)
  const toolCount = (s: AgentServer) =>
    s.tools.length === 0
      ? 'No tools discovered yet'
      : s.assignedTools
        ? `${s.assignedTools.length} of ${s.tools.length} tools`
        : `${s.tools.length} tool${s.tools.length === 1 ? '' : 's'}`
</script>

{#snippet serverRow(s: AgentServer)}
  {@const via = s.via && !alwaysOn(s) ? VIA[s.via] : undefined}
  {@const toolOptions = s.tools.map((t) => ({ value: t.name, label: t.name }))}
  {@const narrowable = canManage && s.carried && (s.assigned || s.allAgents) && !s.workbench && s.tools.length > 0}
  {@const about = [s.description, narrowable ? null : toolCount(s)].filter(Boolean).join(' · ')}
  <div class="flex min-h-[56px] items-center gap-3 px-5 py-3.5">
    <McpServerMark title={s.label} domain={s.domain} size={32} />
    <div class="min-w-0 flex-1">
      <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span class="line-clamp-2 min-w-0 font-sans text-sm font-medium leading-5 text-fg">{s.label}</span>
        {#if s.builtin}<Chip title="The Talaria toolkit, attached to every managed agent automatically">Built-in</Chip>
        {:else if s.appSlug}<Chip title={`Published by the "${s.appSlug}" app`}>App</Chip>
        {:else if s.package}<Chip title="Runs as a hardened container on this deployment">Package</Chip>{/if}
        {#if via}<Chip title={via.hint}>{via.label}</Chip>{/if}
        {#if s.needsConnection}
          <span
            class={stateWord}
            title={s.authMode === 'per-user'
              ? 'A per-user server: the agent carries it now, and its calls work once its owner connects their account under Settings → Connections. Until then the agent is told where to connect.'
              : s.probe?.state === 'reconnect'
                ? 'The provider refused to refresh the org account, so the grant is gone. Reconnect on the Tools page. Until then the agent is told an admin must connect it.'
                : 'The org account is not connected yet. Connect it on the Tools page. Until then the agent is told an admin must connect it.'}
          >
            <StatusDot status="warn" />{connectionLabel(s)}
          </span>
        {:else if s.probe && s.probe.state !== 'ok'}
          <span class={stateWord} title={s.probe.detail}><StatusDot status="danger" />{probeLabel(s.probe.state)}</span>
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
      <div class="line-clamp-2 font-sans text-[13px] leading-5 text-muted" title={about}>{about}</div>
    </div>
    {#if s.assignedToolsMissing.length && canManage && s.assigned}
      <SettingsRowFix
        label={`Fix ${s.label}: drop missing tools`}
        title={`Remove ${s.assignedToolsMissing.join(', ')} from this agent's tool subset`}
        disabled={busy}
        onclick={() => void dropMissing(s)}
      />
    {/if}
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
        class="w-56 shrink-0"
      />
    {/if}
    <!-- One trailing column down the list: a switch, or the reason there is none. -->
    <div class="flex w-[68px] shrink-0 justify-end" title={toggleHint(s)}>
      {#if alwaysOn(s)}
        <span class="whitespace-nowrap font-sans text-[13px] leading-5 text-muted">Always on</span>
      {:else}
        <Toggle checked={s.carried} disabled={busy || !canToggle(s)} onChange={(on) => toggle(s, on)} label="" />
      {/if}
    </div>
  </div>
{/snippet}

<div class="space-y-8">
  {#if busy}
    <Generating site="fleet/mcp-apply" label={`Applying to ${def.displayName}. It reaches the running agent within a few minutes, without a restart.`} lines={2} />
  {/if}
  <QueryState {query} errorTitle="Could not load tool servers" errorVariant="compact" isEmpty={() => false}>
    {#snippet skeleton()}<SkeletonRows rows={3} class="py-2" />{/snippet}
    {#snippet children(data)}
      <div class="space-y-8">
        <SettingsSection
          title="On this agent"
          info="Every server here is reached through Talaria's gateway: credentials stay on the server, and tool subsets and per-person rules are enforced on every call. The switch assigns or removes a server, and the picker narrows its tools for this agent."
        >
          {#snippet action()}
            {#if data.canInstall}
              <Button size="sm" variant="ghost" onclick={() => (adding = 'custom')}>Custom server</Button>
              <Button size="sm" variant="outline" onclick={() => (adding = 'marketplace')}><Plus size={16} /> Add from marketplace</Button>
            {:else if canManage}
              <span class="font-sans text-[13px] leading-5 text-muted" title="Registering new servers needs the agents.manage permission. You can assign any server already in the registry.">
                New servers are added on the <a href={p('/mcp')} class="text-accent hover:underline">Tools page</a>
              </span>
            {/if}
          {/snippet}
          {#if onThisAgent.length === 0}
            <SettingsGroup>
              <EmptyState variant="inline" class="px-5 py-4" title="No tool servers on this agent yet. Turn one on below, or add one from the marketplace." />
            </SettingsGroup>
          {:else}
            <div use:listStagger>
              <SettingsGroup>
                {#each onThisAgent as s (s.id)}{@render serverRow(s)}{/each}
              </SettingsGroup>
            </div>
          {/if}
        </SettingsSection>

        {#if available.length}
          <SettingsSection
            title="Available"
            description="Registered for the org but not on this agent."
            info="Turn one on to assign it. A per-user server also needs the owner's own connection."
          >
            <div class={cn(!canManage && 'opacity-80')} use:listStagger>
              <SettingsGroup>
                {#each available as s (s.id)}{@render serverRow(s)}{/each}
              </SettingsGroup>
            </div>
          </SettingsSection>
        {/if}

        {#if data.legacy.length}
          <SettingsSection
            title="Raw endpoints"
            description="Attached directly, before the gateway. Register each server, assign it, then remove the raw entry."
            info="These bypass credentials, tool subsets and per-person rules, and the agent dials them itself."
          >
            <SettingsGroup class="border-warning/40">
              {#each data.legacy as l (l.name)}
                <div class="flex min-h-[56px] items-center gap-3 px-5 py-3.5">
                  <AlertTriangle size={16} class="shrink-0 text-warning" />
                  <span class="w-40 shrink-0 truncate font-sans text-sm font-medium leading-5 text-fg">{l.name}</span>
                  <span class="min-w-0 flex-1 truncate font-mono text-xs text-muted">{l.url ?? ''}</span>
                  {#if canManage}
                    <DangerLink
                      disabled={busy}
                      onClick={async () => {
                        if (await confirm({ title: 'Remove raw endpoint', message: `Remove "${l.name}" from ${def.displayName}'s config?`, confirmLabel: 'Remove', danger: true }))
                          void edit({ removeLegacy: [l.name] })
                      }}
                    >
                      Remove
                    </DangerLink>
                  {/if}
                </div>
              {/each}
            </SettingsGroup>
          </SettingsSection>
        {/if}
      </div>
    {/snippet}
  </QueryState>

  <!-- The agent's own harness tools, below the servers it is lent. Outside the
       QueryState deliberately: it reads the fleet defs, not the MCP registry,
       so an MCP read that fails must not take the toolsets with it. -->
  <ToolsetsControl {def} {canManage} />

  {#if adding === 'marketplace'}
    <McpMarketplaceModal
      title={`Add tool servers to ${def.displayName}`}
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
