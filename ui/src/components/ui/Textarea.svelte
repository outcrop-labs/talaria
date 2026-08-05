<script lang="ts">
  import type { HTMLTextareaAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'

  type Props = HTMLTextareaAttributes & {
    /** Grow downward with content: starts at `rows`, expands as the user types,
     *  capped by any `max-h-*` in className (then scrolls). */
    autoGrow?: boolean
    ref?: HTMLTextAreaElement | null
  }

  let {
    class: className,
    autoGrow,
    value = $bindable(),
    ref = $bindable(null),
    ...rest
  }: Props = $props()

  // Fit height to content before paint (bound and unbound callers both pass
  // through here on every value change — bind:value below keeps the local
  // `value` in sync even when the parent doesn't bind it).
  $effect(() => {
    void value
    const el = ref
    if (!autoGrow || !el) return
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight + 2}px` // +2: box border
  })
</script>

<!-- The one textarea. Reuse everywhere — do not re-style textareas inline. -->
<textarea
  bind:this={ref}
  bind:value
  class={cn(
    // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
    'w-full resize-none rounded-md border border-line bg-[var(--theme-input)] px-3 py-2.5 font-sans text-sm text-fg outline-none transition-colors',
    'placeholder:text-muted focus:border-accent focus:ring-2 focus:ring-accent-soft',
    autoGrow && 'overflow-y-auto',
    className,
  )}
  {...rest}
></textarea>
