<script lang="ts">
  import { createQueries } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import AgentGithubAccess from '@/components/github-access/AgentGithubAccess.svelte'
  import { fetchGithubPermissions, usePermissionRequests, useRepoRequests } from '@/components/github-access/queries'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowAction from '@/components/settings/SettingsRowAction.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { anyPending } from '@/components/ui/query-state'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, getJsonOr404 } from '@/lib/fetch-json'
  import { useFleetDefs } from '@/lib/fleet-defs'
  import { gitHostAgentsKey, type HostAgents } from '@/lib/git-host-agents'
  import { COULD_NOT_READ, GITHUB_ACCESS_HREF, githubPermissionsKey, overviewRows, requestRows, type OverviewRow } from '@/lib/github-access'
  import { useGitHosts } from './git-hosts'

  // Agent policy › GitHub access: every agent's GitHub access in one place.
  // Without `?agent=` it is the overview: the requests that wait on an
  // admin, then every Developer agent with one word read from its own tokens
  // (the same read, under the same key, its editor makes), and above them
  // what each GitHub installation accepted that no agent's set names. With
  // `?agent=<id>` it is that agent's editor, the one its own GitHub access
  // section shows. The address is the selection: a stale link says the agent
  // is gone rather than landing somewhere else. Admins only, like every
  // Instance page.
  const agentId = $derived(String(searchParams.get('agent') ?? ''))

  const defsQuery = useFleetDefs(true)
  const defs = $derived(defsQuery.data?.defs ?? [])
  const editing = $derived(agentId ? defs.find((d) => d.id === agentId) : undefined)

  const hostsQuery = useGitHosts()
  const hosts = $derived(hostsQuery.data?.hosts ?? [])
  const requests = usePermissionRequests(() => !agentId)
  const repoRequests = useRepoRequests(() => !agentId)

  // One permissions read per Developer agent, under its editor's own key.
  const live = $derived(defs.filter((d) => d.developer && d.enabled))
  const permsQueries = createQueries(() => ({
    queries: live.map((d) => ({
      queryKey: githubPermissionsKey(d.id),
      queryFn: () => fetchGithubPermissions(d.id),
      enabled: !agentId,
    })),
  }))
  // Each host's agents read (shared with the host dialog), for two things
  // only: what its installation accepted that no agent's set names, and the
  // repo counts of agents whose read is not made. A 404 is a server without
  // the route: no notice.
  const agentsQueries = createQueries(() => ({
    queries: hosts.map((h) => ({
      queryKey: gitHostAgentsKey(h.id),
      queryFn: () => getJsonOr404<HostAgents>(`/api/workbench/git-hosts/${encodeURIComponent(h.id)}/agents`),
      enabled: !agentId,
    })),
  }))

  const overview = $derived(
    overviewRows({
      defs,
      perms: Object.fromEntries(
        live.map((d, i) => {
          const q = permsQueries[i]
          return [d.id, { data: q?.data ?? null, error: q?.isError && !q.data ? errorMessage(q.error) : null }]
        }),
      ),
      hosts: hosts.map((h, i) => ({ id: h.id, name: h.name, kind: h.kind, body: agentsQueries[i]?.data })),
      requests: requests.data ?? [],
    }),
  )
  const waiting = $derived(requestRows(requests.data ?? [], repoRequests.data ?? [], defs))
  const pendingRow = (id: string) => {
    const i = live.findIndex((d) => d.id === id)
    return i !== -1 && permsQueries[i]?.isPending === true
  }
  const failedHosts = $derived(
    hosts.flatMap((h, i) => {
      const q = agentsQueries[i]
      return q?.isError ? [{ host: h, error: q.error, retry: () => void q.refetch() }] : []
    }),
  )
</script>

