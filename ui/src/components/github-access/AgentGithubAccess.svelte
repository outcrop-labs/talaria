<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ArrowLeft } from '@lucide/svelte'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { checkRepos, type HostPermissions, type TokenCheck } from '@/lib/agent-github-permissions'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { gitHostAgentsKey } from '@/lib/git-host-agents'
  import { isDecidable } from '@/lib/git-permission-requests'
  import { accessStatus, githubAccessHref, githubPermissionsKey, GITHUB_ACCESS_HREF, requestedKeys } from '@/lib/github-access'
  import { useSession } from '@/lib/session'
  import { navigateHref } from '@/router'
  import AgentRepoGrants from './AgentRepoGrants.svelte'
  import DeveloperSwitchRow from './DeveloperSwitchRow.svelte'
  import GitPermissionRequestCard from './GitPermissionRequestCard.svelte'
  import HostPermissionList from './HostPermissionList.svelte'
  import HostTokenCard from './HostTokenCard.svelte'
  import { useGithubPermissions, usePermissionRequests, useRepoRequests } from './queries'

  // ONE editor for everything that decides what an agent can do on GitHub:
  // its open requests, the Developer switch, the repositories it may touch
  // and their branch rules, and per host the permissions its token carries,
  // what the installation holds beside them, what its tokens did carry, and
  // the workspace secret that answers git ahead of the App. It is mounted at
  // Agent policy › GitHub access (`/admin/github?agent=<id>`) and as the
  // agent's own GitHub access section, and both read and write the same
  // query keys, so the two can never disagree.
  //
  // Who does what is the server's: admins set permissions, check tokens,
  // decide requests and revoke a secret; the agent's managers switch it and
  // grant repositories and rules; anyone who may read the agent reads the
  // rest. Each section owns its read and its failure, so one failing never
  // blanks the others.
  let {
    def,
    place,
    onLeave,
  }: {
    def: AgentDef
    place: 'page' | 'modal'
    /** In the agent's dialog: close it before going to the page. */
    onLeave?: () => void
  } = $props()

  const qc = useQueryClient()
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const perms = useGithubPermissions(() => def.id)
  const requests = usePermissionRequests(() => isAdmin)
  const repoRequests = useRepoRequests(() => isAdmin)

  const status = $derived(perms.data ? accessStatus(def, perms.data) : null)
  const waiting = $derived((requests.data ?? []).filter(isDecidable).filter((r) => r.agentId === def.id))
  const repoAsks = $derived((repoRequests.data ?? []).filter((r) => r.agentId === def.id))
  // A failed read of either queue is said here, never read as "nothing waiting".
  const requestsFailed = $derived(requests.isError && requests.data === undefined)
  const repoRequestsFailed = $derived(repoRequests.isError && repoRequests.data === undefined)
  // Every flag is read before they are combined (query-state.ts).
  const showWaiting = $derived(isAdmin && [waiting.length > 0, repoAsks.length > 0, requestsFailed, repoRequestsFailed].includes(true))

  // Check token, per host: the repo picked, the hosts with a check in
  // flight, and the last answer. Shared by the permission rows (a "Not
  // checked" row offers it) and the token card. Any write that changes what
  // an answer describes drops it: a permission, a decided request, a repo
  // granted or removed, a revoked secret, the Developer switch.
  let picked = $state<Record<string, string>>({})
  let checking = $state<string[]>([])
  let checks = $state<Record<string, { repo: string; result: TokenCheck }>>({})
  const repoFor = (h: HostPermissions) => {
    const repos = checkRepos(h)
    const p = picked[h.host]
    return p && repos.includes(p) ? p : (repos[0] ?? '')
  }
  const check = async (h: HostPermissions) => {
    const repo = repoFor(h)
    if (!repo) return
    checking = [...checking, h.host]
    let result: TokenCheck
    try {
      result = await postJson<TokenCheck>(`/api/workbench/github-permissions/${def.id}/check`, { host: h.host, repo })
    } catch (e) {
      // A refusal's sentence, verbatim: the same words the agent's push gets.
      result = { ok: false, error: errorMessage(e) }
    }
    checks = { ...checks, [h.host]: { repo, result } }
    // A check that minted wrote the agent's credential row: read it back.
    await Promise.all([qc.invalidateQueries({ queryKey: githubPermissionsKey(def.id) }), qc.invalidateQueries({ queryKey: gitHostAgentsKey(h.host) })])
    checking = checking.filter((x) => x !== h.host)
  }
  /** Drop the answers on these hosts, or on every host. */
  const dropChecks = (hosts?: readonly string[]) => {
    checks = hosts ? Object.fromEntries(Object.entries(checks).filter(([h]) => !hosts.includes(h))) : {}
  }

  const leave = () => {
    onLeave?.()
    void navigateHref(githubAccessHref(def.id))
  }
