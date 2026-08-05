<script lang="ts">
  import { Info } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { fade, scale, POP, QUICK } from '@/lib/motion'

  // An ⓘ that explains an area on hover — the home for the sentence that used
  // to sit under a section header cluttering it. Keep the text to one or two
  // sentences; anything longer belongs in docs.
  let { text, class: className }: { text: string; class?: string } = $props()

  let open = $state(false)
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class={cn('group relative inline-flex align-middle', className)}
  onmouseenter={() => (open = true)}
  onmouseleave={() => (open = false)}
>
  <Info size={13} class="cursor-help text-muted transition-colors duration-[120ms] group-hover:text-fg" />
  {#if open}
    <span
      role="tooltip"
      in:scale={{ ...POP, start: 0.97 }}
      out:fade={QUICK}
      class="pointer-events-none absolute left-1/2 top-full z-[70] mt-1.5 w-64 origin-top -translate-x-1/2 rounded-lg border border-line bg-panel px-2.5 py-2 font-sans text-[11px] font-normal normal-case leading-snug tracking-normal text-muted shadow-[var(--theme-shadow-2)]"
    >
      {text}
    </span>
  {/if}
</span>
