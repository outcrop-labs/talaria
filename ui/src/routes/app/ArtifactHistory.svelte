<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson, getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'

  interface Rev {
    id: string
    createdBy: string | null
    createdAt: string
    size: number
  }

  let { id, onRestore }: { id: string; onRestore: (content: string) => Promise<void> } = $props()

  // "No saved revisions yet." is a claim about this artifact's PAST — the one
  // sentence that tells an owner their earlier drafts are gone. A 500 used to
  // render that exact sentence, indistinguishable from a genuinely fresh doc.
  const history = createQuery(() => ({
    queryKey: ['artifact-history', id],
    queryFn: () => getList<Rev>(`/api/history?kind=artifact&id=${id}`, 'revisions'),
  }))
  // Restore used to `return` on a non-2xx: a button that does nothing, forever,
  // with no way to tell a broken restore from a slow one.
  let restoreError = $state<unknown>(null)
  let restoring = $state<string | null>(null)
  const restore = async (rev: Rev) => {
    restoreError = null
    restoring = rev.id
    try {
      const { content } = await getJson<{ content: string }>(`/api/history?kind=artifact&id=${id}&rev=${rev.id}`)
      await onRestore(content)
    } catch (e) {
      restoreError = e
    } finally {
      restoring = null
    }
  }
</script>

<div>
  <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</div>
  {#if restoreError !== null}
    <div transition:slide={{ duration: 150 }}>
      <QueryError variant="inline" class="mb-2 px-2" title="Could not restore that version" error={restoreError} />
    </div>
  {/if}
  <!-- The loading/empty/failed fork lives in QueryState so the "No saved
       revisions yet." sentence can only ever come from a real 200-with-[] —
       a failed read gets its own error row instead. -->
  <QueryState
    query={history}
    errorTitle="Could not load this artifact’s history"
    errorVariant="inline"
  >
    {#snippet skeleton()}<SkeletonRows rows={4} class="px-2 py-1" />{/snippet}
    {#snippet empty()}<EmptyState variant="inline" title="No saved revisions yet." />{/snippet}
    {#snippet children(revs)}
      {#each revs as r, i (r.id)}
        <button data-dither-fill
          type="button"
          disabled={restoring !== null}
          onclick={() => void restore(r)}
          class="block w-full rounded-md px-2 py-1.5 text-left text-xs transition-colors disabled:opacity-60"
          title="Restore this version"
        >
          <div class="text-fg">{i === 0 ? 'Latest' : relativeTime(r.createdAt)}</div>
          <div class="font-mono text-[10px] tracking-[0.05em] text-muted">{r.createdBy ?? 'unknown'} · {r.size} chars</div>
        </button>
      {/each}
    {/snippet}
  </QueryState>
</div>
