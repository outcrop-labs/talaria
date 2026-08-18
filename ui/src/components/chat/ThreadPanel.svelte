<script lang="ts">
  import { MessageSquareText, X } from '@lucide/svelte'
  import MessageRow from './MessageRow.svelte'
  import ChannelComposer from './ChannelComposer.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { slide, GROW_X } from '@/lib/motion'
  import { sendChannelMessage, useThreadMessages } from '@/lib/channels.svelte'
  import { splitAttachments, type Attachment } from '@/lib/attachments'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import type { MessageCtx } from './channel-view'

  // The thread side panel: root + replies + its own composer. Replies are
  // channel messages that hang off the root; @mentioned agents answer HERE.
  let {
    channelId,
    rootId,
    ctx,
    mentionables,
    onClose,
  }: {
    channelId: string
    rootId: string
    ctx: MessageCtx
    mentionables: Mentionable[]
    onClose: () => void
  } = $props()

  // Same trap one level down: `= []` on a rejected thread read rendered an
  // EMPTY thread panel — no root, no replies, no error — beside a rollup that
  // had just said "3 replies".
  const threadQuery = useThreadMessages(() => channelId, () => rootId)
  const messages = $derived(threadQuery.data ?? [])
  let scrollEl = $state<HTMLDivElement | null>(null)
  let prevCount = 0

  $effect(() => {
    void messages
    const el = scrollEl
    if (!el) return
    const loaded = prevCount === 0 && messages.length > 0
    prevCount = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
  })

  const send = async (text: string, atts: Attachment[]) => {
    const { attachmentIds, refs } = splitAttachments(atts)
    await sendChannelMessage(channelId, text, attachmentIds, refs, rootId)
  }
</script>

<!-- Thread side panel — a proper panel surface (spec §8) beside the feed.
     IN-FLOW panel: slide={GROW_X} on both legs so the feed glides as the panel
     grows/shrinks instead of snapping when the width lands (ANIMATIONS.md).
     |global: ChannelView mounts this whole component per thread, so local legs
     on the component root never play (the |global rule). The inner wrapper is
     pinned to the resting width so message text clips instead of rewrapping. -->
<div transition:slide|global={GROW_X} class="shrink-0 border-l border-line bg-panel">
<div class="flex h-full w-[380px] flex-col">
  <div class="flex items-center gap-2 border-b border-line px-4 py-2.5">
    <MessageSquareText size={14} class="text-muted" />
    <span class="font-sans text-sm font-semibold text-fg">Thread</span>
    <button data-dither-fill
      type="button"
      onclick={onClose}
      class="ml-auto grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:text-fg"
      title="Close thread"
    >
      <X size={14} />
    </button>
  </div>
  <div bind:this={scrollEl} class="flex-1 space-y-4 overflow-y-auto px-4 py-4">
    {#if threadQuery.isError && threadQuery.data !== undefined}
      <QueryError
        variant="inline"
        title="Thread may be out of date"
        error={threadQuery.error}
        onRetry={() => void threadQuery.refetch()}
      />
    {/if}
    {#if threadQuery.isError && threadQuery.data === undefined}
      <QueryError
        variant="compact"
        title="Could not load this thread"
        error={threadQuery.error}
        onRetry={() => void threadQuery.refetch()}
      />
    {:else if threadQuery.isLoading}
      <div aria-hidden="true" class="space-y-4">
        {#each Array.from({ length: 3 }) as _, i (i)}
          <div class="flex gap-2.5">
            <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" />
            <div class="min-w-0 flex-1 space-y-2 pt-1">
              <Skeleton class="h-2.5 w-20 rounded-full" />
              <Skeleton class="h-2.5 w-4/5 rounded-full" />
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <!-- Keyed on the root so a warm-cache thread switch (no skeleton pass)
          rebuilds the list instead of cross-fading two threads (see
          ChannelView's message list). -->
      {#key rootId}
        {#each messages as m, i (m.id)}
          <div>
            <MessageRow message={m} {ctx} inThread />
            {#if i === 0 && messages.length > 1}
              <div class="mt-3 border-t border-line pt-1 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
                {messages.length - 1} {messages.length - 1 === 1 ? 'reply' : 'replies'}
              </div>
            {/if}
          </div>
        {/each}
      {/key}
    {/if}
  </div>
  <ChannelComposer channelName="thread" placeholder="Reply in thread. @mention an agent to bring it in" {mentionables} onSend={send} />
</div>
</div>
