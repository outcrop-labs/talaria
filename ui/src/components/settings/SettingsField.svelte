<script lang="ts">
  import type { Snippet } from 'svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'

  // One field of a form in a dialog, where there is no room for a row: the
  // label, the control under it, then at most one sentence.
  let {
    label,
    hint,
    info,
    for: htmlFor,
    class: className,
    children,
  }: {
    label: string
    hint?: string
    /** The longer explanation, behind an InfoTip beside the label. */
    info?: string
    /** The control's id, so the label focuses it. */
    for?: string
    class?: string
    children: Snippet
  } = $props()
</script>

<div class={cn('space-y-1.5', className)}>
  <div class="flex items-center gap-2">
    <label for={htmlFor} class="font-sans text-[13px] font-medium leading-5 text-fg">{label}</label>
    {#if info}<InfoTip text={info} />{/if}
  </div>
  {@render children()}
  {#if hint}<p class="font-sans text-[13px] leading-5 text-muted">{hint}</p>{/if}
</div>
