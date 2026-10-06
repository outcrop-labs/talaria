<script lang="ts" module>
  /** The hover bar's one-click reactions (KTD2) — anything else is one more
   *  click away in the full picker. */
  export const QUICK_REACTIONS = ['✅', '👀', '🙌'] as const
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import { MessageSquareText, Pencil, SmilePlus, Trash2 } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import { cn } from '@/lib/cn'

  // The floating per-message toolbar (R4 / KTD2), shared by channel rows and
  // agent-DM turns: quick reactions · full emoji picker · reply-in-thread ·
  // edit · delete. Each action renders only when its handler is supplied, so a
  // surface opts in to exactly what it supports.
  //
  // VISIBILITY CONTRACT: the caller's row carries the `group/message` class
  // (and `relative`, for the top-right pin). The bar shows on row hover, on
  // keyboard focus anywhere in the row, and stays pinned while the picker is
  // open — the picker's trigger reports `aria-expanded`, which the bar reads
  // with `:has()`, so no open state needs threading through the caller.
  let {
    onReact,
    onReply,
    onEdit,
    onDelete,
    quick = QUICK_REACTIONS,
    extra,
    class: className,
  }: {
    /** Toggle the viewer's reaction — from a quick button or the picker. */
    onReact?: (emoji: string) => void
    /** Reply in thread; omitted → no thread button. */
    onReply?: () => void
    /** Edit (own messages); omitted → no edit button. */
    onEdit?: () => void
    /** Delete (own or owner); omitted → no delete button. The caller owns
     *  the confirm. */
    onDelete?: () => void
    quick?: readonly string[]
    /** Extra trailing buttons (IconButton size="sm"). */
    extra?: Snippet
    class?: string
  } = $props()
</script>

<!-- role=toolbar: a keyboard user tabbing into the row lands on these. -->
<div
  role="toolbar"
  aria-label="Message actions"
  class={cn(
    'absolute -top-3 right-2 z-[1] hidden items-center gap-0.5 rounded-md border border-line bg-raised p-0.5 shadow-[var(--theme-shadow-1)]',
    'group-hover/message:flex group-focus-within/message:flex has-[[aria-expanded=true]]:flex',
    className,
  )}
>
  {#if onReact}
    {#each quick as e (e)}
      <IconButton title={`React ${e}`} size="sm" class="text-sm leading-none" onclick={() => onReact?.(e)}>
        {e}
      </IconButton>
    {/each}
    <EmojiPicker align="right" onPick={(e) => onReact?.(e)}>
      {#snippet trigger(open)}
        <IconButton title="Add reaction" size="sm" active={open} aria-expanded={open}>
          <SmilePlus size={14} />
        </IconButton>
      {/snippet}
    </EmojiPicker>
  {/if}
  {#if onReply}
    <IconButton title="Reply in thread" size="sm" onclick={() => onReply?.()}>
      <MessageSquareText size={14} />
    </IconButton>
  {/if}
  {#if onEdit}
    <IconButton title="Edit message" size="sm" onclick={() => onEdit?.()}>
      <Pencil size={14} />
    </IconButton>
  {/if}
  {#if onDelete}
    <IconButton title="Delete message" size="sm" danger onclick={() => onDelete?.()}>
      <Trash2 size={14} />
    </IconButton>
  {/if}
  {@render extra?.()}
</div>
