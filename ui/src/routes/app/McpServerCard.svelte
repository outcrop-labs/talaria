<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { MoreHorizontal, RefreshCw } from '@lucide/svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { useAgents } from '@/lib/agents'
  import { useUsers } from '@/lib/users'
  import McpAccessRow from './McpAccessRow.svelte'
  import McpAddPickerButton from './McpAddPickerButton.svelte'
  import McpOauthAppSetup from './McpOauthAppSetup.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { ALL_TOOLS, NO_ACCESS, connectPopup, patchServer, resolveScopePick, type McpServerRow } from './mcp'

  /** One registered server. Design grammar: a calm header (identity left, one
   *  status cluster right, actions in a kebab), the tool strip, then a single
   *  ACCESS table where every row reads the same — name · tools · remove — and
   *  adders stay hidden behind ghost "+" buttons until asked for. */
  let { server: s }: { server: McpServerRow } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  // Access RULES are shown per person by name. Defaulted to `[]` a failed
  // directory read rendered each existing rule as a truncated raw id and the
  // "add a person" picker as empty — an access table that looks like it is
  // about strangers, on a server that grants tool use.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const users = $derived(usersList.rows)
  let error = $state<string | null>(null)
  let refreshing = $state(false)
  const menu = useContextMenu()
  const refresh = () => qc.invalidateQueries({ queryKey: ['mcp-servers'] })

  const patch = async (body: unknown) => {
    error = null
    const e = await patchServer(s.id, body)
    if (e) error = e
    await refresh()
  }

  const agentOptions = $derived((fleetQuery.data?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))
  const agentLabel = (model: string) => agentOptions.find((o) => o.value === model)?.label ?? model
  const toolOptions = $derived(s.tools.map((t) => ({ value: t.name, label: t.name })))
  const userLabel = (id: string) => {
    const u = users.find((x) => x.id === id)
    return u?.name ?? u?.email ?? id.slice(0, 8)
  }
  // A "domain" only exists for real web URLs — pseudo-schemes like
  // talaria-workbench://core carry a routing token, not a hostname.
  const domain = $derived.by(() => {
    try {
      const u = new URL(s.url)
      return u.protocol === 'http:' || u.protocol === 'https:' ? u.hostname : null
    } catch {
      return null
    }
  })

  const cardMenu = (e: MouseEvent) =>
    menu.openMenu(e, [
      { label: s.enabled ? 'Disable server' : 'Enable server', onSelect: () => void patch({ enabled: !s.enabled }) },
      {
        label: s.authMode === 'org' ? 'Switch to per-user auth' : 'Switch to org auth',
        onSelect: () => void patch({ authMode: s.authMode === 'org' ? 'per-user' : 'org' }),
      },
      ...(s.oauthEnabled && s.authMode === 'org' && s.orgConnected
        ? [{ label: 'Reconnect org account', onSelect: () => void connectPopup(s.id, 'org') }]
        : []),
      'sep' as const,
      {
        label: 'Remove server',
        danger: true,
        onSelect: () => {
          void confirm({
            title: 'Remove MCP server',
            message: `Remove "${s.label}" org-wide? Every agent loses it on its next roll.`,
            confirmLabel: 'Remove',
          }).then(async (ok) => {
            if (!ok) return
            await fetch(`/api/mcp/servers/${s.id}`, { method: 'DELETE', credentials: 'same-origin' })
            await refresh()
          })
        },
      },
    ])

  // The one status the header needs: connection first, then lifecycle.
  const status = $derived(
    !s.enabled
      ? { label: 'disabled', cls: 'border-line text-muted' }
      : s.oauthEnabled && s.authMode === 'org' && !s.orgConnected
        ? null // the Connect button IS the status
        : s.oauthEnabled && s.authMode === 'per-user'
          ? { label: 'per-user auth', cls: 'border-line text-muted' }
          : s.oauthEnabled && s.orgConnected
            ? { label: '✓ connected', cls: 'border-success/40 text-success' }
            : null,
  )
</script>

