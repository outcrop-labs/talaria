<script lang="ts">
  import { Maximize2, Minimize2, X } from '@lucide/svelte'
  import BriefApproval from '@/components/brief/BriefApproval.svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { cn } from '@/lib/cn'
  import { approvalOf, blockerOf, type InboxCard } from '@/lib/inbox-board'
  import { fade, fly, PANEL_X, QUICK } from '@/lib/motion'
  import { navigateHref } from '@/router'
  import CardActions from './CardActions.svelte'
  import DrawerProperties from './DrawerProperties.svelte'
  import DrawerSections from './DrawerSections.svelte'
  import StateGlyph from './StateGlyph.svelte'
  import { useDrawerDetail } from './drawer-detail.svelte'
  import { ACTION, BREAK, DRAWER_PX, GHOST, type BoardActions } from './inbox-board.svelte'

  let {
    card,
    now,
    full,
    actions,
    onClose,
    onGone,
    onToggleFull,
    onAsk,
  }: {
    card: InboxCard
    now: number
    full: boolean
    actions: BoardActions
    onClose: () => void
    /** Its fade-out ended: it no longer takes room beside the page. */
    onGone: () => void
    onToggleFull: () => void
    /** Hand the card's brief line to the sidebar assistant. */
    onAsk?: () => void
  } = $props()

  let root = $state<HTMLElement | null>(null)
  $effect(() => {
    root?.focus({ preventScroll: true })
  })

  const detail = useDrawerDetail(() => card)
  const approval = $derived(approvalOf(card))
  const blocker = $derived(blockerOf(card))
  const body = $derived(detail.body === blocker?.text ? null : detail.body)
  const external = $derived(!!card.href && /^https?:\/\//.test(card.href))
  const FRAME = 'mx-auto w-full max-w-[calc(47.5rem+5rem)] px-10'

  function openPage(): void {
    if (!card.href) return
    if (external) window.open(card.href, '_blank', 'noopener,noreferrer')
    else void navigateHref(card.href)
  }

  let menuWasOpen = false

  function onkeydowncapture(e: KeyboardEvent): void {
    if (e.key === 'Escape') menuWasOpen = !!root?.querySelector('[aria-haspopup="menu"][aria-expanded="true"]')
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape' && !e.defaultPrevented && !menuWasOpen) onClose()
  }
</script>

<svelte:window {onkeydowncapture} {onkeydown} />

<div
  bind:this={root}
  data-inbox-drawer
  data-inbox-surface
  data-full={full || undefined}
  role="dialog"
  aria-modal="false"
  aria-labelledby="inbox-drawer-title"
  tabindex="-1"
  in:fly|global={PANEL_X}
  out:fade|global={QUICK}
  onoutroend={onGone}
  style:width={full ? undefined : `min(${DRAWER_PX}px, 100%)`}
  class={cn('group/drawer flex min-w-0 flex-col border-l border-line bg-panel outline-none', full ? 'flex-1 border-l-0' : 'shrink-0')}
>
  <header class="flex min-h-12 shrink-0 items-center border-b border-line py-1.5">
    <div class={cn('flex items-center gap-1', full ? FRAME : 'w-full px-5')}>
      <div class="-ml-1.5 flex shrink-0 items-center gap-0.5">
        <IconButton size="sm" title="Close" onclick={onClose}><X size={14} /></IconButton>
        <Button {...ACTION} variant="ghost" data-inbox-drawer-expand class={GHOST} onclick={onToggleFull}>
          {#if full}<Minimize2 size={13} /> Exit full screen{:else}<Maximize2 size={13} /> Full screen{/if}
        </Button>
      </div>
      <div class="-mr-2 ml-auto">
        <CardActions
          {card}
          {actions}
          {onAsk}
          onOpen={card.href ? openPage : undefined}
          openTitle={external ? 'Opens outside Talaria' : undefined}
        />
      </div>
    </div>
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto">
    <article class={cn('pb-12', full ? cn(FRAME, 'pt-6') : 'px-5 pt-5')}>
      <h2
        id="inbox-drawer-title"
        class={cn('text-balance break-words font-sans font-semibold tracking-[-0.01em] text-fg', full ? 'text-[24px] leading-8' : 'text-[19px] leading-[1.3]')}
      >
        {card.title}
      </h2>
      {#if blocker}
        <div class="mt-3 rounded-lg border border-danger/30 bg-[var(--inbox-danger-wash)] px-3.5 py-2.5 font-sans">
          <p class="flex items-center gap-2 text-[13px] font-medium leading-5 text-danger"><StateGlyph glyph={blocker.glyph} /> {blocker.label}</p>
          {#if blocker.text}<p class="mt-0.5 pl-6 text-[13.5px] leading-[21px] text-fg">{blocker.text}</p>{/if}
        </div>
      {/if}
      {#if approval}
        <div class="mt-4"><BriefApproval evidence={approval.evidence} isEvent={(approval.statusLabel ?? '').includes('CREATE EVENT')} /></div>
      {/if}
      <div class="mt-4"><DrawerProperties {card} {now} people={detail.people} due={detail.due} /></div>
      {#if body}
        <div class={cn(BREAK, 'mt-4 font-sans text-[14px] leading-6 text-fg')}>
          <Markdown children={body} />
        </div>
      {/if}
      <DrawerSections {detail} {now} />
    </article>
  </div>
</div>
