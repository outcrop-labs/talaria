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
    onContextMenu,
  }: {
    content: string
    attachments?: Attachment[]
    author?: string | null
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  const name = $derived(author ?? 'You')
</script>

<div in:fade={{ duration: 150 }} class="flex gap-2.5" oncontextmenu={onContextMenu}>
  <MessageAvatar {name} class="mt-0.5" />
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{name}</span>
    </div>
    <div class="font-sans text-sm text-fg">
      <!-- Markdown, like every other message surface — a user turn was the
          one bubble still rendering raw text. -->
      <Markdown children={content} />
      {#if attachments && attachments.length > 0}<MessageAttachments items={attachments} />{/if}
    </div>
  </div>
</div>