{#snippet agentRow(row: OverviewRow)}
  <SettingsRow label={row.name} wrap openHref={row.href} openLabel={`${row.name}'s GitHub access`}>
    {#snippet lead()}<Avatar name={row.name} class="h-7 w-7" />{/snippet}
    {#snippet hint()}
      {@const parts = [row.hint, row.missing.length ? `not on its token: ${row.missing.map((m) => m.split(':')[0]).join(', ')}` : ''].filter(Boolean)}
      {parts.join(' · ')}{#if row.waiting > 0}{parts.length ? ' · ' : ''}<span class="text-warning">{row.waiting} request{row.waiting === 1 ? '' : 's'} waiting</span>{/if}
    {/snippet}
    {#snippet status()}
      {#if row.status}
        <RowStatus status={row.status.tone} title={row.missing.length ? `Not on its token: ${row.missing.join(', ')}` : undefined}>{row.status.word}</RowStatus>
      {:else if row.error}
        <RowStatus status="warn" title={row.error}>{COULD_NOT_READ}</RowStatus>
      {:else if pendingRow(row.id)}
        <Skeleton class="h-3 w-24 rounded-full" />
      {/if}
    {/snippet}
  </SettingsRow>
{/snippet}

{#if agentId}
  {#if defsQuery.isPending}
    <div class="space-y-8" aria-busy="true">
      <div class="flex items-center gap-3"><Skeleton class="h-8 w-8 rounded-full" /><Skeleton class="h-4 w-48 rounded-full" /></div>
      {#each [0, 1, 2] as i (i)}
        <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={2} /></div></SettingsGroup>
      {/each}
    </div>
  {:else if defsQuery.isError}
    <SettingsGroup>
      <QueryError variant="compact" error={defsQuery.error} title="Could not load the agents" onRetry={() => void defsQuery.refetch()} />
    </SettingsGroup>
  {:else if editing}
    {#key editing.id}<AgentGithubAccess def={editing} place="page" />{/key}
  {:else}
    <SettingsGroup>
      <EmptyState variant="compact" title="No such agent" hint="It may have been deleted, or the link is wrong.">
        {#snippet action()}<a href={GITHUB_ACCESS_HREF} class={buttonClasses({ variant: 'outline', size: 'sm', voice: 'sans' })}>All agents</a>{/snippet}
      </EmptyState>
    </SettingsGroup>
  {/if}
{:else}
  {#if waiting.rows.length > 0 || waiting.repoLine || (requests.isError && requests.data === undefined)}
    <SettingsSection title="Waiting on you">
      <SettingsGroup data-testid="github-access-waiting">
        {#if requests.isError && requests.data === undefined}
          <!-- A failed poll must never read as "nothing waiting". -->
          <QueryError variant="compact" error={requests.error} title="Could not load the permission requests" onRetry={() => void requests.refetch()} />
        {/if}
        {#each waiting.rows as r (r.id)}
          {#if r.kind === 'decide'}
            <SettingsRow label={r.label} hint={r.hint} wrap>
              {#snippet lead()}<Avatar name={r.agentName} class="h-7 w-7" />{/snippet}
              {#snippet status()}<RowStatus status={r.tone}>{r.word}</RowStatus>{/snippet}
              <SettingsRowAction href={r.href} label={`Review ${r.agentName}'s request`}>Review</SettingsRowAction>
            </SettingsRow>
          {:else}
            <NoticeRow status="idle">{r.text}</NoticeRow>
          {/if}
        {/each}
        {#if waiting.repoLine}
          <NoticeRow status="idle"><a href="/admin/git" class="text-accent hover:underline">{waiting.repoLine} ›</a></NoticeRow>
        {/if}
      </SettingsGroup>
    </SettingsSection>
  {/if}

  <SettingsSection
    title="Developer agents"
    description="Each agent's token carries only the permissions set here, as far as the App's installation holds them."
    info="What an installation accepted on GitHub is the most an admin may grant an agent. No agent's token carries any of it until an admin grants it to that agent. Each word is read from the agent's own tokens, never from what the installation holds."
  >
    <SettingsGroup data-testid="github-access-agents">
      {#if anyPending(defsQuery, hostsQuery)}
        <div class="px-5 py-4"><SkeletonRows rows={3} avatar /></div>
      {:else if defsQuery.isError}
        <QueryError variant="compact" error={defsQuery.error} title="Could not load the agents" onRetry={() => void defsQuery.refetch()} />
      {:else if hosts.length === 0 && !hostsQuery.isError}
        <EmptyState variant="compact" title="No git host yet" hint="Add one in Git hosts before agents can work in repositories.">
          {#snippet action()}<a href="/admin/git" class={buttonClasses({ variant: 'outline', size: 'sm', voice: 'sans' })}>Git hosts</a>{/snippet}
        </EmptyState>
      {:else if defs.length === 0}
        <EmptyState variant="compact" title="No agents yet">
          {#snippet action()}<a href="/agents" class={buttonClasses({ variant: 'outline', size: 'sm', voice: 'sans' })}>Agents</a>{/snippet}
        </EmptyState>
      {:else}
        {#if hostsQuery.isError}
          <QueryError variant="compact" error={hostsQuery.error} title="Could not load the git hosts" onRetry={() => void hostsQuery.refetch()} />
        {/if}
        {#each overview.unclaimed as u (u.hostId)}
          <NoticeRow status="warn" data-testid="github-access-unclaimed">{u.sentence}</NoticeRow>
        {/each}
        {#each failedHosts as f (f.host.id)}
          <QueryError variant="compact" error={f.error} title={`Could not read the agents on ${f.host.name}`} onRetry={f.retry} />
        {/each}
        {#each overview.developer as row (row.id)}
          {@render agentRow(row)}
        {:else}
          <EmptyState variant="compact" title="No Developer agents yet" hint="Turn on the Developer switch on an agent below to let it work in repositories." />
        {/each}
      {/if}
    </SettingsGroup>
  </SettingsSection>

  {#if defsQuery.data && overview.others.length > 0}
    <!-- Mounted once the agents are read: its open state is read at mount. -->
    <Disclosure variant="card" title={`Agents without the Developer switch (${overview.others.length})`} defaultOpen={overview.developer.length === 0}>
      <SettingsGroup bare class="-my-4 border-y-0">
        {#each overview.others as row (row.id)}
          {@render agentRow(row)}
        {/each}
      </SettingsGroup>
    </Disclosure>
  {/if}
{/if}
