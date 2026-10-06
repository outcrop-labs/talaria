<script lang="ts">
  import { typingSentence } from '@/lib/comms-typing.svelte'
  import TypingDots from './TypingDots.svelte'

  // "Maya is typing" with the dots — the line just above a composer. Out of
  // flow (the parent positions it), so the composer never moves when it shows.
  let { names, class: className = '' }: { names: string[]; class?: string } = $props()
</script>

{#if names.length > 0}
  <div aria-live="polite" class={`pointer-events-none flex items-center gap-1.5 font-sans text-xs text-muted ${className}`}>
    <TypingDots class="mb-[3px]" />
    <span class="font-medium text-fg">{typingSentence(names)}</span>
  </div>
{/if}
