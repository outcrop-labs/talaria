<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // The end of an alert row: the word and a chevron, read as one link. The
  // row is the button, so this is a mark and takes no click of its own. Any
  // other row names its action with `SettingsRowAction`, or opens.
  let {
    text = 'Fix',
    class: className,
  }: {
    /** An alert that has nothing to fix says another word ("Review"). */
    text?: string
    class?: string
  } = $props()
</script>

<!-- A fixed cell with the word at its right, so down a list every Fix ends in one place. -->
<span
  data-row-fix
  class={cn('inline-flex h-8 min-w-12 shrink-0 items-center justify-end gap-1 font-sans text-[13px] font-medium leading-5 text-fg', className)}
>{text}<ChevronRight size={14} aria-hidden="true" class="text-muted transition-colors" /></span>
