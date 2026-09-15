<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { slide } from '@/lib/motion'
  import { p } from '@/router'

  // Explicit per-agent repo grants — the workbench touches ONLY these — and
  // each grant's BRANCH LAW: the base a human merges, whether the agent may
  // push it at all, the prefix its own branches must carry. The rules ride
  // the same write as the chips, and the pre-push hook enforces every line
  // in the container before bytes reach the remote.
  let { agentId }: { agentId: string } = $props()

  interface RepoRule {
    repo: string
    baseBranch: string | null
    pushMode: 'branches_only' | 'free'
    branchPrefix: string | null
  }
  interface ReposBody {
    available: string[]
    granted: string[]
    rules?: RepoRule[]
    branches?: Record<string, string[]>
  }

  const qc = useQueryClient()
  // The route's only 404 is "unknown agent" — and this panel renders INSIDE the
  // manage modal for that very agent, so a 404 is an anomaly, not the legitimate
  // "no such thing" that `getJsonOr404` exists for. Everything throws.
  const query = createQuery(() => ({
    queryKey: ['workbench-repos', agentId],
    queryFn: (): Promise<ReposBody> => getJson<ReposBody>(`/api/workbench/repos/${agentId}`),
  }))
  // Local rule edits, keyed by repo — committed with the chips on the same
  // PUT so a grant toggle and its rules can never half-land.
  let edits = $state<Record<string, RepoRule>>({})
  const ruleFor = (d: ReposBody, repo: string): RepoRule =>
    edits[repo] ??
    d.rules?.find((r) => r.repo === repo) ?? {
      repo,
      baseBranch: null,
      pushMode: 'branches_only',
      branchPrefix: null,
    }
  const toggle = async (repo: string, on: boolean) => {
    const granted = query.data?.granted ?? []
    const next = on ? [...granted, repo] : granted.filter((r) => r !== repo)
    await save(next)
  }
  const save = async (granted: string[]) => {
    // Rules for every granted repo ride along — edited ones and untouched
    // ones alike, so the server's row always matches what this panel shows.
    const rules = granted.map((repo) => ruleFor(query.data!, repo))
    try {
      await putJson(`/api/workbench/repos/${agentId}`, { repos: granted, rules })
      edits = {}
    } catch (e) {
      // Fire-and-forget from this panel: the toast is the only place a failed
      // write can be said, and the refresh below restores the truth.
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
    }
    await qc.invalidateQueries({ queryKey: ['workbench-repos', agentId] })
  }
</script>

<QueryState
  {query}
  errorTitle="Could not load repository grants"
  errorVariant="inline"
  isEmpty={(d) => d.available.length === 0}
>
  {#snippet skeleton()}<div class="mt-2"><SkeletonRows rows={1} /></div>{/snippet}
  {#snippet empty()}
    <p class="mt-1.5 text-xs text-muted">
      No repositories reachable. Connect GitHub under <a href={p('/admin')} class="text-accent hover:underline">Admin → Org</a> to grant repos.
    </p>
  {/snippet}
  {#snippet children(d)}
    <div class="mt-2 space-y-1">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repos this agent may work</span>
      <div class="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
        <!-- Repo grants — the one filter-pill primitive (Chip). -->
        {#each d.available as repo (repo)}
          {@const on = d.granted.includes(repo)}
          <Chip onSelect={() => void toggle(repo, !on)} selected={on} class="px-2.5 py-0.5 normal-case">
            {repo}
          </Chip>
        {/each}
      </div>
      {#if d.granted.length === 0}<p transition:slide={{ duration: 150 }} class="text-xs text-muted">Nothing granted yet, so the workbench can't touch any repo.</p>{/if}
      {#if d.granted.length > 0}
        <div class="mt-3 space-y-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Branch rules — enforced by the pre-push gate in the agent's container</span>
          {#each d.granted as repo (repo)}
            {@const rule = ruleFor(d, repo)}
            {@const branches = d.branches?.[repo] ?? []}
            <div class="flex flex-wrap items-center gap-1.5 rounded-md border border-line-subtle bg-raised/40 px-2 py-1.5">
              <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{repo}</span>
              <Select size="sm" class="w-32" value={rule.baseBranch ?? ''} onchange={(e) => (edits[repo] = { ...rule, baseBranch: (e.currentTarget as HTMLSelectElement).value || null })}>
                <option value="">default branch</option>
                {#each branches as b (b)}<option value={b}>{b}</option>{/each}
              </Select>
              <Input size="sm" class="w-28 font-mono text-xs" placeholder="branch prefix" value={rule.branchPrefix ?? ''} oninput={(e) => (edits[repo] = { ...rule, branchPrefix: e.currentTarget.value || null })} />
              <Select size="sm" class="w-32" value={rule.pushMode} onchange={(e) => (edits[repo] = { ...rule, pushMode: (e.currentTarget as HTMLSelectElement).value as RepoRule['pushMode'] })}>
                <option value="branches_only">branches only</option>
                <option value="free">may push base</option>
              </Select>
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
