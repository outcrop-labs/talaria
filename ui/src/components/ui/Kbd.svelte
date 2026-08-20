<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // Keyboard-key chip (⌘, ⏎, esc) for shortcut hints in menus and chrome.
  // Mercury: raised tile + hairline + 10px mono chrome voice.
  let { children, class: className }: { children: Snippet; class?: string } = $props()
</script>

<kbd
  class={cn(
    'inline-grid h-5 min-w-5 place-items-center rounded-md border border-line bg-raised px-1.5 font-mono text-[10px] leading-none tracking-[0.05em] text-muted',
    className,
  )}
>
  {@render children()}
</kbd>
