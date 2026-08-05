<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { CheckCircle2, CornerDownRight, MessageSquareText } from '@lucide/svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { cn } from '@/lib/cn'
  import { fade, slide, GROW_X, QUICK } from '@/lib/motion'
  import KbCommentBody from './KbCommentBody.svelte'
  import type { KbComment } from './knowledge.svelte'

  /** Notion-shaped comment threads: roots (optionally anchored to a quote),
   *  replies, resolve. Lives as a side panel beside the doc body. */
  let {
    docId,
    comments,
    loadFailed,
    loadError,
    onRetryLoad,
    meId,
    docOwnerId,
    pendingQuote,
    onQuoteConsumed,
    focusId,
    onFocusConsumed,
    onClose,
  }: {
    docId: string
    comments: KbComment[]
    /** The thread read FAILED — the empty list below is ignorance, not silence. */
    loadFailed: boolean
    loadError: unknown
    onRetryLoad: () => void
    meId: string | null
    docOwnerId: string | null
    pendingQuote: string | null
    onQuoteConsumed: () => void
    focusId?: string | null
    onFocusConsumed?: () => void
    onClose: () => void
  } = $props()

  const qc = useQueryClient()
  let draft = $state('')
  let replyTo = $state<string | null>(null)
  let replyDraft = $state('')
  let showResolved = $state(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['kb-comments', docId] })

  const post = async (content: string, parentId: string | null, quote: string | null) => {
    if (!content.trim()) return
    await fetch(`/api/kb/docs/${docId}/comments`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: content.trim(), parentId, quote }),
    })
    await refresh()
  }
  const setResolved = async (id: string, resolved: boolean) => {
    await fetch(`/api/kb/comments/${id}`, {
      method: 'PATCH',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ resolved }),
    })
    await refresh()
  }
  const remove = async (id: string) => {
    await fetch(`/api/kb/comments/${id}`, { method: 'DELETE', credentials: 'same-origin' })
    await refresh()
  }

  // A mark click lands here: scroll its thread into view and flash it.
  $effect(() => {
    if (!focusId) return
    const el = document.querySelector(`[data-kb-thread="${focusId}"]`)
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      el.classList.add('kb-comment-flash')
      setTimeout(() => el.classList.remove('kb-comment-flash'), 1300)
    }
    onFocusConsumed?.()
  })

  const roots = $derived(comments.filter((c) => !c.parentId))
  const open = $derived(roots.filter((c) => !c.resolved))
  const resolved = $derived(roots.filter((c) => c.resolved))
  const repliesOf = (id: string) => comments.filter((c) => c.parentId === id)
</script>

{#snippet thread(root: KbComment)}
  <div
    in:fade={{ duration: 150 }}
    out:fade={QUICK}
    data-kb-thread={root.id}
    class={cn('space-y-2 rounded-lg border border-line-subtle p-2.5', root.resolved && 'opacity-60')}
  >
    <!-- Quoted passage — same warning-gold tint as the in-doc comment mark. -->
    {#if root.quote}
      <div class="border-l-2 border-warning/50 pl-2 font-sans text-[11px] italic text-muted line-clamp-2">“{root.quote}”</div>
    {/if}
    <KbCommentBody c={root} {meId} onDelete={() => void remove(root.id)} />
    {#each repliesOf(root.id) as r (r.id)}
      <div class="flex gap-1.5 pl-3">
        <CornerDownRight size={11} class="mt-1 shrink-0 text-muted/60" />
        <div class="min-w-0 flex-1">
          <KbCommentBody c={r} {meId} onDelete={() => void remove(r.id)} />
        </div>
      </div>
    {/each}
    <div class="flex items-center gap-2 pl-3">
      {#if replyTo === root.id}
        <Textarea
          autofocus
          autoGrow
          rows={1}
          bind:value={replyDraft}
          onkeydown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void post(replyDraft, root.id, null).then(() => {
                replyDraft = ''
                replyTo = null
              })
            } else if (e.key === 'Escape') {
              replyTo = null
            }
          }}
          placeholder="Reply"
          class="min-h-0 flex-1 text-xs"
        />
      {:else}
        <button type="button" onclick={() => (replyTo = root.id)} class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg">
          Reply
        </button>
      {/if}
      {#if root.authorUserId === meId || docOwnerId === meId}
        <button
          type="button"
          onclick={() => void setResolved(root.id, !root.resolved)}
          class="ml-auto flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-success"
          title={root.resolved ? 'Reopen this thread' : 'Resolve this thread'}
        >
          <CheckCircle2 size={12} /> {root.resolved ? 'Reopen' : 'Resolve'}
        </button>
      {/if}
    </div>
  </div>
{/snippet}

<!-- IN-FLOW panel: slide={GROW_X} on both legs so the editor glides as the
     rail grows/shrinks instead of snapping (ANIMATIONS.md). |global:
     KbDocEditor mounts this whole component per toggle, so local legs on the
     component root never play (the |global rule). Inner wrapper pinned to the
     resting width so comment text clips instead of rewrapping. -->
<aside transition:slide|global={GROW_X} class="shrink-0 border-l border-line-subtle">
<div class="flex h-full w-80 flex-col">
  <!-- §8 section header row: mono uppercase dim + right-aligned mono meta. -->
  <div class="flex items-center gap-2 border-b border-line-subtle px-3 py-2">
    <MessageSquareText size={13} class="text-muted" />
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Comments</span>
    <span class="flex-1"></span>
    {#if resolved.length > 0}
      <button type="button" onclick={() => (showResolved = !showResolved)} class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg">
        {showResolved ? 'Hide resolved' : `Resolved (${resolved.length})`}
      </button>
    {/if}
    <CloseButton onClick={onClose} size={13} class="h-6 w-6 rounded p-0" />
  </div>
  <div class="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3">
    {#if loadFailed}
      <QueryError variant="inline" error={loadError} title="Could not load comments" onRetry={onRetryLoad} />
    {:else if open.length === 0 && !showResolved}
      <div class="font-sans text-xs text-muted">No open threads. Select text in the doc to comment on it.</div>
    {/if}
    {#each open as c (c.id)}
      {@render thread(c)}
    {/each}
    {#if showResolved}
      {#each resolved as c (c.id)}
        {@render thread(c)}
      {/each}
    {/if}
  </div>
  <div class="border-t border-line-subtle p-3">
    {#if pendingQuote}
      <div transition:slide={{ duration: 150 }} class="mb-1.5 flex items-start gap-1.5 border-l-2 border-warning/50 pl-2 font-sans text-[11px] italic text-muted">
        <span class="min-w-0 flex-1 line-clamp-2">“{pendingQuote}”</span>
        <CloseButton onClick={onQuoteConsumed} size={11} class="shrink-0 p-0 hover:bg-transparent" />
      </div>
    {/if}
    <Textarea
      autoGrow
      rows={1}
      bind:value={draft}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          void post(draft, null, pendingQuote).then(() => {
            draft = ''
            onQuoteConsumed()
          })
        }
      }}
      placeholder={pendingQuote ? 'Comment on the selection' : 'Start a thread'}
      class="text-sm"
    />
  </div>
</div>
</aside>