</script>

<div class="space-y-8" data-testid="agent-github-access">
  {#if place === 'page'}
    <div class="space-y-3">
      <a href={GITHUB_ACCESS_HREF} class="inline-flex items-center gap-1.5 font-sans text-[13px] leading-5 text-muted hover:text-fg">
        <ArrowLeft size={14} aria-hidden="true" />All agents
      </a>
      <div class="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <Avatar name={def.displayName} class="h-8 w-8" />
        <h2 class="min-w-0 break-words font-sans text-[15px] font-semibold leading-5 text-fg">{def.displayName}</h2>
        <span class="ml-auto">
          {#if status}
            <RowStatus status={status.tone} title={status.missing.length ? `Not on its token: ${status.missing.join(', ')}` : undefined}>{status.word}</RowStatus>
          {:else if perms.isPending}
            <Skeleton class="h-3 w-24 rounded-full" />
          {/if}
        </span>
      </div>
    </div>
  {:else if isAdmin}
    <p class="font-sans text-[13px] leading-5 text-muted">
      {#if status}<span class="mr-1 inline-flex align-middle"><RowStatus status={status.tone}>{status.word}</RowStatus></span> ·{/if}
      <button type="button" class="cursor-pointer text-accent hover:underline" onclick={leave}>Every agent's GitHub access ›</button>
    </p>
  {/if}

  {#if showWaiting}
    <SettingsSection title="Waiting on you">
      <SettingsGroup>
        {#if requestsFailed}
          <!-- A failed poll must never read as "nothing waiting". -->
          <QueryError variant="compact" error={requests.error} title="Could not load the permission requests" onRetry={() => void requests.refetch()} />
        {/if}
        {#if repoRequestsFailed}
          <QueryError variant="compact" error={repoRequests.error} title="Could not load the repository requests" onRetry={() => void repoRequests.refetch()} />
        {/if}
        {#each waiting as r (r.id)}
          <GitPermissionRequestCard request={r} onDecided={() => dropChecks([r.host])} />
        {/each}
        {#each repoAsks as q (q.id)}
          <NoticeRow status="idle">
            {def.displayName} asked for a new repository, <span class="font-mono">{q.org}/{q.name}</span>. Decided in
            <a href="/admin/git" class="text-accent hover:underline">Git hosts ›</a>
          </NoticeRow>
        {/each}
      </SettingsGroup>
    </SettingsSection>
  {/if}

  <SettingsSection title="Developer">
    <SettingsGroup>
      <DeveloperSwitchRow {def} canManage={def.canManage} context="editor" onChanged={() => dropChecks()} />
    </SettingsGroup>
  </SettingsSection>

  <AgentRepoGrants agentId={def.id} agentName={def.displayName} canManage={def.canManage} onGranted={(hosts) => dropChecks(hosts)} />

  {#if perms.isPending}
    <SettingsSection title="Permissions">
      <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={4} /></div></SettingsGroup>
    </SettingsSection>
  {:else if perms.isError}
    <SettingsSection title="Permissions">
      <SettingsGroup>
        <div class="px-5 py-4">
          <QueryError variant="inline" error={perms.error} title="Could not load GitHub permissions" onRetry={() => void perms.refetch()} />
        </div>
      </SettingsGroup>
    </SettingsSection>
  {:else if perms.data}
    {@const d = perms.data}
    {#each d.hosts as h (h.host)}
      {#if h.narrowing === 'hostWide'}
        <SettingsSection title={h.name}>
          <SettingsGroup>
            <NoticeRow status="idle">{h.name}'s own token decides: {h.hostWide ?? 'it cannot be narrowed per agent'}.</NoticeRow>
          </SettingsGroup>
        </SettingsSection>
      {:else}
        {@const canCheck = d.canCheck === true && isAdmin}
        <HostPermissionList
          {def}
          data={d}
          block={h}
          {isAdmin}
          requested={requestedKeys(requests.data ?? [], def.id, h.host)}
          checking={checking.includes(h.host)}
          onCheck={canCheck && h.repos.length > 0 ? () => void check(h) : undefined}
          onWrote={() => dropChecks([h.host])}
        />
        <HostTokenCard
          block={h}
          {canCheck}
          checking={checking.includes(h.host)}
          picked={repoFor(h)}
          result={checks[h.host]}
          onPick={(repo) => (picked = { ...picked, [h.host]: repo })}
          onCheck={() => void check(h)}
        />
      {/if}
    {:else}
      <SettingsSection title="Permissions">
        <SettingsGroup>
          <p class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">Grant a repository above to set what its token carries on that host.</p>
        </SettingsGroup>
      </SettingsSection>
    {/each}
  {/if}
</div>
