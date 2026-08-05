<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { p } from '@/router'

  // Explicit per-agent repo grants — the workbench touches ONLY these.
  let { agentId }: { agentId: string } = $props()

  const qc = useQueryClient()
  // The route's only 404 is "unknown agent" — and this panel renders INSIDE the
  // manage modal for that very agent, so a 404 is an anomaly, not the legitimate
  // "no such thing" that `getJsonOr404` exists for. Everything throws.
  const query = createQuery(() => ({
    queryKey: ['workbench-repos', agentId],
    queryFn: (): Promise<{ available: string[]; granted: string[] }> =>
      getJson<{ available: string[]; granted: string[] }>(`/api/workbench/repos/${agentId}`),
  }))
  const toggle = async (repo: string, on: boolean) => {
    const granted = query.data?.granted ?? []
    const next = on ? [...granted, repo] : granted.filter((r) => r !== repo)
    await fetch(`/api/workbench/repos/${agentId}`, {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ repos: next }),
    })
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
      No repositories reachable — connect GitHub under <a href={p('/admin')} class="text-accent hover:underline">Admin → Org</a> to grant repos.
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
      {#if d.granted.length === 0}<p class="text-xs text-muted">Nothing granted yet — the workbench can't touch any repo.</p>{/if}
    </div>
  {/snippet}
</QueryState>
