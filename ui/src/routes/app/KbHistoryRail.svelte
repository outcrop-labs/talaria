<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  interface Rev {
    id: string
    createdBy: string | null
    createdAt: string
    size: number
  }

  let {
    kind = 'kb-doc',
    id,
    onRestore,
  }: { kind?: 'kb-doc' | 'kb-space'; id: string; onRestore: (content: string) => Promise<void> } = $props()

  let revs = $state<Rev[]>([])
  let loading = $state(true)
  // `r.ok ? r.json() : { revisions: [] }` told the owner of a document that it
  // has NO VERSION HISTORY — on the one rail whose entire purpose is restoring
  // a previous version. If the read fails the honest answer is "we could not
  // read it", not "there is nothing to go back to".
  let error = $state<unknown>(null)
  let reload = $state(0)
  let preview = $state<{ rev: Rev; content: string } | null>(null)
  let restoring = $state(false)
  $effect(() => {
    // Both kb-doc and kb-space history key on the item id (like memory).
    void reload
    let live = true
    loading = true
    error = null
    getJson<{ revisions?: Rev[] }>(`/api/history?kind=${kind}&id=${id}`)
      .then((d) => {
        if (!live) return
        revs = d.revisions ?? []
      })
      .catch((e: unknown) => {
        if (!live) return
        revs = []
        error = e
      })
      .finally(() => {
        if (live) loading = false
      })
    return () => {
      live = false
    }
  })

  const open = async (rev: Rev) => {
    const r = await fetch(`/api/history?kind=${kind}&id=${id}&rev=${rev.id}`)
    if (!r.ok) return
    const { content } = (await r.json()) as { content: string }
    preview = { rev, content }
  }
</script>

<div>
  <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</div>
  {#if loading}
    <SkeletonRows rows={4} class="px-2 py-1" />
  {:else if error}
    <QueryError
      variant="inline"
      title="Could not load this document’s history"
      {error}
      onRetry={() => (reload += 1)}
    />
  {:else if revs.length === 0}
    <EmptyState variant="inline" title="No saved revisions yet." />
  {:else}
    {#each revs as r, i (r.id)}
      <button data-dither-fill
        type="button"
        onclick={() => void open(r)}
        class="block w-full rounded-md px-2 py-1.5 text-left text-xs transition-colors"
      >
        <div class="text-fg">{i === 0 ? 'Latest' : relativeTime(r.createdAt)}</div>
        <div class="font-mono text-[10px] tracking-[0.05em] text-muted">{r.createdBy ?? 'unknown'} · {r.size} chars</div>
      </button>
    {/each}
  {/if}

  <Modal
    open={!!preview}
    onClose={() => (preview = null)}
    title={preview ? `Revision · ${relativeTime(preview.rev.createdAt)}` : 'Revision'}
    width="max-w-3xl"
  >
    <div class="max-h-[60vh] overflow-y-auto">
      {#if preview}<Markdown children={preview.content} />{/if}
    </div>
    {#snippet footer()}
      <div class="flex justify-end gap-2">
        <Button variant="outline" size="sm" onclick={() => (preview = null)}>
          Close
        </Button>
        <Button
          size="sm"
          disabled={restoring}
          onclick={async () => {
            if (!preview) return
            restoring = true
            try {
              await onRestore(preview.content)
              preview = null
            } finally {
              restoring = false
            }
          }}
        >
          {restoring ? 'Restoring' : 'Restore this version'}
        </Button>
      </div>
    {/snippet}
  </Modal>
</div>
