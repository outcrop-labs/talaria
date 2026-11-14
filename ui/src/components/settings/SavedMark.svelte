<script lang="ts">
  import { Check } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // "Saved" beside a control that saves on change: `SettingsRow` draws it for
  // its `saved` prop, and a control outside a row (a switch in a grid, a field
  // of a dialog) places it itself. Pass `autosave.saved(key)`, which is only
  // ever true where the page's header shows no autosave cue.
  let { on, class: className }: { on: boolean; class?: string } = $props()
</script>

<!-- Always in the tree, so a screen reader hears the word arrive. -->
<span
  role="status"
  data-row-saved
  class={cn('flex shrink-0 items-center gap-1.5 whitespace-nowrap font-sans text-[13px] font-normal leading-5 text-muted', className)}
>{#if on}<Check size={14} aria-hidden="true" class="text-success" />Saved{/if}</span>
