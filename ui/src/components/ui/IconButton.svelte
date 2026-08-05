<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import { focusRing } from './control'

  // The compact control for contextual actions — an icon with a tooltip, not a
  // labeled button. Use wherever placement implies the verb (rail headers,
  // surface headers, row hover actions); reserve labeled <Button>s for a page's
  // one or two primary actions and anything destructive/deploying.
  interface Props extends HTMLButtonAttributes {
    /** Tooltip — REQUIRED: the icon carries no label. */
    title: string
    active?: boolean
    danger?: boolean
    size?: 'sm' | 'md'
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
  }

  let {
    title,
    active,
    danger,
    size = 'md',
    class: className,
    children,
    ref = $bindable(null),
    ...rest
  }: Props = $props()
</script>

<button
  bind:this={ref}
  type="button"
  {title}
  aria-label={title}
  class={cn(
    // Mercury icon tile (spec §8): radius 6, raised when active,
    // hover-token fill, danger only ever tints the glyph — never a fill.
    'grid shrink-0 place-items-center rounded-md transition-colors disabled:opacity-40',
    focusRing,
    size === 'sm' ? 'h-7 w-7' : 'h-8 w-8',
    active ? 'bg-raised text-fg' : 'text-muted hover:bg-hover',
    danger ? 'hover:text-danger' : 'hover:text-fg',
    className,
  )}
  {...rest}
>
  {@render children?.()}
</button>
