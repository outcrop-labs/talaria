<script lang="ts">
  import MessageAvatar from './MessageAvatar.svelte'
  import MessageActions from './MessageActions.svelte'
  import ReactionChips, { type ReactionChip } from './ReactionChips.svelte'
  import MessageAttachments from '@/components/chat/MessageAttachments.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import ChatChips from './ChatChips.svelte'
  import type { ChatChip } from '@/lib/chips'
  import { fade } from '@/lib/motion'
  import type { Attachment } from '@/lib/attachments'

  // Flattened user turn (spec §10): avatar square + name + 10px mono time,
  // 14px sans body — no heavy bubble. Same row chrome as a channel message
  // (MessageRow): hover/focus highlight, the shared action bar, reaction chips.
  let {
    content,
    attachments,
    chips,
    author,
    picture,
    time,
    reactions = [],
    onReact,
    onContextMenu,
    onInvoke,
    onDecided,
  }: {
    content: string
    attachments?: Attachment[]
    chips?: ChatChip[]
    author?: string | null
    /** The author's photo (R3) — initials when absent. */
    picture?: string | null
    /** Send time beside the name (R2); empty → none shown. */
    time?: string
    reactions?: ReactionChip[]
    /** Toggle the viewer's reaction. Omitted → no action bar (a row with no
     *  server id yet has nothing to react to). */
    onReact?: (emoji: string) => void
    onContextMenu?: (e: MouseEvent) => void
    onInvoke?: (text: string) => void
    onDecided?: () => void
  } = $props()

  const name = $derived(author ?? 'You')
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the message context menu has no keyboard path -->
<div
  in:fade={{ duration: 150 }}
  class="group/message relative -mx-2 flex gap-2.5 rounded-md px-2 py-1 transition-colors hover:bg-card2 focus-within:bg-card2"
  oncontextmenu={onContextMenu}
>
  <MessageAvatar {name} src={picture} class="mt-0.5" />
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{name}</span>
      {#if time}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{time}</span>{/if}
    </div>
    <div class="font-sans text-sm text-fg">
      <!-- Markdown, like every other message surface — a user turn was the
          one bubble still rendering raw text. -->
      <Markdown children={content} />
      {#if attachments && attachments.length > 0}<MessageAttachments items={attachments} />{/if}
      <ChatChips {chips} {content} {onInvoke} {onDecided} />
    </div>
    {#if onReact}<ReactionChips {reactions} onToggle={onReact} />{/if}
  </div>
  <!-- Reactions only (R6): an agent DM has no threads, edits or deletes. -->
  {#if onReact}<MessageActions {onReact} />{/if}
</div>
