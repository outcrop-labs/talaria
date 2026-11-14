<script lang="ts">
  import { publicIconDomain } from '@/lib/icon-domain'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import FactsList from '@/components/admin/FactsList.svelte'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import type { Fact } from '@/components/admin/facts'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { useWorkspaceSecrets } from '@/lib/workspace-secrets'
  import AdminGithubConnectModal from './AdminGithubConnectModal.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { HOST_COLS, hostInUse, hostState, inUseSummary, useGitHost, useGitHostsRefresh, type GitHost, type GitHostTest } from './git-hosts'

  /** One git host in Admin → Git hosts: what it is and whether its connection
   *  answers right now. Its settings and its connection open as dialogs.
   *  "Test connection" asks the host live who the connection is and which
   *  repos it reaches. A host that a grant, a job, a request or a plugin still
   *  names cannot be removed, and its dialog says what holds it. */
  let { host }: { host: GitHost } = $props()

  const live = useGitHost(() => host.id)
  const refresh = useGitHostsRefresh()
  const current = $derived(live.data ?? host)
  const status = $derived(live.data?.status)
  const url = $derived(`/api/workbench/git-hosts/${encodeURIComponent(host.id)}`)

  let open = $state(false)
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
  const conn = $derived(hostState(live.isPending ? 'pending' : live.isError ? 'failed' : 'ok', status))
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
    <span class={HOST_COLS.state}><RowStatus status={testing ? 'idle' : conn.status}>{testing ? 'Testing' : conn.word}</RowStatus></span>
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
