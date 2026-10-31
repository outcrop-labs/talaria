<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { repoKey, type HostRepo } from './git-hosts'

  /** Per-repo git flow — every repo any git host reaches is a row; a blank
   *  field means the default, so there is no separate "add" ceremony to
   *  learn. The same owner/name on two hosts is two rows. */
  interface FlowData {
    flows: Array<HostRepo & { baseBranch: string | null }>
    repos: HostRepo[]
    hosts: Array<{ id: string; name: string; kind: string; error: string | null }>
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['workbench-flow'],
    queryFn: (): Promise<FlowData> => getJson<FlowData>('/api/workbench/flow'),
  }))
  const data = $derived(query.data)
  const savedFlash = useSavedFlash()

  const save = async (r: HostRepo, baseBranch: string | null) => {
    try {
      await putJson<{ ok: true }>('/api/workbench/flow', { host: r.host, repo: r.repo, baseBranch })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    savedFlash.flash()
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
  // Every reachable repo is a row; configs for repos a host no longer
  // reaches still show (flagged) so they can be understood and cleared.
  const rows = $derived.by((): HostRepo[] => {
    if (!data) return []
    const seen = new Map<string, HostRepo>()
    for (const r of [...data.repos, ...data.flows.map((f) => ({ host: f.host, repo: f.repo }))]) seen.set(repoKey(r), r)
    return [...seen.values()].sort((a, b) => repoKey(a).localeCompare(repoKey(b)))
  })
  const manyHosts = $derived((data?.hosts.length ?? 0) > 1)
  const hostName = (id: string) => data?.hosts.find((h) => h.id === id)?.name ?? id
</script>

<!-- A failed read used to REMOVE the whole section, so per-repo base branches
     looked unconfigured — and re-entering them would have overwritten the real
     flow. -->
{#if !data}
  {#if query.isError}
    <QueryError variant="inline" error={query.error} title="Could not load repository flow" onRetry={() => void query.refetch()} />
  {/if}
{:else}
  <div class="space-y-2">
    <div class="flex flex-wrap items-baseline gap-x-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repository flow</span>
      <span class="text-xs text-muted">which branch workbench PRs target, per repo</span>
      {#if savedFlash.saved}<span class="ml-auto text-xs text-success">Saved</span>{/if}
    </div>
    {#each data.hosts.filter((h) => h.error) as h (h.id)}
      <p class="text-xs text-warning">{h.name} could not list its repositories: {h.error}</p>
    {/each}
    <div class="space-y-1.5">
      {#each rows as r (repoKey(r))}
        {@const f = data.flows.find((x) => x.host === r.host && x.repo === r.repo)}
        {@const unreachable = !data.repos.some((x) => x.host === r.host && x.repo === r.repo)}
        <div class="grid grid-cols-1 items-center gap-1 sm:grid-cols-[minmax(0,1fr)_11rem] sm:gap-2">
          <span class="min-w-0 truncate font-mono text-xs text-fg">
            {#if manyHosts}<span class="text-muted">{`${hostName(r.host)} · `}</span>{/if}{r.repo}
            {#if unreachable}<span class="ml-2 font-sans text-xs text-warning">no longer reachable</span>{/if}
          </span>
          <Input
            size="sm"
            aria-label="PRs land on for {r.repo}"
            value={f?.baseBranch ?? ''}
            placeholder="default branch"
            onblur={(e) => e.currentTarget.value.trim() !== (f?.baseBranch ?? '') && void save(r, e.currentTarget.value.trim() || null)}
          />
        </div>
      {/each}
      {#if rows.length === 0}<p class="text-xs text-muted">No repositories reachable yet. Connect a git host above.</p>{/if}
    </div>
    <p class="text-[11px] text-muted">
      <span class="text-fg">PRs land on</span>: the branch workbench jobs cut from and pull requests target; blank uses the repo's default branch. Talaria's job ends at the pull request — how it is then reviewed and merged belongs to the repository's own branch protection and CI.
    </p>
  </div>
{/if}
