<script lang="ts" module>
  /** One reaction as a chip draws it — surface-agnostic, so channel messages
   *  and agent-DM turns map their own wire shapes onto it. */
  export interface ReactionChip {
    emoji: string
    count: number
    /** The viewer reacted with this emoji — highlighted, and a click removes it. */
    mine: boolean
    /** Hover text, e.g. the reactors' names. */
    title?: string
  }
</script>

<script lang="ts">
  import { SmilePlus } from '@lucide/svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'

  // Reaction chips under a message (R5 / KTD2): emoji + count, the viewer's
  // own in the accent style; a click toggles the viewer's reaction. A trailing
  // add chip opens the full picker. Renders nothing when there are no
  // reactions — the hover bar is how the first one gets added.
  let {
    reactions,
    onToggle,
    addable = true,
    class: className,
  }: {
    reactions: readonly ReactionChip[]
    onToggle: (emoji: string) => void
    /** Show the trailing add-reaction chip. */
    addable?: boolean
    class?: string
  } = $props()

  const chip = 'flex h-6 items-center gap-1 rounded-md border px-1.5 text-xs transition-colors'
</script>

{#if reactions.length > 0}
  <div in:fade={{ duration: 150 }} out:fade={QUICK} class={cn('mt-1.5 flex flex-wrap items-center gap-1', className)}>
    {#each reactions as r (r.emoji)}
      <button
        in:fade={{ duration: 150 }}
        out:fade={QUICK}
        type="button"
        title={r.title}
        aria-pressed={r.mine}
        onclick={() => onToggle(r.emoji)}
        class={cn(
          chip,
          r.mine ? 'border-accent bg-accent-soft text-fg' : 'border-line bg-raised text-muted dither-fill hover:text-fg',
        )}
      >
        <span>{r.emoji}</span>
        <span class="font-mono text-[10px] tracking-[0.05em]">{r.count}</span>
      </button>
    {/each}
    {#if addable}
      <EmojiPicker onPick={onToggle}>
        {#snippet trigger(open)}
          <button
            type="button"
            title="Add reaction"
            aria-label="Add reaction"
            aria-expanded={open}
            class={cn(chip, 'border-line bg-raised text-muted dither-fill hover:text-fg', open && 'text-fg')}
          >
            <SmilePlus size={13} />
          </button>
        {/snippet}
      </EmojiPicker>
    {/if}
  </div>
{/if}
