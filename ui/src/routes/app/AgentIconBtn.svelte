<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // A tiny icon button used for the row/tile controls.
  let {
    title,
    onClick,
    danger,
    disabled,
    children,
  }: {
    title: string
    onClick: () => void
    danger?: boolean
    disabled?: boolean
    children: Snippet
  } = $props()
</script>

<button data-dither-fill
  type="button"
  {title}
  onclick={onClick}
  {disabled}
  class={cn(
    'grid h-8 w-8 place-items-center rounded-md text-muted transition-colors disabled:opacity-40',
    danger ? 'hover:text-danger' : 'hover:text-fg',
  )}
>
  {@render children()}
</button>
