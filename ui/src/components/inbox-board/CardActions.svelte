<script lang="ts">
  import type { Component } from 'svelte'
  import { ArrowUpRight, Check, CornerUpLeft, Ellipsis, MessageSquareText, Send, Undo2, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import { approvalOf, cardActions, openLabel, ownersOf, type CardAction, type InboxCard } from '@/lib/inbox-board'
  import { ACTION, FILLED, GHOST, type BoardActions } from './inbox-board.svelte'

  let {
    card,
    actions,
    onAsk,
    onOpen,
    openTitle,
  }: {
    card: InboxCard
    actions: BoardActions
    /** Hand the card's brief line to the sidebar assistant. */
    onAsk?: () => void
    /** Go to the card's own page. */
    onOpen?: () => void
    openTitle?: string
  } = $props()

  const line = $derived(card.line)
  const approval = $derived(approvalOf(card))
  const isEvent = $derived((approval?.statusLabel ?? '').includes('CREATE EVENT'))
  const set = $derived(cardActions(card, { ask: !!onAsk, open: !!onOpen }))
  const owner = $derived(ownersOf(card)?.[0]?.name ?? null)

  let busy = $state<CardAction | null>(null)
  let error = $state<string | null>(null)

  async function run(id: CardAction): Promise<void> {
    if (busy) return
    if (id === 'ask') return onAsk?.()
    if (id === 'open') return onOpen?.()
    busy = id
    error = null
    if (id === 'approve' || id === 'reject') {
      const result = approval?.sourceId ? await actions.decideApproval(approval.sourceId, id) : null
      if (result && !result.ok) error = result.error
    } else if (line) {
      const action = id === 'check' ? 'check' : id === 'dismiss' ? 'dismiss' : 'restore'
      if (!(await actions.markItem(line.key, action))) error = 'That did not go through.'
    }
    busy = null
  }

  type Face = { icon: Component<{ size?: number }>; label: string }
  const FACE: Record<Exclude<CardAction, 'approve' | 'open'>, Face> = {
    check: { icon: Check, label: 'Mark handled' },
    reject: { icon: X, label: 'Reject' },
    restore: { icon: Undo2, label: 'Put back' },
    dismiss: { icon: X, label: 'Remove from inbox' },
    ask: { icon: MessageSquareText, label: 'Ask' },
  }

  function faceOf(id: CardAction, primary: boolean): Face {
    if (id === 'ask' && primary) return { icon: CornerUpLeft, label: owner ? `Reply to ${owner}` : 'Reply' }
    if (id === 'open') return { icon: ArrowUpRight, label: openLabel(card) }
    if (id !== 'approve') return FACE[id]
    const label = busy === 'approve' ? (isEvent ? 'Creating…' : 'Sending…') : isEvent ? 'Approve event' : 'Approve send'
    return { icon: isEvent ? Check : Send, label }
  }

  function overflow(): ContextMenuEntry[] {
    return set.menu.map((id) => {
      const face = faceOf(id, false)
      return { label: face.label, icon: [face.icon, { size: 13 }], disabled: busy !== null, onSelect: () => void run(id) }
    })
  }
</script>

{#if set.bar.length > 0}
  <div class="flex flex-wrap items-center justify-end gap-0.5">
    {#each set.bar as { id, primary } (id)}
      {@const face = faceOf(id, primary)}
      <Button
        {...ACTION}
        variant={primary ? 'primary' : 'ghost'}
        class={primary ? FILLED : cn(GHOST, 'text-fg')}
        disabled={busy !== null && id !== 'ask' && id !== 'open'}
        title={id === 'ask' && !primary ? 'Ask your assistant about this' : id === 'open' ? openTitle : undefined}
        onclick={() => void run(id)}
      >
        <face.icon size={13} />
        {face.label}
      </Button>
    {/each}
    {#if set.menu.length > 0}
      <DropdownMenu items={overflow}>
        {#snippet trigger(open)}
          <IconButton size="sm" title="More actions" class="w-6" active={open} aria-haspopup="menu" aria-expanded={open}>
            <Ellipsis size={14} />
          </IconButton>
        {/snippet}
      </DropdownMenu>
    {/if}
    {#if error}
      <span transition:fade={QUICK} class="font-sans text-[12px] text-[color:var(--theme-danger)]">{error}</span>
    {/if}
  </div>
{/if}
