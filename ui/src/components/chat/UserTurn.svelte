<script lang="ts">
  import MessageAvatar from './MessageAvatar.svelte'
  import MessageAttachments from '@/components/chat/MessageAttachments.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { fade } from '@/lib/motion'
  import type { Attachment } from '@/lib/attachments'

  // Flattened user turn (spec §10): avatar square + name row, 14px sans body —
  // no heavy bubble.
  let {
    content,
    attachments,
    author,
    queued = false,
    onContextMenu,
  }: {
    content: string
    attachments?: Attachment[]
    author?: string | null
    /** The turn ahead is still in flight — this message is queued, not lost.
     *  Rendered the moment it is sent (no waiting on a poll). */
    queued?: boolean
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  const name = $derived(author ?? 'You')
</script>

<div in:fade={{ duration: 150 }} class="flex gap-2.5" oncontextmenu={onContextMenu}>
  <MessageAvatar {name} class="mt-0.5" />
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{name}</span>
      {#if queued}
        <!-- Explicit queued state: the message visibly waits behind the turn
            in flight instead of reading as delivered-and-ignored. -->
        <span class="rounded border border-line px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">queued</span>
      {/if}
    </div>
    <div class="font-sans text-sm text-fg">
      <!-- Markdown, like every other message surface — a user turn was the
          one bubble still rendering raw text. -->
      <Markdown children={content} />
      {#if attachments && attachments.length > 0}<MessageAttachments items={attachments} />{/if}
    </div>
  </div>
</div>
