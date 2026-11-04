<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { useWorkspaceSecrets } from '@/lib/workspace-secrets'
  import AdminGithubHostFields from './AdminGithubHostFields.svelte'
  import { hostInUse, useGitHost, useGitHostsRefresh, type GitHost, type GitHostTest } from './git-hosts'

  /** One git host in Admin → Git hosts: what it is, whether its connection
   *  answers right now, and — opened — its settings. "Test connection" asks
   *  the host live who the connection is and which repos it reaches. A host
   *  that a grant, a job, a request or a plugin still names cannot be
   *  removed, and the card says what holds it. */
  let { host }: { host: GitHost } = $props()

  const live = useGitHost(() => host.id)
  const refresh = useGitHostsRefresh()
  const current = $derived(live.data ?? host)
  const status = $derived(live.data?.status)
  const url = $derived(`/api/workbench/git-hosts/${encodeURIComponent(host.id)}`)

  let open = $state(false)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let tested = $state<GitHostTest | null>(null)
  let testing = $state(false)

  // Seed-once drafts, re-seeded each time the card opens.
  let name = $state('')
  let baseUrl = $state('')
  let apiUrl = $state('')
  let orgs = $state('')
  let secret = $state('')
  const seed = () => {
    name = current.name
    baseUrl = current.baseUrl
    apiUrl = current.apiUrl ?? ''
    orgs = current.repoCreationOrgs.join(', ')
    secret = current.secret ?? ''
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

  const using = $derived.by(() => {
    const u = host.inUse
    if (!u) return ''
    const parts: string[] = []
    if (u.grants) parts.push(`${u.grants} grant${u.grants === 1 ? '' : 's'}`)
    if (u.jobs) parts.push(`${u.jobs} job${u.jobs === 1 ? '' : 's'}`)
    if (u.repoRequests) parts.push(`${u.repoRequests} repo request${u.repoRequests === 1 ? '' : 's'}`)
    if (u.installRequests) parts.push(`${u.installRequests} install request${u.installRequests === 1 ? '' : 's'}`)
    if (u.plugins) parts.push(`${u.plugins} plugin${u.plugins === 1 ? '' : 's'}`)
    return parts.join(' · ')
  })
  const dot = $derived(!status ? 'idle' : status.configured && !status.error ? 'ok' : status.configured ? 'danger' : 'warn')
</script>

<Panel as="article" class="space-y-3 p-3 sm:p-4" data-testid="git-host">
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1.5">
    <StatusDot status={dot} title={status?.error ?? (status?.configured ? 'connected' : 'not connected')} />
    <span class="min-w-0 truncate text-sm font-medium text-fg">{current.name}</span>
    <Chip>{current.kindLabel}</Chip>
    <span class="font-mono text-[11px] text-muted">{current.id}</span>
    <span class="flex w-full flex-wrap items-center gap-1.5 sm:ml-auto sm:w-auto">
      <Button size="xs" variant="outline" disabled={testing} onclick={() => void test()}>{testing ? 'Testing…' : 'Test connection'}</Button>
      <Button
        size="xs"
        variant="ghost"
        onclick={() => {
          if (!open) seed()
          open = !open
        }}>{open ? 'Close' : 'Edit'}</Button
      >
      <Button
        size="xs"
        variant="ghost"
        class="hover:text-danger"
        disabled={hostInUse(host)}
        title={hostInUse(host) ? `In use: ${using}` : 'Remove this host'}
        onclick={() => void remove()}>Remove</Button
      >
    </span>
  </div>
  <div class="min-w-0 space-y-0.5 text-xs">
    <div class="truncate font-mono text-muted">{current.baseUrl}{current.apiUrl ? ` · API ${current.apiUrl}` : ''}</div>
    {#if live.isPending}
      <div class="text-muted">checking the connection…</div>
    {:else if live.isError}
      <div class="text-warning">{errorMessage(live.error)}</div>
    {:else if status?.configured && !status.error}
      <div class="text-fg">Connected{status.account ? ` as ${status.account}` : ''}{current.kind === 'github' && current.github?.mode ? ` · ${current.github.mode === 'app' ? 'GitHub App' : 'access token'}` : ''}</div>
    {:else if status?.error}
      <div class="text-warning">{status.error}</div>
    {:else}
      <div class="text-muted">Not connected{current.kind === 'github' ? ' — pick a method below (Edit)' : ' — name a workspace secret (Edit)'}.</div>
    {/if}
    {#if current.repoCreationOrgs.length > 0}
      <div class="text-muted">Agents may request new repos in <span class="font-mono text-fg">{current.repoCreationOrgs.join(', ')}</span></div>
    {/if}
    {#if using}<div class="text-muted">In use: {using}</div>{/if}
  </div>
  {#if tested}
    <div transition:slide={{ duration: 150 }} class="rounded-md border border-line-subtle bg-raised/40 px-3 py-2 text-xs" data-testid="git-host-test">
      {#if tested.ok}
        <span class="text-success">Reached {current.name}</span>
        <span class="text-muted">
          as {tested.status.account ?? 'the configured login'} · {tested.repoCount} repo{tested.repoCount === 1 ? '' : 's'} reachable{tested.repos && tested.repos.length > 0 ? `: ${tested.repos.join(', ')}${(tested.repoCount ?? 0) > tested.repos.length ? ', …' : ''}` : ''}
        </span>
      {:else}
        <span class="text-danger">Could not use {current.name}:</span>
        <span class="text-muted">{tested.reposError ?? tested.status.error ?? 'the connection is not configured'}</span>
      {/if}
    </div>
  {/if}

  {#if open}
    <div transition:slide={{ duration: 150 }} class="space-y-3 border-t border-line-subtle pt-3">
      <div class="grid gap-2 sm:grid-cols-2">
        <label class="space-y-1 text-xs text-muted">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</span>
          <Input size="sm" bind:value={name} maxlength={80} />
        </label>
        <label class="space-y-1 text-xs text-muted">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Base URL</span>
          <Input size="sm" bind:value={baseUrl} class="font-mono" />
        </label>
        <label class="space-y-1 text-xs text-muted">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">API URL (optional)</span>
          <Input size="sm" bind:value={apiUrl} class="font-mono" placeholder={current.kind === 'github' ? 'derived: api.github.com or <server>/api/v3' : 'derived: <base>/api/v1'} />
        </label>
        <label class="space-y-1 text-xs text-muted">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repo creation orgs</span>
          <Input size="sm" bind:value={orgs} placeholder="comma-separated (empty = off)" />
        </label>
        {#if current.kind === 'forgejo'}
          <label class="space-y-1 text-xs text-muted sm:col-span-2">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Token (workspace secret)</span>
            <Select size="sm" bind:value={secret} class="w-full">
              <option value="">none — public repositories only</option>
              {#each eligible as s (s.id)}<option value={s.name}>{s.title} ({s.name})</option>{/each}
              {#if secret && !eligible.some((s) => s.name === secret)}<option value={secret}>{secret}</option>{/if}
            </Select>
            <span class="block text-[11px]">
              A workspace secret whose Spendable-at list names {hostname || 'this host'}, holding a token (and optionally its username), with no use limit. Give the token to a bot account that sees only the repositories agents work on.
            </span>
          </label>
        {/if}
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={busy || !name.trim() || !baseUrl.trim()} onclick={() => void save()}>Save</Button>
        <Button size="sm" variant="ghost" onclick={() => (open = false)}>Cancel</Button>
      </div>
      {#if current.kind === 'github'}
        <AdminGithubHostFields host={current} />
      {/if}
    </div>
  {/if}
  {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
</Panel>
