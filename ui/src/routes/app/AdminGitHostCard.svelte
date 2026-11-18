<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import type { Snippet } from 'svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { ChipTone } from '@/components/ui/chip'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { useClock } from '@/lib/clock.svelte'
  import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { checkedAgo, installationDiff, permissionLabel, type PermissionEntry } from '@/lib/git-permissions'
  import { slide } from '@/lib/motion'
  import { useWorkspaceSecrets } from '@/lib/workspace-secrets'
  import AdminGithubHostFields from './AdminGithubHostFields.svelte'
  import { gitHostKey, hostInUse, useGitHost, useGitHostsRefresh, type GitHost, type GitHostInstallation, type GitHostTest } from './git-hosts'

  /** One git host in Admin → Git hosts: what it is, whether its connection
   *  answers right now, and — opened — its settings. "Test connection" asks
   *  the host live who the connection is and which repos it reaches. A host
   *  that a grant, a job, a request or a plugin still names cannot be
   *  removed, and the card says what holds it. A GitHub App host also lists,
   *  per installation, what the App asks for, what the installation has
   *  accepted, and what is still waiting for an org owner on GitHub — the
   *  dot turns gold while anything is. */
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
  const permissionsTitle = $derived(
    ['Permissions', waitingCount > 0 ? `${waitingCount} waiting for org-owner approval` : null, unknownCount > 0 ? `${unknownCount} not checked` : null]
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
    } catch (e) {
      error = errorMessage(e)
    }
    refreshing = false
  }

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
  const dot = $derived(
    !status ? 'idle' : status.configured && !status.error ? (waitingCount > 0 ? 'warn' : 'ok') : status.configured ? 'danger' : 'warn',
  )
  const dotTitle = $derived(
    status?.error ??
      (!status?.configured ? 'not connected' : waitingCount > 0 ? 'connected — a GitHub App permission change is waiting for org-owner approval' : 'connected'),
  )
</script>

{#snippet permissionRow(label: string, entries: PermissionEntry[], tone: ChipTone, tail?: Snippet)}
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
    <span class="w-44 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
    {#each entries as e (e.key)}<Chip {tone}>{permissionLabel(e)}</Chip>{:else}<span class="text-muted">nothing</span>{/each}
    {@render tail?.()}
  </div>
{/snippet}

<Panel as="article" class="space-y-3 p-3 sm:p-4" data-testid="git-host">
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1.5">
    <StatusDot status={dot} title={dotTitle} />
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
  {#if installations.length > 0}
    <Disclosure title={permissionsTitle}>
      <div class="space-y-3 text-xs" data-testid="git-host-permissions">
        <div class="flex flex-wrap items-center gap-2 text-muted">
          <span>What this host's GitHub App holds is the most any token can carry. Talaria hands each agent only what an admin grants it on this host.</span>
          <InfoTip
            text="Add a permission to the App on GitHub and every installation's org owners are asked to accept it; until they do it shows here as waiting, and tokens are minted without it."
          />
        </div>
        {#each installations as { inst, diff } (inst.id)}
          <div class="space-y-1.5 rounded-md border border-line-subtle px-2.5 py-2">
            <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span class="font-mono text-fg">{inst.account || `installation ${inst.id}`}</span>
              {#if inst.account}<span class="font-mono text-[11px] text-muted">#{inst.id}</span>{/if}
              {#if inst.repositorySelection}<Chip>{inst.repositorySelection === 'all' ? 'all repos' : 'selected repos'}</Chip>{/if}
              {#if inst.suspendedAt}<Chip tone="danger">suspended</Chip>{/if}
              <span class="text-[11px] text-muted sm:ml-auto">{checkedAgo(inst.checkedAt, clock.now)}</span>
            </div>
            {#if inst.error}<div class="text-warning">The last check failed: {inst.error}</div>{/if}
            {#if diff.known}
              {@render permissionRow('App asks for', diff.asked, 'neutral')}
              {@render permissionRow('Installation accepted', diff.accepted, 'success')}
              {#if diff.waiting.length > 0}
                {#snippet review()}
                  {#if inst.url}
                    <a href={inst.url} target="_blank" rel="noreferrer" class="text-accent hover:underline">an org owner accepts it on GitHub</a>
                  {/if}
                {/snippet}
                {@render permissionRow('Waiting for org-owner approval', diff.waiting, 'warn', review)}
              {/if}
            {:else}
              <div class="text-muted">Not checked yet: what this installation accepted is unknown. Refresh reads it now.</div>
            {/if}
          </div>
        {/each}
        <div class="flex flex-wrap items-center gap-2">
          <Button size="xs" variant="outline" disabled={refreshing} onclick={() => void refreshHoldings()}>{refreshing ? 'Refreshing…' : 'Refresh'}</Button>
          <span class="text-[11px] text-muted">Read again every ten minutes, and whenever GitHub refuses a token.</span>
        </div>
      </div>
    </Disclosure>
  {/if}
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
