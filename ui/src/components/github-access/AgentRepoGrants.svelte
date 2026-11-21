<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { MediaQuery } from 'svelte/reactivity'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSaveBar from '@/components/settings/SettingsSaveBar.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { useHasPerm } from '@/lib/session'
  import { slide } from '@/lib/motion'
  import { readOf, repoKey as key, saveRepoGrants, sameRepo as same, type HostRepo, type ReposBody, type RepoRule } from '@/lib/repo-grants'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import ProjectVariablesModal from './ProjectVariablesModal.svelte'
  import { useGithubAccessRefresh } from './queries'

  // The repositories an agent may touch, explicit per agent (the workbench
  // touches ONLY these), and each grant's BRANCH LAW: the base a human
  // merges, whether the agent may push it at all, and the prefix its own
  // branches must carry. A grant names its git host: the same owner/name on
  // two hosts is two repositories. The pre-push hook enforces every rule in
  // the container before bytes reach the remote.
  //
  // Its managers and admins edit (PUT /api/workbench/repos/{id},
  // require_agent_manager); anyone who may read the agent sees the grants
  // and rules as text. Adding or removing a repo writes at once, carrying any
  // rule edits still pending, so a grant and its rules never half-land; rule
  // edits alone gather under the save bar. Removing a grant revokes its
  // tokens and may withdraw the agent's requests on that host, so every write
  // re-reads the permissions and the requests too.
  //
  // Each write is a compare-and-set from what this read (lib/repo-grants):
  // when another manager wrote meanwhile, or an approval added a repo, the
  // server answers 409 with what is there now and writes nothing, and this
  // re-reads and says what changed. Pending rule edits were made against the
  // old read, so they go too: the toast asks for the change again.
  let {
    agentId,
    agentName,
    canManage,
    onGranted,
  }: {
    agentId: string
    agentName: string
    canManage: boolean
    /** A repo was granted or removed on these hosts: its tokens there changed. */
    onGranted?: (hosts: string[]) => void
  } = $props()

  // Project variables belong to the repository and their routes require
  // `agents.manage`, which a named manager need not hold: a door that would
  // only 403 is not drawn.
  const mayEditVariables = useHasPerm('agents.manage')

  const refresh = useGithubAccessRefresh()
  // The route's only 404 is "unknown agent", and this renders for that very
  // agent: a 404 is an anomaly, so everything throws.
  const query = createQuery(() => ({
    queryKey: ['workbench-repos', agentId],
    queryFn: (): Promise<ReposBody> => getJson<ReposBody>(`/api/workbench/repos/${agentId}`),
  }))

  // Rule edits, keyed host:repo: they ride the next PUT with the grants. A
  // write that lands clears them, so the rule controls wait while one is in
  // flight: an edit made meanwhile would be wiped unsaved.
  let edits = $state<Record<string, RepoRule>>({})
  let saving = $state(false)
  let saved = $state(false)
  const dirty = $derived(Object.keys(edits).length > 0)
  const ruleFor = (d: ReposBody, r: HostRepo): RepoRule =>
    edits[key(r)] ?? d.rules?.find((x) => same(x, r)) ?? { ...r, baseBranch: null, pushMode: 'branches_only', branchPrefix: null }

  const write = async (granted: HostRepo[], touched: HostRepo[] = granted) => {
    const d = query.data
    if (!d) return
    saving = true
    // Rules for every granted repo ride along, edited and untouched alike, so
    // the server's rows always match what this shows.
    const rules = granted.map((r) => ruleFor(d, r))
    let ok = false
    // A conflict's answer: hosts another write touched re-read too.
    let current: HostRepo[] = []
    try {
      const res = await saveRepoGrants(agentId, readOf(d), { repos: granted, rules }, (id) => hostName(d, id))
      edits = {}
      if (res.kind === 'changed') {
        current = res.current.repos
        pushToast({ title: res.title, body: res.body })
      } else {
        ok = true
        if (granted !== d.granted) onGranted?.([...new Set(touched.map((r) => r.host))])
      }
    } catch (e) {
      // The toast is the only place a failed write can be said, and the
      // re-read below restores the truth.
      toastError('Could not save the repositories', e)
    }
    await refresh(agentId, [...new Set([...granted, ...touched, ...d.granted, ...current].map((r) => r.host))])
    saving = false
    if (ok && granted === d.granted) {
      saved = true
      setTimeout(() => (saved = false), 1800)
    }
  }

  const add = (d: ReposBody, value: string) => {
    const r = d.available.find((x) => key(x) === value)
    if (r) void write([...d.granted, r], [r])
  }
  const remove = async (d: ReposBody, r: HostRepo) => {
    const ok = await confirm({
      title: 'Remove repository',
      message: `Remove ${r.repo} from ${agentName}? Its git tokens for it are revoked now, and its open permission requests on that host go if it holds no other repo there.`,
      confirmLabel: 'Remove',
      danger: true,
    })
    if (ok) void write(
      d.granted.filter((g) => !same(g, r)),
      [r],
    )
  }

  let variablesFor = $state<string | null>(null)
  const menu = (d: ReposBody, g: HostRepo): ContextMenuEntry[] => [
    ...(mayEditVariables.current ? [{ label: 'Project variables', onSelect: () => (variablesFor = g.repo) }, 'sep' as const] : []),
    { label: 'Remove repository', danger: true, disabled: saving, onSelect: () => void remove(d, g) },
  ]

  const manyHosts = (d: ReposBody) => new Set([...d.granted, ...d.available].map((r) => r.host)).size > 1
  const hostName = (d: ReposBody, id: string) => d.hosts.find((h) => h.id === id)?.name ?? id
  // The three rule controls stand beside the name when the row has room, and
  // stack at full width under it when it does not (a phone, a narrow dialog).
  const wide = new MediaQuery('min-width: 1024px')
  const PUSH_WORD: Record<RepoRule['pushMode'], string> = { branches_only: 'Branches only', free: 'May push the base' }