<Panel class={cn(!s.enabled && 'opacity-60')}>
  <!-- ── Header: identity left, one status cluster right ── -->
  <div class="flex items-center gap-3">
    <McpServerMark title={s.label} domain={domain?.replace(/^mcp\./, '')} size={36} />
    <div class="min-w-0 flex-1">
      <div class="flex items-baseline gap-2">
        <span class="truncate text-sm font-semibold text-fg">{s.label}</span>
        {#if s.builtin}
          <Chip
            class="shrink-0"
            title="Talaria's own toolkit — every agent carries it. Govern who may use which tools below; identity and lifecycle are managed by the platform."
          >
            built-in
          </Chip>
        {:else if s.appSlug}
          <Chip
            class="shrink-0"
            title={`Published by the "${s.appSlug}" app — tools dispatch inside this deployment. Govern access below; lifecycle follows the app (Manage → Apps).`}
          >
            app
          </Chip>
        {:else if domain}
          <span class="truncate font-mono text-[11px] text-muted">{domain}</span>
        {/if}
      </div>
      {#if s.description}<div class="truncate font-sans text-xs text-muted">{s.description}</div>{/if}
    </div>
    {#if status}
      <span class={cn('shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]', status.cls)}>
        {status.label}
      </span>
    {/if}
    {#if s.oauthEnabled && s.authMode === 'org' && !s.orgConnected && s.enabled && (s.oauthMeta?.dcr || s.oauthMeta?.clientSet)}
      <Button
        size="sm"
        variant="accent-soft"
        class="shrink-0"
        onclick={() => void connectPopup(s.id, 'org')}
        title="This server authenticates with OAuth — connect the org account so agents can use it"
      >
        Connect
      </Button>
    {/if}
    {#if !s.builtin}
      <IconButton size="sm" title="Server actions" onclick={cardMenu}>
        <MoreHorizontal size={15} />
      </IconButton>
    {/if}
  </div>

  {#if s.oauthEnabled && s.oauthMeta && !s.oauthMeta.dcr && !s.oauthMeta.clientSet}
    <McpOauthAppSetup serverId={s.id} {domain} docs={s.oauthMeta.documentation} onSaved={refresh} />
  {/if}

  <!-- ── Tools strip ── -->
  <div class="mt-3 flex flex-wrap items-center gap-1.5">
    <button
      type="button"
      onclick={() => {
        refreshing = true
        void patch({ refreshTools: true }).finally(() => (refreshing = false))
      }}
      class="flex items-center gap-1.5 rounded border border-line px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg"
      title="Ask the server for its tool catalog"
    >
      {#if refreshing}<GeneratingBars bars={3} variant="scan" />{:else}<RefreshCw size={11} />{/if}
      {s.tools.length ? `${s.tools.length} tools` : 'Discover tools'}
      {#if s.toolsRefreshedAt}<span class="normal-case text-ink-dim">· {relativeTime(s.toolsRefreshedAt)}</span>{/if}
    </button>
    {#each s.tools.slice(0, 6) as t (t.name)}
      <span title={t.description} class="rounded border border-line-subtle px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted">
        {t.name}
      </span>
    {/each}
    {#if s.tools.length > 6}
      <span class="font-mono text-[10px] tracking-[0.05em] text-ink-dim" title={s.tools.slice(6).map((t) => t.name).join(', ')}>
        +{s.tools.length - 6} more
      </span>
    {/if}
  </div>

  <!-- ── Access: two bounded groups — agents, then people ── -->
  <div class="mt-4 border-t border-line pt-3">
    <div class="mb-2 flex items-center gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Access</span>
      <InfoTip text="Which agents carry this server, and which people may exercise it through agents acting for them. Tool cells narrow a row to a subset; empty = every tool. The gateway enforces all of it." />
    </div>

    <!-- Agents -->
    <div>
      <div class="flex items-center gap-3 pb-1">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agents</span>
        {#if s.builtin}
          <span class="text-xs text-muted">every agent — rows below narrow individual agents</span>
        {:else}
          <Checkbox
            checked={s.allAgents}
            onChange={(checked) => void patch({ allAgents: checked })}
            label="all agents"
            title="Every enabled agent carries this server; rows below become per-agent tool overrides"
          />
        {/if}
        <span class="flex-1"></span>
        <McpAddPickerButton
          title={s.allAgents ? 'Add a per-agent tool override' : 'Add an agent'}
          placeholder="Search agents"
          options={agentOptions.filter((o) => !s.assignments.some((a) => a.agentModel === o.value))}
          onPick={(m) => void patch({ assign: { agentModel: m, tools: null } })}
        />
      </div>
      {#if s.assignments.length === 0}
        <EmptyState
          variant="inline"
          class="px-1.5 py-1 text-muted/70"
          title={s.allAgents ? 'Every enabled agent, every tool. Add a row to narrow one agent.' : 'No agents yet — add one, or check “all agents”.'}
        />
      {:else}
        <div class="space-y-0.5">
          {#if s.allAgents}
            <div class="px-1.5 pb-0.5 text-[11px] text-muted/70">Every enabled agent carries this server; these rows narrow individual agents.</div>
          {/if}
          {#each s.assignments as a (a.agentModel)}
            <McpAccessRow
              name={agentLabel(a.agentModel)}
              onRemove={() => void patch({ unassign: a.agentModel })}
              removeTitle="Remove this agent"
            >
              {#snippet tools()}
                <Combobox
                  options={[{ value: ALL_TOOLS, label: 'All tools' }, ...toolOptions]}
                  selected={a.tools ?? [ALL_TOOLS]}
                  onChange={(sel) => {
                    const next = resolveScopePick(sel, { denied: false, tools: a.tools })
                    void patch({ assign: { agentModel: a.agentModel, tools: next.tools } })
                  }}
                  multiple
                  size="sm"
                  placeholder="All tools"
                  class="w-full"
                />
              {/snippet}
            </McpAccessRow>
          {/each}
        </div>
      {/if}
    </div>

    <!-- People -->
    <div class="mt-3 border-t border-line-subtle pt-2.5">
      <div class="flex items-center gap-2 pb-1">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People</span>
        <InfoTip text="No rules = everyone with an assigned agent may use it. A rule narrows one person to specific tools, or denies them outright." />
        <span class="flex-1"></span>
        <McpAddPickerButton
          title="Add a person rule"
          placeholder="Search people"
          options={users
            .filter((u) => !s.userAccess.some((r) => r.userId === u.id))
            .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id.slice(0, 8), sub: u.email ?? undefined }))}
          onPick={(id) => void patch({ userAccess: { userId: id, allowed: true, tools: null } })}
        />
      </div>
      {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
      {#if s.userAccess.length === 0}
        <EmptyState variant="inline" class="px-1.5 py-1 text-muted/70" title="Everyone with an assigned agent may use it." />
      {:else}
        <div class="space-y-0.5">
          {#each s.userAccess as ua (ua.userId)}
            <McpAccessRow
              name={userLabel(ua.userId)}
              dim={!ua.allowed}
              onRemove={() => void patch({ userAccess: { userId: ua.userId, allowed: null, tools: null } })}
              removeTitle="Remove this rule (back to default access)"
            >
              {#snippet tools()}
                <Combobox
                  options={[
                    { value: ALL_TOOLS, label: 'All tools' },
                    { value: NO_ACCESS, label: 'No access' },
                    ...toolOptions,
                  ]}
                  selected={ua.allowed ? (ua.tools ?? [ALL_TOOLS]) : [NO_ACCESS]}
                  onChange={(sel) => {
                    const next = resolveScopePick(sel, { denied: !ua.allowed, tools: ua.tools })
                    void patch({ userAccess: { userId: ua.userId, allowed: !next.denied, tools: next.tools } })
                  }}
                  multiple
                  size="sm"
                  placeholder="All tools"
                  class="w-full"
                />
              {/snippet}
            </McpAccessRow>
          {/each}
        </div>
      {/if}
    </div>
  </div>

  {#if error}<div class="mt-2 text-xs text-danger">{error}</div>{/if}
  <ContextMenu {menu} />
</Panel>
