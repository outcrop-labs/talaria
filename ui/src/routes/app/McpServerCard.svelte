<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, errorMessage } from '@/lib/fetch-json'
  import { iconDomains } from '@/lib/mcp-icon'
  import { useSession } from '@/lib/session'
  import McpServerModal from './McpServerModal.svelte'
  import McpCredentialsModal from './McpCredentialsModal.svelte'
  import McpOauthAppSetup from './McpOauthAppSetup.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import {
    STATUS_DOT,
    connectPopup,
    healthChecked,
    isInternalServer,
    patchServer,
    serverDomain,
    serverStatus,
    staleAssignments,
    type McpServerRow,
  } from './mcp'

  /** One registered server as a row of the list: what it is, what it is for,
   *  how much it is used and its status. Clicking the row opens Manage: the
   *  tool list, the last check, the calls and the access table are one level
   *  down, in that dialog. A button shows only when the status names something
   *  to fix (sign in, set credentials, check again). The status is
   *  `serverStatus`'s verdict over the row: lifecycle, then auth, then what
   *  the last discovery found. */
  let { server: s }: { server: McpServerRow } = $props()

  const qc = useQueryClient()
  const session = useSession()
  let error = $state<string | null>(null)
  let checking = $state(false)
  let manageOpen = $state(false)
  let credentialsOpen = $state(false)
  let oauthOpen = $state(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['mcp-servers'] })

  const patch = async (body: unknown) => {
    error = null
    const e = await patchServer(s.id, body)
    if (e) error = e
    await refresh()
  }
  const ask = (body: unknown) => {
    checking = true
    void patch(body).finally(() => (checking = false))
  }

  const domain = $derived(serverDomain(s.url))
  // The provider an OAuth server signs in with is the one publisher domain a
  // registry row carries (the marketplace entry's own is not stored).
  const markDomains = $derived(iconDomains({ publisher: s.oauthMeta?.guide?.domain, url: s.url }))
  // The Workbench is granted by each agent's Developer Agent switch, not from
  // here. The api refuses assignment and access rows for it.
  const isWorkbench = $derived(s.name === 'workbench')
  const verdict = $derived(serverStatus(s))
  // Assignments whose tool subset names a tool the catalog no longer offers;
  // each agent's MCP tab offers the one-click fix.
  const stale = $derived(staleAssignments(s.assignments, s.tools))

  const count = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`
  const agentsLine = $derived(
    isWorkbench
      ? 'Developer Agents'
      : s.allAgents || s.builtin
        ? 'Every agent'
        : s.assignments.length
          ? count(s.assignments.length, 'agent')
          : 'No agents yet',
  )
  const use = $derived(`${s.tools.length ? count(s.tools.length, 'tool') : 'No tools yet'} · ${agentsLine}`)

  // The one thing the row asks for, when its status asks for something. A
  // server the health pass does not ask (the deployment's own, a stdio
  // package) has no "check again" to offer.
  type Fix = { label: string; title?: string; run: () => void }
  const fix = $derived.by((): Fix | null => {
    if (!s.enabled) return null
    if (verdict.action === 'connect') {
      return verdict.kind === 'reconnect'
        ? {
            label: 'Reconnect',
            title: 'The provider refused to refresh the org account. Sign in again so agents can use it',
            run: () => void connectPopup(s.id, 'org'),
          }
        : {
            label: 'Connect',
            title: 'This server signs in with OAuth. Connect the org account so agents can use it',
            run: () => void connectPopup(s.id, 'org'),
          }
    }
    if (verdict.action === 'set-credentials') {
      return { label: 'Set credentials', title: verdict.detail ?? undefined, run: () => (credentialsOpen = true) }
    }
    if (verdict.action === 'setup-oauth-app' && s.oauthMeta) {
      return { label: 'Set up', title: verdict.detail ?? undefined, run: () => (oauthOpen = true) }
    }
    if (healthChecked(s) && (verdict.kind === 'unreachable' || verdict.kind === 'error' || verdict.kind === 'untested')) {
      return {
        label: 'Check now',
        title: 'Ask the server again now: the handshake, its tool list, and whether the credentials still work',
        run: () => ask({ refreshTools: true }),
      }
    }
    return null
  })

  // Drops the org's OAuth tokens; the gateway answers for the server
  // in-house again and the carriers re-list on its notice, without a restart.
  const disconnectOrg = async () => {
    const ok = await confirm({
      title: 'Disconnect org account',
      message: `Disconnect the org's ${s.label} account? Within a few minutes, without a restart, agents that carry "${s.label}" can no longer use its tools: each call answers that the account isn't connected, until someone connects it again.`,
      confirmLabel: 'Disconnect',
      danger: true,
    })
    if (!ok) return
    error = null
    try {
      await delJson<{ ok: true; disconnected: boolean }>(`/api/mcp/servers/${s.id}/oauth`)
    } catch (e) {
      error = errorMessage(e)
    }
    await refresh()
  }

  const remove = async () => {
    const ok = await confirm({
      title: 'Remove MCP server',
      message: `Remove "${s.label}" org-wide? Every agent that carries it loses it within about a minute, without a restart.`,
      confirmLabel: 'Remove',
    })
    if (!ok) return
    try {
      await delJson<{ ok: true }>(`/api/mcp/servers/${s.id}`)
    } catch (e) {
      error = errorMessage(e)
      return
    }
    await refresh()
  }

  const external = $derived(!isInternalServer(s))
  // The deployment's own servers are governed, never edited: Manage is all
  // their menu holds.
  const menu = (): ContextMenuEntry[] => [
    { label: 'Manage', onSelect: () => (manageOpen = true) },
    ...(s.builtin ? [] : settingsMenu()),
  ]
  const settingsMenu = (): ContextMenuEntry[] => [
    ...(external && !s.package && s.authMode === 'org'
      ? [{ label: s.headerKeys?.length ? 'Edit credentials' : 'Set credentials', onSelect: () => (credentialsOpen = true) }]
      : []),
    ...(external && !s.package
      ? [{ label: 'Detect sign-in method again', onSelect: () => ask({ redetectAuth: true, refreshTools: true }) }]
      : []),
    ...(s.oauthEnabled && s.oauthMeta?.clientSet ? [{ label: 'OAuth app settings', onSelect: () => (oauthOpen = true) }] : []),
    { label: s.enabled ? 'Disable server' : 'Enable server', onSelect: () => void patch({ enabled: !s.enabled }) },
    {
      label: s.authMode === 'org' ? 'Switch to per-user sign-in' : 'Switch to one org account',
      onSelect: () => void patch({ authMode: s.authMode === 'org' ? 'per-user' : 'org' }),
    },
    ...(s.oauthEnabled && s.authMode === 'org' && s.orgConnected
      ? [{ label: 'Reconnect org account', onSelect: () => void connectPopup(s.id, 'org') }]
      : []),
    // The org's connection is the workspace's credential: admins only, and
    // the route refuses anyone else whatever this menu shows.
    ...(s.oauthEnabled && s.orgConnected && session.data?.role === 'admin'
      ? [{ label: 'Disconnect org account', danger: true, onSelect: () => void disconnectOrg() }]
      : []),
    'sep',
    { label: 'Remove server', danger: true, onSelect: () => void remove() },
  ]
</script>

{#snippet purpose()}
  <span class="block truncate">{s.description || domain}</span>
{/snippet}

<div data-mcp-server={s.name}>
  <SettingsRow
    label={s.label}
    hint={s.description || domain ? purpose : undefined}
    onOpen={() => (manageOpen = true)}
    openLabel={`Manage ${s.label}`}
    class={cn((!s.enabled || s.appOff) && 'opacity-60')}
  >
    {#snippet lead()}<McpServerMark title={s.label} domain={markDomains} />{/snippet}
    {#snippet meta()}
      <!-- -my-px: a 22px chip must not make its row 2px taller than a row without one. -->
      {#if s.appSlug}
        <Chip class="-my-px" title={`Published by the "${s.appSlug}" app. Its tools run inside this deployment, and it follows the app on the Apps page.`}>App</Chip>
      {:else if s.package}
        <Chip class="-my-px" title={`Runs as a hardened container from a ${s.package.kind.toUpperCase()} package. One org credential, sealed at rest and spoken only to the container.`}>
          Package
        </Chip>
      {/if}
      {#if s.authMode === 'per-user' && !s.builtin}
        <Chip class="-my-px" title="Each person connects their own account. Until they do, their assistant is told where to connect.">Per user</Chip>
      {/if}
      {#if stale.length && s.enabled}
        <Chip class="-my-px" tone="warn" title={`${count(stale.length, 'agent')} named tools this server no longer offers. Manage shows which.`}>Tools changed</Chip>
      {/if}
    {/snippet}
    {#snippet status()}
      <!-- A fixed width, so the dot after it starts at one place down the list. -->
      <span class="hidden w-40 truncate lg:block">{use}</span>
      <span class="flex items-center gap-2" title={verdict.detail ?? undefined}>
        {#if checking}
          <WaitingMark site="mcp/server-refresh" size={12} />Checking
        {:else}
          <StatusDot status={STATUS_DOT[verdict.tone]} />{verdict.label}
        {/if}
      </span>
    {/snippet}

    <!-- One width for every fix, so the buttons and the kebabs after them end on one line down the list. -->
    {#if fix}<Button variant="outline" size="sm" class="w-32" title={fix.title} disabled={checking} onclick={fix.run}>{fix.label}</Button>{/if}
    <SettingsRowMenu label={`Actions for ${s.label}`} items={menu} />
  </SettingsRow>
  {#if error}<div class="px-5 pb-4 font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}

  {#if manageOpen}<McpServerModal server={s} onClose={() => (manageOpen = false)} onRemove={s.builtin ? undefined : () => void remove()} />{/if}
  {#if credentialsOpen}<McpCredentialsModal server={s} onClose={() => (credentialsOpen = false)} />{/if}
  {#if oauthOpen && s.oauthMeta}
    <McpOauthAppSetup
      serverId={s.id}
      serverLabel={s.label}
      meta={s.oauthMeta}
      onSaved={() => {
        oauthOpen = false
        void refresh()
      }}
      onClose={() => (oauthOpen = false)}
    />
  {/if}
</div>