</script>

<SettingsSection title="Repositories" description="It touches only these. Branch rules are checked in its container on every push.">
  <SettingsGroup>
    {#if query.isPending}
      <div class="px-5 py-4"><SkeletonRows rows={2} /></div>
    {:else if query.isError}
      <QueryError variant="compact" error={query.error} title="Could not load repository grants" onRetry={() => void query.refetch()} />
    {:else if query.data}
      {@const d = query.data}
      {@const pool = d.available.filter((r) => !d.granted.some((g) => same(g, r)))}
      {#if d.available.length === 0 && d.granted.length === 0}
        <p class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">
          No repositories reachable. Connect a git host under <a href="/admin/git" class="text-accent hover:underline">Agent policy › Git hosts</a> to grant some.
        </p>
      {:else}
        {#each d.granted as g (key(g))}
          {@const rule = ruleFor(d, g)}
          {@const branches = d.branches?.[key(g)] ?? []}
          <SettingsRow wrap stacked={!wide.current} hint={manyHosts(d) ? hostName(d, g.host) : undefined}>
            {#snippet label()}<span class="font-mono text-[13px] font-normal" title={g.repo}>{g.repo}</span>{/snippet}
            {#if canManage}
              <div class="flex w-full flex-col gap-2 lg:w-auto lg:flex-row lg:items-center">
                <Select
                  size="sm"
                  class="w-full lg:w-40"
                  aria-label={`Base branch for ${g.repo}`}
                  value={rule.baseBranch ?? ''}
                  disabled={saving}
                  onchange={(e) => (edits[key(g)] = { ...rule, baseBranch: (e.currentTarget as HTMLSelectElement).value || null })}
                >
                  <option value="">Default branch</option>
                  {#each branches as b (b)}<option value={b}>{b}</option>{/each}
                  {#if rule.baseBranch && !branches.includes(rule.baseBranch)}<option value={rule.baseBranch}>{rule.baseBranch}</option>{/if}
                </Select>
                <Input
                  size="sm"
                  class="w-full font-mono lg:w-36"
                  placeholder="branch prefix"
                  aria-label={`Branch prefix for ${g.repo}`}
                  value={rule.branchPrefix ?? ''}
                  disabled={saving}
                  oninput={(e) => (edits[key(g)] = { ...rule, branchPrefix: e.currentTarget.value || null })}
                />
                <Select
                  size="sm"
                  class="w-full lg:w-44"
                  aria-label={`Push rule for ${g.repo}`}
                  value={rule.pushMode}
                  disabled={saving}
                  onchange={(e) => (edits[key(g)] = { ...rule, pushMode: (e.currentTarget as HTMLSelectElement).value as RepoRule['pushMode'] })}
                >
                  <option value="branches_only">Branches only</option>
                  <option value="free">May push the base</option>
                </Select>
                <div class="flex justify-end lg:contents">
                  <SettingsRowMenu label={`Actions for ${g.repo}`} items={menu(d, g)} />
                </div>
              </div>
            {:else}
              <span class="font-sans text-[13px] leading-5 text-muted">
                {rule.baseBranch ?? 'Default branch'} · {rule.branchPrefix ? `prefix ${rule.branchPrefix}` : 'no prefix'} · {PUSH_WORD[rule.pushMode]}
              </span>
            {/if}
          </SettingsRow>
        {/each}
        {#if d.granted.length === 0}
          <p transition:slide={{ duration: 150 }} class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">Nothing granted yet, so it cannot touch any repository.</p>
        {/if}
        {#if canManage}
          {#each d.hosts.filter((h) => h.error) as h (h.id)}
            <p class="px-5 py-3 font-sans text-[13px] leading-5 text-warning">{h.name} could not list its repositories: {h.error}</p>
          {/each}
          {#if pool.length > 0}
            <SettingsRow label="Add repository" hint="Its token reaches it on the next push." apart>
              <Combobox
                size="sm"
                class="w-full sm:w-72"
                placeholder="Add repository"
                triggerLabel="Add repository"
                disabled={saving}
                options={pool.map((r) => ({ value: key(r), label: r.repo, sub: hostName(d, r.host) }))}
                selected={[]}
                onChange={(next) => next[0] && add(d, next[0])}
              />
            </SettingsRow>
          {/if}
          <SettingsSaveBar
            label="Save branch rules"
            reason="Branch rules apply on the agent's next push."
            {dirty}
            {saving}
            {saved}
            onSave={() => void write(d.granted)}
            onDiscard={() => (edits = {})}
          />
        {/if}
      {/if}
    {/if}
  </SettingsGroup>
</SettingsSection>

{#if variablesFor}
  <ProjectVariablesModal open={variablesFor !== null} onClose={() => (variablesFor = null)} repo={variablesFor} />
{/if}
