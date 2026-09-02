<script lang="ts">
  import { cn } from '@/lib/cn'
  import WingMark from './WingMark.svelte'

  // The Talaria brand: the gold three-stroke wing mark, on its own.
  //
  // The spaced-caps wordmark that used to sit beside it is gone — the mark
  // carries the brand alone now. The collapsed rail had already been doing
  // exactly this (NavRail renders a bare WingMark at its own size), so the two states
  // of the sidebar now show the same thing at the same size instead of the
  // expanded one growing a word.
  //
  // THE NAME SURVIVES FOR SCREEN READERS. WingMark is `aria-hidden` — it is a
  // decorative path — so deleting the visible text would have left the brand
  // with no accessible name at all, in five places including the login and
  // join pages where it is the only thing identifying the product. `sr-only`
  // keeps that without putting the word back on screen.
  //
  // `size` exists because a lockup and a lone mark do not want the same scale:
  // 20px reads correctly beside a wordmark and in a 24px rail row, and reads as
  // an afterthought centred in a login panel with nothing next to it.
  let {
    class: className,
    showTag = false,
    size = 20,
  }: { class?: string; showTag?: boolean; size?: number } = $props()
</script>

<div class={cn('flex items-center gap-2.5', className)}>
  <WingMark {size} />
  <span class="sr-only">Talaria</span>
  {#if showTag}
    <div class="theme-muted text-[11px] tracking-wide">get things done together</div>
  {/if}
</div>
