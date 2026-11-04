<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
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
  {#snippet skeleton()}<div class="mt-2"><SkeletonRows rows={1} /></div>{/snippet}
  {#snippet empty()}
    <p class="mt-1.5 text-xs text-muted">
      No repositories reachable. Connect a git host under <a href={p('/admin/:tab', { params: { tab: 'git' } })} class="text-accent hover:underline">Admin → Git hosts</a> to grant repos.
    </p>
  {/snippet}
  {#snippet children(d)}
    <div class="mt-2 space-y-1">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repos this agent may work</span>
      {#each d.hosts.filter((h) => h.error) as h (h.id)}
        <p class="text-xs text-warning">{h.name} could not list its repositories: {h.error}</p>
      {/each}
      <div class="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
        <!-- Repo grants — the one filter-pill primitive (Chip). -->
        {#each d.available as r (key(r))}
          {@const on = d.granted.some((g) => same(g, r))}
          <Chip onSelect={() => void toggle(r, !on)} selected={on} class="px-2.5 py-0.5 normal-case" title={hostName(d, r.host)}>
            {#if manyHosts(d)}<span class="text-muted">{r.host}:</span>{/if}{r.repo}
          </Chip>
        {/each}
      </div>
      {#if d.granted.length === 0}<p transition:slide={{ duration: 150 }} class="text-xs text-muted">Nothing granted yet, so the workbench can't touch any repo.</p>{/if}
      {#if d.granted.length > 0}
        <div class="mt-3 space-y-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Branch rules — enforced by the pre-push gate in the agent's container</span>
          {#each d.granted as g (key(g))}
            {@const rule = ruleFor(d, g)}
            {@const branches = d.branches?.[key(g)] ?? []}
            <div class="flex flex-wrap items-center gap-1.5 rounded-md border border-line-subtle bg-raised/40 px-2 py-1.5">
              <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">
                {#if manyHosts(d)}<span class="text-muted">{`${hostName(d, g.host)} · `}</span>{/if}{g.repo}
              </span>
              <Select size="sm" class="w-32" value={rule.baseBranch ?? ''} onchange={(e) => (edits[key(g)] = { ...rule, baseBranch: (e.currentTarget as HTMLSelectElement).value || null })}>
                <option value="">default branch</option>
                {#each branches as b (b)}<option value={b}>{b}</option>{/each}
              </Select>
              <Input size="sm" class="w-28 font-mono text-xs" placeholder="branch prefix" value={rule.branchPrefix ?? ''} oninput={(e) => (edits[key(g)] = { ...rule, branchPrefix: e.currentTarget.value || null })} />
              <Select size="sm" class="w-32" value={rule.pushMode} onchange={(e) => (edits[key(g)] = { ...rule, pushMode: (e.currentTarget as HTMLSelectElement).value as RepoRule['pushMode'] })}>
                <option value="branches_only">branches only</option>
                <option value="free">may push base</option>
              </Select>
            </div>
            {@const envKeys = d.env?.[g.repo] ?? []}
            {@const pendingEnv = (envDeletes[g.repo]?.length ?? 0) > 0 || (newEnvRow[g.repo]?.key && newEnvRow[g.repo]?.value)}
            <div class="flex flex-wrap items-center gap-1.5 px-2 py-1">
              <span class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim" title="Values are sealed at rest, materialized into the agent's container, and never displayed again — project keys only, never org credentials">project env</span>
              {#each envKeys as k (k)}
                {#if !(envDeletes[g.repo] ?? []).includes(k)}
                  <button type="button" class="rounded border border-line bg-raised/60 px-1.5 py-0.5 font-mono text-[11px] text-fg hover:border-danger hover:text-danger" title="Remove this variable" onclick={() => removeEnvKey(g.repo, k)}>{k} ×</button>
                {/if}
              {/each}
              <Input size="sm" class="w-24 font-mono text-xs" placeholder="KEY" value={newEnvRow[g.repo]?.key ?? ''} oninput={(e) => (newEnvRow[g.repo] = { key: e.currentTarget.value.toUpperCase(), value: newEnvRow[g.repo]?.value ?? '' })} />
              <Input size="sm" class="w-40 font-mono text-xs" type="password" placeholder="value" value={newEnvRow[g.repo]?.value ?? ''} oninput={(e) => (newEnvRow[g.repo] = { key: newEnvRow[g.repo]?.key ?? '', value: e.currentTarget.value })} />
              {#if pendingEnv}
                <button type="button" class="text-xs text-accent hover:underline" onclick={() => void saveEnv(g.repo)}>save env</button>
              {/if}
            </div>
          {/each}
          {#if Object.keys(edits).length > 0}
            <button type="button" class="mt-1 text-xs text-accent hover:underline" onclick={() => void save(d.granted)}>Save branch rules</button>
          {/if}
        </div>
      {/if}
    </div>
  {/snippet}
</QueryState>
