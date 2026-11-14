<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // The one action a row shows when its state needs a person: the word and a
  // chevron, never a boxed button. The row's status says what is wrong, and
  // the kebab holds the same action under its own name.
  let {
    onclick,
    href,
    label,
    title,
    disabled = false,
    text = 'Fix',
    class: className,
  }: {
    /** What Fix does. With neither this nor `href` it is a mark, for a row
     *  that is itself the button (an alert row). */
    onclick?: () => void
    href?: string
    /** The accessible name: what is fixed and how ("Fix QuickBooks Online:
     *  set credentials"). */
    label?: string
    title?: string
    disabled?: boolean
    /** Only an alert that has nothing to fix says another word ("Review"). */
    text?: string
    class?: string
  } = $props()

  // A fixed cell with the word at its right, so down a list every Fix ends
  // in one place, ahead of the kebab.
  const cell = 'inline-flex h-8 min-w-12 shrink-0 items-center justify-end gap-1 font-sans text-[13px] font-medium leading-5 text-fg'
  const live =
    'rounded-md underline-offset-2 transition-colors hover:underline focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40 [&:hover>svg]:text-fg'
</script>

{#snippet body()}{text}<ChevronRight size={14} aria-hidden="true" class="text-muted transition-colors" />{/snippet}

{#if href}
  <a data-row-fix {href} {title} aria-label={label} class={cn(cell, live, className)}>{@render body()}</a>
{:else if onclick}
  <button data-row-fix type="button" {onclick} {title} {disabled} aria-label={label} class={cn(cell, live, className)}>{@render body()}</button>
{:else}
  <span data-row-fix class={cn(cell, className)}>{@render body()}</span>
{/if}
