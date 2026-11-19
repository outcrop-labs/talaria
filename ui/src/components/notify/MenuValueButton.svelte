<script lang="ts">
  import type { Component } from 'svelte'
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { ChevronDown } from '@lucide/svelte'
  import { controlSizes, disabledControl } from '@/components/ui/control'
  import { cn } from '@/lib/cn'

  // The trigger of a menu that sets a value: it shows the value in words, as
  // a select does, and takes the frame of the area's select so the two read as
  // one control. Render it in a `DropdownMenu`'s `trigger` snippet.
  let {
    label,
    icon: Icon,
    open,
    class: className,
    ...rest
  }: HTMLButtonAttributes & {
    /** The current value, in words. */
    label: string
    icon?: Component<{ size?: number; strokeWidth?: number; class?: string }>
    open: boolean
    class?: string
  } = $props()
</script>

<button
  type="button"
  aria-haspopup="menu"
  aria-expanded={open}
  class={cn(
    controlSizes.sm,
    'flex w-[280px] max-w-full items-center gap-2 rounded-md border border-line bg-[var(--theme-input)] px-2.5 font-sans text-sm text-fg outline-none transition-colors hover:border-line-strong focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent-soft',
    open && 'border-line-strong',
    disabledControl,
    className,
  )}
  {...rest}
>
  {#if Icon}<Icon size={16} strokeWidth={1.5} class="shrink-0 text-muted" />{/if}
  <span class="min-w-0 flex-1 truncate text-left">{label}</span>
  <ChevronDown size={16} strokeWidth={1.5} class="shrink-0 text-muted" />
</button>
