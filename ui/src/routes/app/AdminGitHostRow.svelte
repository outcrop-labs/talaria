<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import type { Snippet } from 'svelte'
  import { publicIconDomain } from '@/lib/icon-domain'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import FactsList from '@/components/admin/FactsList.svelte'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import type { Fact } from '@/components/admin/facts'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { ChipTone } from '@/components/ui/chip'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useClock } from '@/lib/clock.svelte'
  import { delJson, errorMessage, getJsonOr404, postJson, putJson } from '@/lib/fetch-json'
  import { gitHostAgentsKey, unclaimedKeys, unclaimedSentence, type HostAgents } from '@/lib/git-host-agents'
  import { GITHUB_ACCESS_HREF } from '@/lib/github-access'
  import { checkedAgo, installationDiff, permissionLabel, type PermissionEntry } from '@/lib/git-permissions'
  import { slide } from '@/lib/motion'
  import { useWorkspaceSecrets } from '@/lib/workspace-secrets'
  import AdminGithubConnectModal from './AdminGithubConnectModal.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import {
    gitHostKey,
    HOST_COLS,
    hostInUse,
    hostState,
    inUseSummary,
    useGitHost,
    useGitHostsRefresh,
    type GitHost,
    type GitHostInstallation,
    type GitHostTest,
  } from './git-hosts'

  /** One git host in Admin → Git hosts: what it is and whether its connection
   *  answers right now. Its settings and its connection open as dialogs.
   *  "Test connection" asks the host live who the connection is and which
   *  repos it reaches. A host that a grant, a job, a request or a plugin still
   *  names cannot be removed, and its dialog says what holds it. A GitHub App
   *  host's dialog also lists, per installation, what the App asks for, what
   *  the installation has accepted, and what is still waiting for an org
   *  owner on GitHub — the row's status turns gold while anything is. What an
   *  installation accepted is a fact, drawn neutral, never as a grant: it is
   *  the most an admin may grant an agent, and no agent's token carries any
   *  of it until an admin does, in Agent policy › GitHub access (linked
   *  here). What the installation accepted that no agent's set names is said
   *  as such. */
  let { host }: { host: GitHost } = $props()

  const live = useGitHost(() => host.id)
  const refresh = useGitHostsRefresh()
  const qc = useQueryClient()
  const clock = useClock()
  const current = $derived(live.data ?? host)
  const status = $derived(live.data?.status)
  const url = $derived(`/api/workbench/git-hosts/${encodeURIComponent(host.id)}`)

  // The App's ask, read live with the status, is what each installation is
  // measured against; the installation's own last-read copy stands in when
  // the live read failed. So a permission an admin just added to the App
  // shows as waiting at once, not at the next ten-minute refresh.
  const askOf = (inst: GitHostInstallation) => status?.appPermissions ?? inst.appPermissions
  const installations = $derived(
    (status?.installations ?? []).map((inst) => ({ inst, diff: installationDiff(askOf(inst), inst.held) })),
  )
  const waitingCount = $derived(installations.filter((i) => i.diff.waiting.length > 0).length)
  const unknownCount = $derived(installations.filter((i) => !i.diff.known || i.inst.error).length)
  const permissionsNote = $derived(
    [waitingCount > 0 ? `${waitingCount} waiting for org-owner approval` : null, unknownCount > 0 ? `${unknownCount} not checked` : null]
      .filter(Boolean)
      .join(' · '),
  )

  let refreshing = $state(false)
  // Read the App and every installation again now (what the scheduler does
  // every ten minutes); the answer is the host as GET draws it.
  const refreshHoldings = async () => {
    refreshing = true
    error = null
    try {
      const fresh = await postJson<{ host: GitHost }>(`${url}/refresh`)
      qc.setQueryData(gitHostKey(host.id), fresh.host)
      await qc.invalidateQueries({ queryKey: gitHostAgentsKey(host.id) })
    } catch (e) {
      error = errorMessage(e)
    }
    refreshing = false
  }

  let open = $state(false)
  // What the installation accepted that no agent's set names, read while the
  // dialog is open. A server without the route answers 404: nothing is said.
  const agents = createQuery(() => ({
    queryKey: gitHostAgentsKey(host.id),
    queryFn: () => getJsonOr404<HostAgents>(`${url}/agents`),
    enabled: open && current.kind === 'github',
  }))
  const unclaimed = $derived(unclaimedSentence(unclaimedKeys(agents.data)))
  let connecting = $state(false)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let tested = $state<GitHostTest | null>(null)
  let testing = $state(false)

  // Seed-once drafts, re-seeded each time the dialog opens.
  let name = $state('')
  let baseUrl = $state('')
  let apiUrl = $state('')
  let orgs = $state('')
  let secret = $state('')
  const edit = () => {
    name = current.name
    baseUrl = current.baseUrl
    apiUrl = current.apiUrl ?? ''
    orgs = current.repoCreationOrgs.join(', ')
    secret = current.secret ?? ''
    error = null
    open = true
  }

  const secrets = useWorkspaceSecrets()
  // A host authenticates with a workspace credential whose Spendable-at list
  // names it; the route checks again, this just narrows the picker.
  const hostname = $derived.by(() => {
    try {
      return new URL(current.baseUrl).hostname.toLowerCase()
    } catch {
      return ''
    }
  })
  const eligible = $derived(
    (secrets.data?.secrets ?? []).filter((s) =>
      s.allowedHosts.some((h) => {
        const a = h.trim().toLowerCase().replace(/^\./, '')
        return a !== '' && (hostname === a || hostname.endsWith(`.${a}`))
      }),
    ),
  )

  const save = async () => {
    busy = true
    error = null
    const body: Record<string, unknown> = {
      name: name.trim(),
      baseUrl: baseUrl.trim(),
      apiUrl: apiUrl.trim() || null,
      repoCreationOrgs: orgs
        .split(',')
        .map((o) => o.trim())
        .filter(Boolean),
    }
    if (current.kind === 'forgejo') body.secret = secret.trim() || null
    try {
      await putJson(url, body)
      open = false
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh(host.id)
  }

  const test = async () => {
    testing = true
    error = null
    try {
      tested = await postJson<GitHostTest>(`${url}/test`)
    } catch (e) {
      error = errorMessage(e)
      tested = null
    }
    testing = false
    await refresh(host.id)
  }

  const remove = async () => {
    if (!(await confirm({ title: `Remove ${current.name}`, message: 'Its per-repo PR bases go with it. Agents keep nothing on it.', confirmLabel: 'Remove', danger: true }))) return
    try {
      await delJson(url)
    } catch (e) {
      error = errorMessage(e)
    }
    await refresh()
  }

  const using = $derived(inUseSummary(host))
  // The one thing worth a second clause on the row: why it is failing, or who
  // the connection acts as.
  // The kind is a fact every host has, so it is quiet text, and left out where the name already says it.
  const kindSays = $derived(current.kindLabel.toLowerCase() !== current.name.trim().toLowerCase())
  const detail = $derived(live.isError ? errorMessage(live.error) : (status?.error ?? status?.account ?? null))
  const read = $derived(hostState(live.isPending ? 'pending' : live.isError ? 'failed' : 'ok', status))
  // Connected, with a permission the App asks for that an org owner has not
  // accepted yet: tokens are minted without it until they do.
  const conn = $derived(read.status === 'ok' && waitingCount > 0 ? { ...read, status: 'warn' as const, word: 'Approval waiting' } : read)
  const connTitle = $derived(
    conn === read ? undefined : 'Connected. A GitHub App permission change is waiting for an org owner to accept it on GitHub.',
  )
  // A GitHub host is connected in its own dialog. A Forgejo host's token is
  // one of its settings, so the same fix opens those.
  const fix = $derived(conn.fix === 'reconnect' ? 'Reconnect' : conn.fix === 'connect' ? 'Connect' : null)
  const openFix = () => {
    if (current.kind === 'github') connecting = true
    else edit()
  }
  // The host's own mark: GitHub's for any GitHub host, and a self-hosted
  // one's own favicon when its address is public.
  const markDomain = $derived(current.kind === 'github' ? 'github.com' : publicIconDomain(hostname))
  const menu = $derived.by((): ContextMenuEntry[] => [
    { label: 'Edit host', onSelect: edit },
    { label: 'Test connection', disabled: testing, onSelect: () => void test() },
    ...(fix ? [{ label: fix, onSelect: openFix }] : []),
    ...(!fix && current.kind === 'github' ? [{ label: 'Change connection', onSelect: () => (connecting = true) }] : []),
    'sep',
    { label: 'Remove host', danger: true, disabled: hostInUse(host), onSelect: () => void remove() },
  ])
  const facts = $derived.by((): Fact[] => [
    { label: 'Id', value: current.id, mono: true },
    {
      label: 'Connection',
      value: live.isError
        ? errorMessage(live.error)
        : status?.configured && !status.error
          ? `Connected${status.account ? ` as ${status.account}` : ''}`
          : (status?.error ?? 'Not connected'),
    },
    ...(current.kind === 'github' && current.github?.mode
      ? [{ label: 'Method', value: current.github.mode === 'app' ? 'GitHub App' : 'Access token' }]
      : []),
    { label: 'In use by', value: using || 'Nothing, so it can be removed' },
  ])
</script>

{#snippet permissionRow(label: string, entries: PermissionEntry[], tone: ChipTone, tail?: Snippet)}
  <!-- The label above its chips: a dialog is too narrow for a label column. -->
  <div class="space-y-1">
    <div class="text-muted">{label}</div>
    <div class="flex flex-wrap items-center gap-1">
      {#each entries as e (e.key)}<Chip {tone}>{permissionLabel(e)}</Chip>{:else}<span class="text-muted">Nothing</span>{/each}
      {@render tail?.()}
    </div>
  </div>
{/snippet}

<div data-testid="git-host">
  <SettingsRow label={current.name} onOpen={edit} openLabel={`Edit ${current.name}`}>
    {#snippet lead()}
      <McpServerMark title={current.name} domain={markDomain} />
    {/snippet}
    {#snippet meta()}
      {#if kindSays}<span class="shrink-0 font-sans text-[13px] font-normal leading-5 text-muted">{current.kindLabel}</span>{/if}
    {/snippet}
    {#snippet hint()}
      <span class="font-mono text-xs">{current.baseUrl}</span>{#if detail}{' · '}{detail}{/if}
    {/snippet}
    <span class={HOST_COLS.state}>
      <RowStatus status={testing ? 'idle' : conn.status} title={testing ? undefined : connTitle}>{testing ? 'Testing' : conn.word}</RowStatus>
    </span>
    <SettingsRowMenu label={`Actions for ${current.name}`} items={menu} />
  </SettingsRow>
  {#if tested}
    <div transition:slide={{ duration: 150 }}>
      <NoticeRow status={tested.ok ? 'ok' : 'danger'} class="border-t border-line-subtle" data-testid="git-host-test">
        {#if tested.ok}
          Reached {current.name} as {tested.status.account ?? 'the configured login'}.
          {tested.repoCount} repo{tested.repoCount === 1 ? '' : 's'} reachable{tested.repos && tested.repos.length > 0
            ? `: ${tested.repos.join(', ')}${(tested.repoCount ?? 0) > tested.repos.length ? ', and more' : ''}`
            : ''}.
        {:else}
          Could not use {current.name}: {tested.reposError ?? tested.status.error ?? 'the connection is not configured'}
        {/if}
      </NoticeRow>
    </div>
  {/if}
  {#if error && !open}
    <div transition:slide={{ duration: 150 }}>
      <NoticeRow status="danger" class="border-t border-line-subtle">{error}</NoticeRow>
    </div>
  {/if}

  <Modal {open} onClose={() => (open = false)} title={current.name} width="max-w-lg">
    <div class="space-y-5">
      <FactsList {facts} class="p-0" />
      {#if installations.length > 0}
        <section class="space-y-3 border-t border-line-subtle pt-5" data-testid="git-host-permissions">
          <div class="flex items-center gap-2">
            <h3 class="font-sans text-sm font-medium leading-5 text-fg">Permissions</h3>
            <InfoTip
              text="Add a permission to the App on GitHub and every installation's org owners are asked to accept it. Until they do it shows here as waiting, and tokens are minted without it."
            />
            <span class="flex-1"></span>
            <Button size="sm" variant="outline" disabled={refreshing} onclick={() => void refreshHoldings()}>
              {refreshing ? 'Refreshing…' : 'Refresh'}
            </Button>
          </div>
          {#if permissionsNote}<p class="font-sans text-[13px] font-medium leading-5 text-fg">{permissionsNote[0]?.toUpperCase()}{permissionsNote.slice(1)}.</p>{/if}
          <p class="font-sans text-[13px] leading-5 text-muted">
            What this host's GitHub App holds is the most an admin may grant an agent. Talaria hands each agent only what an admin grants it on
            this host. Read again every ten minutes, and whenever GitHub refuses a token.
          </p>
          {#each installations as { inst, diff } (inst.id)}
            <div class="space-y-2 rounded-md border border-line-subtle px-3 py-2.5 font-sans text-[13px] leading-5">
              <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span class="font-medium text-fg">{inst.account || `Installation ${inst.id}`}</span>
                {#if inst.account}<span class="font-mono text-xs text-muted">{inst.id}</span>{/if}
                {#if inst.repositorySelection}<Chip>{inst.repositorySelection === 'all' ? 'All repositories' : 'Selected repositories'}</Chip>{/if}
                {#if inst.suspendedAt}<Chip tone="danger">Suspended</Chip>{/if}
                <span class="text-muted sm:ml-auto">{checkedAgo(inst.checkedAt, clock.now)}</span>
              </div>
              {#if inst.error}<p class="text-warning">The last check failed: {inst.error}</p>{/if}
              {#if diff.known}
                {@render permissionRow('App asks for', diff.asked, 'neutral')}
                <!-- A fact, never a grant: neutral, not the success tone. -->
                {@render permissionRow('Installation accepted (what an admin may grant an agent)', diff.accepted, 'neutral')}
                {#if diff.waiting.length > 0}
                  {#snippet review()}
                    {#if inst.url}
                      <a href={inst.url} target="_blank" rel="noreferrer" class="ml-1 text-accent hover:underline">An org owner accepts it on GitHub ↗</a>
                    {/if}
                  {/snippet}
                  {@render permissionRow('Waiting for org-owner approval', diff.waiting, 'warn', review)}
                {/if}
              {:else}
                <p class="text-muted">Not checked yet, so what this installation accepted is unknown. Refresh reads it now.</p>
              {/if}
            </div>
          {/each}
          <!-- The default set (contents, pull requests, metadata) is on every
               Developer agent's token without a grant; nothing else is. -->
          <p class="font-sans text-[13px] leading-5 text-muted">
            Beyond the default set every Developer agent starts with (contents, pull requests, metadata), no agent's token carries any of this until an admin grants it to that
            agent.
          </p>
          {#if unclaimed}
            <p class="font-sans text-[13px] leading-5 text-warning" data-testid="git-host-unclaimed">{unclaimed}</p>
          {/if}
          {#if agents.isError}
            <!-- A server without the route answers 404, drawn as nothing; any
                 other failure is said, never silent. -->
            <p class="flex items-start gap-2 font-sans text-[13px] leading-5 text-muted" data-testid="git-host-agents-error">
              <StatusDot status="warn" class="mt-[7px]" />
              <span class="min-w-0">Could not read which agents are granted these: {errorMessage(agents.error)}</span>
            </p>
          {/if}
          <p class="font-sans text-[13px] leading-5 text-muted" data-testid="git-host-access-link">
            Each agent's GitHub access: <a href={GITHUB_ACCESS_HREF} class="text-accent hover:underline" onclick={() => (open = false)}>Agent policy › GitHub access ›</a>
          </p>
        </section>
      {/if}
      <div class="space-y-5 border-t border-line-subtle pt-5">
        <SettingsField label="Name" for="git-host-name-{host.id}">
          <Input id="git-host-name-{host.id}" size="sm" bind:value={name} maxlength={80} />
        </SettingsField>
        <SettingsField label="Base URL" for="git-host-url-{host.id}">
          <Input id="git-host-url-{host.id}" size="sm" bind:value={baseUrl} class="font-mono" />
        </SettingsField>
        <SettingsField label="API URL" for="git-host-api-{host.id}" hint="Optional, and worked out from the base URL when left blank.">
          <Input
            id="git-host-api-{host.id}"
            size="sm"
            bind:value={apiUrl}
            class="font-mono"
            placeholder={current.kind === 'github' ? 'https://api.github.com' : `${current.baseUrl}/api/v1`}
          />
        </SettingsField>
        <SettingsField
          label="Organizations for new repositories"
          for="git-host-orgs-{host.id}"
          hint="Where agents may ask for new repositories, separated by commas, or empty for none."
        >
          <Input id="git-host-orgs-{host.id}" size="sm" bind:value={orgs} class="font-mono" placeholder="acme, acme-labs" />
        </SettingsField>
        {#if current.kind === 'forgejo'}
          <SettingsField
            label="Access token"
            for="git-host-secret-{host.id}"
            hint="A workspace secret that may be used at {hostname || 'this host'}."
            info="The secret holds a token, and optionally its username, with no use limit. Give the token to a bot account that sees only the repositories agents work on."
          >
            <Select id="git-host-secret-{host.id}" size="sm" bind:value={secret} class="w-full">
              <option value="">None, public repositories only</option>
              {#each eligible as s (s.id)}<option value={s.name}>{s.title} ({s.name})</option>{/each}
              {#if secret && !eligible.some((s) => s.name === secret)}<option value={secret}>{secret}</option>{/if}
            </Select>
          </SettingsField>
        {/if}
        {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
      </div>
    </div>
    {#snippet footer()}
      <div class="flex items-center gap-2">
        <DangerLink
          disabled={hostInUse(host)}
          onClick={() => {
            open = false
            void remove()
          }}
        >
          Remove host
        </DangerLink>
        <span class="flex-1"></span>
        <Button size="sm" variant="ghost" onclick={() => (open = false)}>Cancel</Button>
        <Button size="sm" disabled={busy || !name.trim() || !baseUrl.trim()} onclick={() => void save()}>Save host</Button>
      </div>
    {/snippet}
  </Modal>
  {#if current.kind === 'github'}
    <AdminGithubConnectModal host={current} open={connecting} onClose={() => (connecting = false)} />
  {/if}
</div>
