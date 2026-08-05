<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /** The one-line surface header: title + meta on the left, actions right.
   *  Same height as the rail header so the top line runs straight across. */
  let {
    title,
    meta,
    actions,
    class: className,
  }: {
    title: string | Snippet
    meta?: string | Snippet
    actions?: Snippet
    class?: string
  } = $props()
</script>

<header class={cn('flex h-12 shrink-0 items-center gap-2 border-b border-line px-5', className)}>
  <span class="min-w-0 shrink truncate font-sans text-sm font-semibold text-fg">
    {#if typeof title === 'string'}{title}{:else}{@render title()}{/if}
  </span>
  {#if meta !== undefined}
    <span class="min-w-0 truncate font-mono text-[11px] text-muted">
      {#if typeof meta === 'string'}{meta}{:else}{@render meta()}{/if}
    </span>
  {/if}
  <span class="ml-auto"></span>
  {@render actions?.()}
</header>
