<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson, patchJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { slide } from '@/lib/motion'
  import { p } from '@/router'

  // Explicit per-agent repo grants — the workbench touches ONLY these — and
  // each grant's BRANCH LAW: the base a human merges, whether the agent may
  // push it at all, the prefix its own branches must carry. A grant names
  // the git host too: the same owner/name on two hosts is two repositories.
  // The rules ride the same write as the chips, and the pre-push hook
  // enforces every line in the container before bytes reach the remote.
  let { agentId }: { agentId: string } = $props()

  interface HostRepo {
    host: string
    repo: string
  }
  interface RepoRule extends HostRepo {
    baseBranch: string | null
    pushMode: 'branches_only' | 'free'
    branchPrefix: string | null
  }
  interface ReposBody {
    hosts: Array<{ id: string; name: string; kind: string; error: string | null }>
    available: HostRepo[]
    granted: HostRepo[]
    rules?: RepoRule[]
    /** Keyed `host:owner/name`. */
    branches?: Record<string, string[]>
    env?: Record<string, string[]>
  }

  const key = (r: HostRepo) => `${r.host}:${r.repo}`
  const same = (a: HostRepo, b: HostRepo) => a.host === b.host && a.repo === b.repo

  const qc = useQueryClient()
  // The route's only 404 is "unknown agent" — and this panel renders INSIDE the
  // manage modal for that very agent, so a 404 is an anomaly, not the legitimate
  // "no such thing" that `getJsonOr404` exists for. Everything throws.
  const query = createQuery(() => ({
    queryKey: ['workbench-repos', agentId],
    queryFn: (): Promise<ReposBody> => getJson<ReposBody>(`/api/workbench/repos/${agentId}`),
  }))
  // Local rule edits, keyed by host:repo — committed with the chips on the
  // same PUT so a grant toggle and its rules can never half-land.
  let edits = $state<Record<string, RepoRule>>({})
  const ruleFor = (d: ReposBody, r: HostRepo): RepoRule =>
    edits[key(r)] ??
    d.rules?.find((x) => same(x, r)) ?? {
      ...r,
      baseBranch: null,
      pushMode: 'branches_only',
      branchPrefix: null,
    }
  // ── Project env stores ─────────────────────────────────────────────────
  // Keys arrive from the server; values NEVER do (write-only, sealed at
  // rest). An edit row holds a new key/value pair; saving sends {set:[...]}
  // for rows with a value, {delete:[...]} for removed keys.
  let envDeletes = $state<Record<string, string[]>>({})
  let newEnvRow = $state<Record<string, { key: string; value: string }>>({})
  const removeEnvKey = (repo: string, k: string) => {
    envDeletes[repo] = [...(envDeletes[repo] ?? []), k]
  }
  const saveEnv = async (repo: string) => {
    const set = []
    if (newEnvRow[repo]?.key && newEnvRow[repo]?.value) set.push(newEnvRow[repo])
    const del = envDeletes[repo] ?? []
    if (set.length === 0 && del.length === 0) return
    try {
      await patchJson(`/api/workbench/env/${repo}`, { set, delete: del })
      envDeletes[repo] = []
      newEnvRow[repo] = { key: '', value: '' }
    } catch (e) {
      toastError('Env save failed', e)
    }
    await qc.invalidateQueries({ queryKey: ['workbench-repos', agentId] })
  }

  const toggle = async (r: HostRepo, on: boolean) => {
    const granted = query.data?.granted ?? []
    const next = on ? [...granted, r] : granted.filter((g) => !same(g, r))
    await save(next)
  }
  const save = async (granted: HostRepo[]) => {
    // Rules for every granted repo ride along — edited ones and untouched
    // ones alike, so the server's row always matches what this panel shows.
    const rules = granted.map((r) => ruleFor(query.data!, r))
    try {
      await putJson(`/api/workbench/repos/${agentId}`, { repos: granted.map((r) => ({ host: r.host, repo: r.repo })), rules })
      edits = {}
    } catch (e) {
      // Fire-and-forget from this panel: the toast is the only place a failed
      // write can be said, and the refresh below restores the truth.
      toastError('Save failed', e)
    }
    await qc.invalidateQueries({ queryKey: ['workbench-repos', agentId] })
  }
  const manyHosts = (d: ReposBody) => d.hosts.length > 1
  const hostName = (d: ReposBody, id: string) => d.hosts.find((h) => h.id === id)?.name ?? id
</script>

<QueryState
  {query}
  errorTitle="Could not load repository grants"
  errorVariant="inline"
  isEmpty={(d) => d.available.length === 0 && d.granted.length === 0}
