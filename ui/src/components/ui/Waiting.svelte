<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { WaitingSiteRef } from '@/lib/waiting/session.svelte'
  import WaitingMark from './WaitingMark.svelte'

  /**
   * A waiting mark with the label that carries its meaning.
   *
   * The mark is decorative and `aria-hidden` (a screen reader must never be
   * handed a run of U+2800 codepoints); the LABEL is the live region. Keeping
   * that pairing in one component is what stops a surface from shipping a mark
   * with no announcement, or two live regions competing to announce the same
   * one thing.
   *
   * Use `<WaitingMark>` directly only where the surrounding row already says
   * what is happening in text — a button whose own label is "Saving", a status
   * line that renders its own copy. Everywhere else, use this.
   */
  let {
    site,
    label,
    size = 14,
    class: className,
  }: {
    site: WaitingSiteRef
    label: string
    size?: number
    class?: string
  } = $props()
</script>

<div class={cn('flex items-center gap-2.5', className)}>
  <WaitingMark {site} {size} class="text-accent" />
  <span role="status" class="font-sans text-[13px] text-muted">{label}</span>
</div>
