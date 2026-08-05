<script lang="ts">
  import { Info } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // An ⓘ that explains an area on hover — the home for the sentence that used
  // to sit under a section header cluttering it. Keep the text to one or two
  // sentences; anything longer belongs in docs.
  let { text, class: className }: { text: string; class?: string } = $props()
</script>

<span class={cn('group relative inline-flex align-middle', className)}>
  <Info size={13} class="cursor-help text-muted transition-colors duration-[120ms] group-hover:text-fg" />
  <span
    role="tooltip"
    class="pointer-events-none absolute left-1/2 top-full z-[70] mt-1.5 w-64 -translate-x-1/2 rounded-lg border border-line bg-panel px-2.5 py-2 font-sans text-[11px] font-normal normal-case leading-snug tracking-normal text-muted opacity-0 shadow-[var(--theme-shadow-2)] transition-opacity duration-[120ms] group-hover:opacity-100"
  >
    {text}
  </span>
</span>
