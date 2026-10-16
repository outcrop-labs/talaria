<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { KeyRound, MoreHorizontal, RefreshCw } from '@lucide/svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import McpAccessModal from './McpAccessModal.svelte'
  import McpCredentialsModal from './McpCredentialsModal.svelte'
  import McpOauthAppSetup from './McpOauthAppSetup.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { connectPopup, isInternalServer, patchServer, serverStatus, type McpServerRow } from './mcp'

  /** One registered server. Design grammar: a calm header (identity left, ONE
   *  status chip right that says what the server needs — and the one button
   *  that gives it that — actions in a kebab), the tool strip, then a single
   *  ACCESS line and a door to the table. The status is `serverStatus`'s
   *  verdict over the row: lifecycle, then auth, then what the last discovery
   *  found. */
  let { server: s }: { server: McpServerRow } = $props()

  const qc = useQueryClient()
  let error = $state<string | null>(null)
  let refreshing = $state(false)
  let redetecting = $state(false)
  let accessOpen = $state(false)
  let credentialsOpen = $state(false)
  let oauthEditing = $state(false)
  const menu = useContextMenu()
  const refresh = () => qc.invalidateQueries({ queryKey: ['mcp-servers'] })

  const patch = async (body: unknown) => {
    error = null
    const e = await patchServer(s.id, body)
    if (e) error = e
    await refresh()
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

  // The Workbench is granted by each agent's Developer Agent switch, not from
  // here. The api refuses assignment and access rows for it.
  const isWorkbench = $derived(s.name === 'workbench')
  const external = $derived(!isInternalServer(s))
  const status = $derived(serverStatus(s))
  const TONE: Record<typeof status.tone, string> = {
    muted: 'border-line text-muted',
    warning: 'border-warning/40 text-warning',
    danger: 'border-danger/40 text-danger',
    success: 'border-success/40 text-success',
  }
  const showOauthSetup = $derived(!!s.oauthMeta && (status.kind === 'needs-oauth-app' || oauthEditing))

  const cardMenu = (e: MouseEvent) =>
    menu.openMenu(e, [
      ...(external && !s.package && s.authMode === 'org'
        ? [{ label: Object.keys(s.headers).length ? 'Edit credentials…' : 'Set credentials…', onSelect: () => (credentialsOpen = true) }]
        : []),
      ...(external && !s.package
        ? [
            {
              label: 'Re-detect auth',
              onSelect: () => {
                redetecting = true
                void patch({ redetectAuth: true, refreshTools: true }).finally(() => (redetecting = false))
              },
            },
          ]
        : []),
      ...(s.oauthEnabled && s.oauthMeta?.clientSet ? [{ label: 'OAuth app settings…', onSelect: () => (oauthEditing = true) }] : []),
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
            try {
              await delJson<{ ok: true }>(`/api/mcp/servers/${s.id}`)
            } catch (e) {
              error = errorMessage(e)
              return
            }
            await refresh()
          })
        },
      },
    ])
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
            title="Talaria's own toolkit: every agent carries it. Govern who may use which tools below; the platform manages its identity and lifecycle."
          >
            built-in
          </Chip>
        {:else if s.appSlug}
          <Chip
            class="shrink-0"
            title={`Published by the "${s.appSlug}" app; its tools run inside this deployment. Govern access below; lifecycle follows the app (Manage → Apps).`}
          >
            app
          </Chip>
        {:else if s.package}
          <Chip
            class="shrink-0"
            title={`Runs as a hardened container from a ${s.package.kind.toUpperCase()} package. One org-shared credential, sealed at rest and spoken only to the container.`}
          >
            pkg
          </Chip>
        {:else if domain}
          <span class="truncate font-mono text-[11px] text-muted">{domain}</span>
        {/if}
        {#if s.authMode === 'per-user' && !s.builtin}
          <Chip class="shrink-0" title="Each person connects their own account; an assistant carries the server once its owner has.">per-user</Chip>
        {/if}
      </div>
      {#if s.description}<div class="truncate font-sans text-xs text-muted">{s.description}</div>{/if}
    </div>
    {#if !(status.kind === 'per-user' && s.authMode === 'per-user')}
      <span
        class={cn('shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]', TONE[status.tone])}
        title={status.detail ?? undefined}
      >
        {status.label}
      </span>
    {/if}
    {#if status.action === 'connect' && s.enabled}
      <Button
        size="sm"
        variant="accent-soft"
        class="shrink-0"
        onclick={() => void connectPopup(s.id, 'org')}
        title="This server authenticates with OAuth. Connect the org account so agents can use it"
      >
        Connect
      </Button>
    {:else if status.action === 'set-credentials' && s.enabled}
      <Button size="sm" variant="accent-soft" class="shrink-0 gap-1.5" onclick={() => (credentialsOpen = true)} title={status.detail ?? undefined}>
        <KeyRound size={13} /> Set credentials
      </Button>
    {/if}
    {#if !s.builtin}
      <IconButton size="sm" title="Server actions" onclick={cardMenu}>
        <MoreHorizontal size={15} />
      </IconButton>
    {/if}
  </div>

  {#if showOauthSetup && s.oauthMeta}
    <McpOauthAppSetup
      serverId={s.id}
      meta={s.oauthMeta}
      rejected={!!s.oauthMeta.dcrRejected}
      editing={oauthEditing}
      onSaved={() => {
        oauthEditing = false
        void refresh()
      }}
      onCancel={() => (oauthEditing = false)}
    />
  {/if}

  <!-- ── Tools strip ── -->
  <div class="mt-3 flex flex-wrap items-center gap-1.5">
    <Button variant="outline" size="xs" class="gap-1.5 rounded py-0.5 text-muted hover:text-fg" onclick={() => {
        refreshing = true
        void patch({ refreshTools: true }).finally(() => (refreshing = false))
      }}
      disabled={refreshing || redetecting}
      title="Ask the server for its tool catalog">
      {#if refreshing || redetecting}<WaitingMark site="mcp/server-refresh" size={11} />{:else}<RefreshCw size={11} />{/if}
      {s.tools.length ? `${s.tools.length} tools` : 'Discover tools'}
      {#if s.toolsRefreshedAt}<span class="normal-case text-ink-dim">· {relativeTime(s.toolsRefreshedAt)}</span>{/if}
    </Button>
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
  <!-- What the last discovery said, when it is something to act on — the
       server's own words, so a 401 reads as the server's 401. -->
  {#if s.probe && s.probe.state !== 'ok' && s.enabled && status.detail}
    <div class={cn('mt-2 font-sans text-[11px] leading-relaxed', status.tone === 'danger' ? 'text-danger' : 'text-muted')}>
      {s.probe.detail}
      <span class="text-ink-dim"> · {relativeTime(s.probe.at)}</span>
    </div>
  {/if}

  <!-- ── Access: the SHAPE of it, and a door to the table ──
       The full governance table (a row per agent, a row per person, tool
       subsets on each) used to sit open inside every card, so a registry of
       eight servers rendered eight copies of it and the thing you came to read
       — what this server is, whether it is connected — was the smallest part
       of the page. The counts are the part worth seeing at a glance; editing
       is a deliberate act and gets a dialog. -->
  <div class="mt-4 flex items-center gap-3 border-t border-line pt-3">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Access</span>
    <span class="min-w-0 flex-1 truncate font-sans text-xs text-muted">
      {#if isWorkbench}
        Every Developer Agent. Turn it on in an agent's settings.
      {:else if s.allAgents || s.builtin}
        Every agent{#if s.assignments.length}, {s.assignments.length} narrowed{/if}
      {:else if s.assignments.length}
        {s.assignments.length} agent{s.assignments.length === 1 ? '' : 's'}
      {:else}
        No agents yet
      {/if}
      {#if !isWorkbench}
        ·
        {#if s.userAccess.length}
          {s.userAccess.length} {s.userAccess.length === 1 ? 'person' : 'people'}
        {:else}
          no per-person rules
        {/if}
        {#if s.teamAccess.length}
          · {s.teamAccess.length} {s.teamAccess.length === 1 ? 'team' : 'teams'}
        {/if}
      {/if}
    </span>
    {#if !isWorkbench}
      <Button size="sm" variant="outline" onclick={() => (accessOpen = true)}>Manage access</Button>
    {/if}
  </div>

  {#if error}<div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">{error}</div>{/if}
  {#if accessOpen}<McpAccessModal server={s} onClose={() => (accessOpen = false)} />{/if}
  {#if credentialsOpen}<McpCredentialsModal server={s} onClose={() => (credentialsOpen = false)} />{/if}
  <ContextMenu {menu} />
</Panel>
