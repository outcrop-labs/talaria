<script lang="ts">
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { slide, GROW_X } from '@/lib/motion'
  import type { Heading } from './knowledge.svelte'

  // The table-of-contents side rail — identical in the space and doc editors.
  let {
    headings,
    onJump,
    onClose,
    emptyText,
  }: {
    headings: Heading[]
    onJump: (index: number) => void
    onClose: () => void
    emptyText: string
  } = $props()
</script>

<!-- IN-FLOW rail: slide={GROW_X} on both legs so the editor glides as the rail
     grows/shrinks instead of snapping (ANIMATIONS.md). |global: the editors
     mount this whole component per toggle, so local legs on the component root
     never play (the |global rule). Inner wrapper pinned to the resting width so
     the empty-state sentence clips instead of rewrapping mid-grow. -->
<div transition:slide|global={GROW_X} class="shrink-0 overflow-y-auto border-l border-line-subtle">
<div class="w-56 p-3">
  <!-- §8 section header: 10px mono uppercase 0.08em ink-dim. -->
  <div class="mb-2 flex items-center justify-between font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    <span>Contents</span>
    <CloseButton onClick={onClose} size={12} class="p-0 hover:bg-transparent" />
  </div>
  {#if headings.length === 0}
    <EmptyState variant="inline" title={emptyText} />
  {:else}
    <div class="space-y-0.5">
      {#each headings as h, i (i)}
        <button
          type="button"
          onclick={() => onJump(i)}
          class="block w-full truncate text-left font-sans text-xs text-muted hover:text-fg"
          style:padding-left="{(h.level - 1) * 10}px"
        >
          {h.text || 'Untitled'}
        </button>
      {/each}
    </div>
  {/if}
</div>
</div>