>
  {#snippet skeleton()}<SkeletonRows rows={1} />{/snippet}
  {#snippet empty()}
    <p class="font-sans text-[13px] leading-5 text-muted">
      No repositories reachable. Connect a git host under <a href={p('/admin/:tab', { params: { tab: 'git' } })} class="text-accent hover:underline">Admin, Git hosts</a> to grant some.
    </p>
  {/snippet}
  {#snippet children(d)}
    <div class="space-y-5">
      <div class="space-y-2">
        <div class="font-sans text-[13px] font-medium leading-5 text-fg">Repositories it may work in</div>
        {#each d.hosts.filter((h) => h.error) as h (h.id)}
          <p class="font-sans text-[13px] leading-5 text-warning">{h.name} could not list its repositories: {h.error}</p>
        {/each}
        <div class="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
          <!-- Repo grants — the one filter-pill primitive (Chip). -->
          {#each d.available as r (key(r))}
            {@const on = d.granted.some((g) => same(g, r))}
            <Chip onSelect={() => void toggle(r, !on)} selected={on} class="font-mono" title={hostName(d, r.host)}>
              {#if manyHosts(d)}<span class="text-muted">{r.host}:</span>{/if}{r.repo}
            </Chip>
          {/each}
        </div>
        {#if d.granted.length === 0}
          <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted">Nothing granted yet, so it cannot touch any repository.</p>
        {/if}
      </div>
      {#if d.granted.length > 0}
        <div class="space-y-3">
          <div>
            <div class="font-sans text-[13px] font-medium leading-5 text-fg">Branch rules</div>
            <p class="font-sans text-[13px] leading-5 text-muted">Checked in the agent's container every time it pushes.</p>
          </div>
          {#each d.granted as g (key(g))}
            {@const rule = ruleFor(d, g)}
            {@const branches = d.branches?.[key(g)] ?? []}
            {@const envKeys = d.env?.[g.repo] ?? []}
            {@const pendingEnv = (envDeletes[g.repo]?.length ?? 0) > 0 || (newEnvRow[g.repo]?.key && newEnvRow[g.repo]?.value)}
            <div class="space-y-2 border-t border-line-subtle pt-3">
              <div class="flex flex-wrap items-center gap-2">
                <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">
                  {#if manyHosts(d)}<span class="text-muted">{`${hostName(d, g.host)} · `}</span>{/if}{g.repo}
                </span>
                <Select size="sm" class="w-40" aria-label="Base branch" value={rule.baseBranch ?? ''} onchange={(e) => (edits[key(g)] = { ...rule, baseBranch: (e.currentTarget as HTMLSelectElement).value || null })}>
                  <option value="">Default branch</option>
                  {#each branches as b (b)}<option value={b}>{b}</option>{/each}
                </Select>
                <Input size="sm" class="w-36 font-mono" placeholder="branch prefix" aria-label="Branch prefix" value={rule.branchPrefix ?? ''} oninput={(e) => (edits[key(g)] = { ...rule, branchPrefix: e.currentTarget.value || null })} />
                <Select size="sm" class="w-44" aria-label="Push rule" value={rule.pushMode} onchange={(e) => (edits[key(g)] = { ...rule, pushMode: (e.currentTarget as HTMLSelectElement).value as RepoRule['pushMode'] })}>
                  <option value="branches_only">Branches only</option>
                  <option value="free">May push the base</option>
                </Select>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <span
                  class="w-28 shrink-0 font-sans text-[13px] leading-5 text-muted"
                  title="Values are sealed at rest, placed in the agent's container, and never shown again. Project keys only, never org credentials."
                >
                  Project variables
                </span>
                {#each envKeys as k (k)}
                  {#if !(envDeletes[g.repo] ?? []).includes(k)}
                    <Chip class="font-mono" title="Remove this variable" onRemove={() => removeEnvKey(g.repo, k)}>{k}</Chip>
                  {/if}
                {/each}
                <Input size="sm" class="w-32 font-mono" placeholder="KEY" aria-label="Variable name" value={newEnvRow[g.repo]?.key ?? ''} oninput={(e) => (newEnvRow[g.repo] = { key: e.currentTarget.value.toUpperCase(), value: newEnvRow[g.repo]?.value ?? '' })} />
                <Input size="sm" class="w-44 font-mono" type="password" placeholder="value" aria-label="Variable value" value={newEnvRow[g.repo]?.value ?? ''} oninput={(e) => (newEnvRow[g.repo] = { key: newEnvRow[g.repo]?.key ?? '', value: e.currentTarget.value })} />
                {#if pendingEnv}
                  <Button size="sm" variant="outline" onclick={() => void saveEnv(g.repo)}>Save variables</Button>
                {/if}
              </div>
            </div>
          {/each}
          {#if Object.keys(edits).length > 0}
            <Button size="sm" variant="outline" onclick={() => void save(d.granted)}>Save branch rules</Button>
          {/if}
        </div>
      {/if}
    </div>
  {/snippet}
</QueryState>
