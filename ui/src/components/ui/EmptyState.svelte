<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // The one empty/zero state. Centered mark + short line + optional single
  // action. Reuse for every no-data view — don't hand-roll bare "No X yet"
  // strings. `compact` for zero states inside panels/lists (no full-height
  // centering, smaller mark); `inline` for a single quiet line.
  let {
    icon = '◇',
    title,
    hint,
    action,
    variant = 'full',
    class: className,
  }: {
    icon?: string | Snippet
    title: string
    hint?: string
    action?: Snippet
    variant?: 'full' | 'compact' | 'inline'
    class?: string
  } = $props()
</script>

<!-- Spec §2: empty-state copy is reading voice — sans, never the mono chrome
     voice the app shell inherits from the base font. -->
{#if variant === 'inline'}
  <div class={cn('font-sans text-xs text-muted', className)}>
    {title}{hint ? ` — ${hint}` : ''}
  </div>
{:else}
  <div class={cn(variant === 'full' ? 'grid h-full place-items-center p-6' : 'px-2 py-6', 'text-center', className)}>
    <div class={cn('max-w-xs', variant === 'compact' && 'mx-auto')}>
      <div class={cn('mx-auto text-ink-dim', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>
        {#if typeof icon === 'string'}{icon}{:else}{@render icon()}{/if}
      </div>
      <div class={cn('font-sans font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
      {#if hint}<div class="mt-1 font-sans text-xs leading-5 text-muted">{hint}</div>{/if}
      {#if action}<div class={variant === 'full' ? 'mt-4' : 'mt-3'}>{@render action()}</div>{/if}
    </div>
  </div>
{/if}
