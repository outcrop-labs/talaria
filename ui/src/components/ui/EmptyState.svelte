<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import DitherLayer from './DitherLayer.svelte'
  import type { DitherSource } from '@/lib/dither'

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
    vignette,
    class: className,
  }: {
    icon?: string | Snippet
    title: string
    hint?: string
    action?: Snippet
    variant?: 'full' | 'compact' | 'inline'
    /**
     * A dithered vignette behind the words — texture where there is nothing.
     *
     * ON BY DEFAULT FOR `full`, which is the variant that owns a whole pane.
     * A large surface with three lines centred in it reads as a dead void; the
     * field gives it material without faking content. `compact` and `inline`
     * sit inside something that already has texture of its own, so they stay
     * plain and cost nothing.
     *
     * THIS IS THE RESOLVED-EMPTY RENDERING AND ONLY THAT. Empty, broken and
     * loading are three different answers — a failed read renders a
     * QueryError, and a read still in flight renders a skeleton (which now
     * carries its own field, SIGNAL STATIC). Putting a vignette on this one
     * must not make it look like either of those.
     */
    vignette?: boolean
    class?: string
  } = $props()

  const wantsVignette = $derived(vignette ?? variant === 'full')

  // Four edges, inward. The centre stays clear on purpose: density belongs at
  // the boundary, where it says "this surface is real", and away from the
  // words, which have to stay the most legible thing in the frame.
  const sources: DitherSource[] = [
    { id: 'n', kind: 'edge', side: 'top', depth: 60, strength: 0.28 },
    { id: 's', kind: 'edge', side: 'bottom', depth: 60, strength: 0.28 },
    { id: 'w', kind: 'edge', side: 'left', depth: 80, strength: 0.2 },
    { id: 'e', kind: 'edge', side: 'right', depth: 80, strength: 0.2 },
  ]
</script>

<!-- Spec §2: empty-state copy is reading voice — sans, never the mono chrome
     voice the app shell inherits from the base font. -->
{#if variant === 'inline'}
  <div class={cn('font-sans text-xs text-muted', className)}>
    {title}{hint ? ` — ${hint}` : ''}
  </div>
{:else}
  <div
    class={cn(
      variant === 'full' ? 'grid h-full place-items-center p-6' : 'px-2 py-6',
      'text-center',
      wantsVignette && 'relative overflow-hidden',
      className,
    )}
  >
    {#if wantsVignette}<DitherLayer {sources} organic={0.55} />{/if}
    <div class={cn('relative max-w-xs', variant === 'compact' && 'mx-auto')}>
      <div class={cn('mx-auto text-ink-dim', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>
        {#if typeof icon === 'string'}{icon}{:else}{@render icon()}{/if}
      </div>
      <div class={cn('font-sans font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
      {#if hint}<div class="mt-1 font-sans text-xs leading-5 text-muted">{hint}</div>{/if}
      {#if action}<div class={variant === 'full' ? 'mt-4' : 'mt-3'}>{@render action()}</div>{/if}
    </div>
  </div>
{/if}
