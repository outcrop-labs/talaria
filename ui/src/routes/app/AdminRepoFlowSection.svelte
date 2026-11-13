<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { useSavedCue } from '@/components/admin/saved-cue'
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
  const autosave = useSavedCue()

  const save = async (r: HostRepo, baseBranch: string | null) => {
    try {
      await putJson<{ ok: true }>('/api/workbench/flow', { host: r.host, repo: r.repo, baseBranch })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    autosave.flash()
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
<SettingsSection
  title="Pull request branches"
  description="The branch each repository's jobs start from and its pull requests target."
  info="Leave a field blank to use the repository's default branch. Talaria's job ends at the pull request. How it is reviewed and merged belongs to the repository's own branch protection and CI."
>
  <SettingsGroup>
    {#if !data}
      {#if query.isError}
        <QueryError variant="compact" error={query.error} title="Could not load repository flow" onRetry={() => void query.refetch()} />
      {:else}
        <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
      {/if}
    {:else}
      {#each data.hosts.filter((h) => h.error) as h (h.id)}
        <NoticeRow status="danger">{h.name} could not list its repositories: {h.error}</NoticeRow>
      {/each}
      {#each rows as r (repoKey(r))}
        {@const f = data.flows.find((x) => x.host === r.host && x.repo === r.repo)}
        {@const unreachable = !data.repos.some((x) => x.host === r.host && x.repo === r.repo)}
        <SettingsRow hint={manyHosts ? hostName(r.host) : undefined} as="label">
          {#snippet label()}<span class="font-mono text-[13px] font-normal">{r.repo}</span>{/snippet}
          {#snippet status()}{#if unreachable}<RowStatus status="danger">Unreachable</RowStatus>{/if}{/snippet}
          <Input
            size="sm"
            class="w-[280px] font-mono"
            aria-label="PRs land on for {r.repo}"
            value={f?.baseBranch ?? ''}
            placeholder="default branch"
            onblur={(e) => e.currentTarget.value.trim() !== (f?.baseBranch ?? '') && void save(r, e.currentTarget.value.trim() || null)}
          />
        </SettingsRow>
      {/each}
      {#if rows.length === 0}
        <SettingsRow label="No repositories yet" hint="Connect a git host above and its repositories show here." />
      {/if}
    {/if}
  </SettingsGroup>
</SettingsSection>
