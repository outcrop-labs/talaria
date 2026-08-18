<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { ChevronDown } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  // FieldPill — the clickable-inline-value affordance. A property shown as a
  // quiet pill that REVEALS its editability: border + chevron surface on hover,
  // cursor says clickable, active state while its picker is open. Use it as the
  // trigger for DropdownMenu pickers on rows, cards, and property rails —
  // never a bare <select>/<input> masquerading as text.
  interface Props extends Omit<HTMLButtonAttributes, 'children'> {
    /** Leading dot color (status/priority) — omit for none. */
    dot?: string
    icon?: Snippet
    /** Picker-open state (from DropdownMenu's trigger render prop). */
    active?: boolean
    /** Muted placeholder styling when the field is unset. */
    empty?: boolean
    /** Always look like a control: visible border + chevron at rest. Use on
     *  cards, where hover-reveal made editability a mystery. */
    persistent?: boolean
    children: Snippet
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
  }

  let {
    dot,
    icon,
    active,
    empty,
    persistent,
    class: className,
    children,
    ref = $bindable(null),
    ...rest
  }: Props = $props()

  // Stop drag-start on draggable cards; the CLICK is left to bubble one
  // level — DropdownMenu's trigger wrapper owns it (and stops it there,
  // so rows/cards underneath never fire).
  const stopDrag = (e: MouseEvent) => e.stopPropagation()
</script>

<button
  bind:this={ref}
  type="button"
  onmousedown={stopDrag}
  class={cn(
    // Chip-family control (spec §8): radius 6, raised tile when engaged,
    // hover fill card2, dashed-gold keyboard focus. Chip chrome voice
    // (spec §2): mono UPPERCASE + letterspacing — never italic.
    'group/pill inline-flex max-w-full cursor-pointer items-center gap-1.5 rounded-md border px-1.5 py-0.5 text-left font-mono text-[10px] uppercase leading-3 tracking-[0.05em] transition-colors',
    focusGold,
    active
      ? 'border-line-strong bg-raised text-fg'
      : persistent
        ? 'border-line-subtle bg-raised/40 text-muted hover:border-line hover:dither-fill hover:text-fg'
        : 'border-transparent text-muted hover:border-line hover:dither-fill hover:text-fg',
    // Unset placeholder: dimmest ink, same voice (no italics in Mercury).
    !active && empty && 'text-ink-dim',
    className,
  )}
  {...rest}
>
  {#if dot}<span class="h-2 w-2 shrink-0 rounded-full" style:background={dot}></span>{/if}
  {#if icon}<span class="grid w-3.5 shrink-0 place-items-center">{@render icon()}</span>{/if}
  <span class="min-w-0 truncate">{@render children()}</span>
  <ChevronDown
    size={11}
    class={cn(
      'shrink-0 transition-opacity',
      active ? 'opacity-100' : persistent ? 'opacity-50 group-hover/pill:opacity-100' : 'opacity-0 group-hover/pill:opacity-100',
    )}
  />
</button>
