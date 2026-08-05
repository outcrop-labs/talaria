<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import InfoTip from './InfoTip.svelte'

  // The one panel/section header — the §8 canonical: 10px mono UPPERCASE
  // tracking 0.08em ink-dim, with optional InfoTip and a right-aligned mono
  // meta/action slot. Replaces the 19 hand-rolled `flex items-center gap-1.5`
  // clusters.
  let {
    title,
    info,
    action,
    class: className,
  }: {
    title: string | Snippet
    /** InfoTip copy — omit for no tip. */
    info?: string
    /** Right-aligned control or mono meta (button, chip, toggle, `08 LIVE`). */
    action?: string | Snippet
    class?: string
  } = $props()
</script>

<div class={cn('mb-3 flex min-h-6 items-center gap-1.5', className)}>
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    {#if typeof title === 'string'}{title}{:else}{@render title()}{/if}
  </span>
  {#if info}<InfoTip text={info} />{/if}
  {#if action}
    <span class="ml-auto flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
      {#if typeof action === 'string'}{action}{:else}{@render action()}{/if}
    </span>
  {/if}
</div>
