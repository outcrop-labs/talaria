<script lang="ts">
  import { tick } from 'svelte'
  import { ArrowUpRight, Check, CornerUpLeft, Ellipsis, X } from '@lucide/svelte'
  import BriefReply from '@/components/brief/BriefReply.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Kbd from '@/components/ui/Kbd.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { sendChannelMessage } from '@/lib/channels.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { ago, openLabel, type InboxMessage } from '@/lib/inbox-board'
  import { fade, QUICK, slide } from '@/lib/motion'
  import { wordClamp } from '@/lib/word-clamp'
  import { navigateHref } from '@/router'
  import PersonMark from './PersonMark.svelte'
  import { ACTION, ROW_GRID, type TodayActions } from './inbox-board.svelte'

  let {
    message,
    now,
    actions,
    replying,
    onReply,
    onGone,
  }: {
    message: InboxMessage
    now: number
    actions: TodayActions
    /** This row's reply is the one being written. */
    replying: boolean
    /** Open this row's reply, or close it. */
    onReply: () => void
    /** The message was marked handled or removed. */
    onGone: () => void
  } = $props()

  const INK = 'h-7 gap-1.5 border border-transparent text-fg hover:bg-fg/[0.06]'
  const WORD = 'hidden @lg:inline'
  const UNDER = 'pl-[3.75rem] pr-5 font-sans @5xl:col-span-full'

  const key = $derived(message.line.key)
  const quote = $derived(`inbox-message:${key}`)

  let unfolded = $state(false)
  let busy = $state(false)
  let failed = $state(false)
  let sent = $state(false)
  let draft = $state('')
  let sending = $state(false)
  let sendError = $state<string | null>(null)
  let input = $state<HTMLInputElement | null>(null)

  async function mark(action: 'check' | 'dismiss'): Promise<void> {
    if (busy) return
    busy = true
    const ok = await actions.markItem(key, action)
    busy = false
    failed = !ok
    if (ok) onGone()
  }

  async function toggleReply(): Promise<void> {
    draft = ''
    sendError = null
    onReply()
    await tick()
    input?.focus()
  }

  async function send(): Promise<void> {
    const text = draft.trim()
    if (!text || !message.comms || sending) return
    sending = true
    sendError = null
    try {
      await sendChannelMessage(message.comms.channelId, text)
      sent = true
      draft = ''
      onReply()
      actions.invalidate()
    } catch (e) {
      sendError = errorMessage(e)
    } finally {
      sending = false
    }
  }

  function overflow(): ContextMenuEntry[] {
    const href = message.href
    const items: ContextMenuEntry[] = [{ label: 'Remove from inbox', icon: [X, { size: 13 }], disabled: busy, onSelect: () => void mark('dismiss') }]
    if (href) items.push({ label: openLabel({ source: 'message', taskId: null }), icon: [ArrowUpRight, { size: 13 }], onSelect: () => void navigateHref(href) })
    return items
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onReply()
    }
  }
</script>

<div class={cn('relative font-sans hover:bg-hover', ROW_GRID)}>
  <div class={cn('flex items-center gap-3 px-5 pt-1 text-[13.5px] leading-5 @5xl:min-h-[52px] @5xl:py-2', ROW_GRID)}>
    <PersonMark name={message.from} size="lg" />
    <button
      type="button"
      aria-expanded={unfolded}
      aria-controls={quote}
      onclick={() => (unfolded = !unfolded)}
      class="flex min-w-0 flex-1 items-baseline gap-x-1 text-left outline-none after:absolute after:inset-0 focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-accent @5xl:col-span-3 @5xl:grid @5xl:grid-cols-subgrid @5xl:items-center @5xl:gap-x-3"
    >
      <span class="flex min-w-0 items-baseline gap-x-1 @5xl:pr-1">
        <span use:wordClamp={message.from} class="min-w-0 truncate font-medium text-fg"></span>
        <span aria-hidden="true" class="text-muted">·</span>
        <span use:wordClamp={message.source} class="min-w-0 shrink-[9] truncate text-muted"></span>
      </span>
      <span aria-hidden="true" class="text-muted @5xl:hidden">·</span>
      <span
        use:wordClamp={message.text}
        class={cn('hidden min-w-0 text-[13px] leading-[18px] text-fg', unfolded ? 'invisible truncate @5xl:block' : 'break-words @5xl:line-clamp-2')}
      ></span>
      <time datetime={message.at} class="shrink-0 text-[12.5px] tabular-nums text-muted @5xl:text-right">{ago(now - Date.parse(message.at))}</time>
    </button>
    <div class="relative flex shrink-0 items-center justify-end gap-1">
      <Button {...ACTION} variant="ghost" class={INK} aria-label="Mark handled" disabled={busy} onclick={() => void mark('check')}>
        <Check size={13} /><span class={WORD}>Mark handled</span>
      </Button>
      {#if message.comms}
        <Button
          {...ACTION}
          variant="ghost"
          class={cn(INK, 'border-line-strong', replying && 'bg-fg/[0.06]')}
          aria-label="Reply"
          aria-expanded={replying}
          onclick={() => void toggleReply()}
        >
          <CornerUpLeft size={13} /><span class={WORD}>Reply</span>
        </Button>
      {/if}
      <DropdownMenu items={overflow}>
        {#snippet trigger(open)}
          <IconButton size="sm" title="More actions" class="h-6 w-6" active={open} aria-haspopup="menu" aria-expanded={open}>
            <Ellipsis size={14} />
          </IconButton>
        {/snippet}
      </DropdownMenu>
    </div>
  </div>
  {#if unfolded}
    <p id={quote} class={cn(UNDER, 'relative break-words pb-2 text-[13px] leading-5 text-fg @5xl:-mt-1.5 @5xl:pb-2.5')}>{message.text}</p>
  {:else}
    <p id={quote} use:wordClamp={message.text} class={cn(UNDER, 'truncate pb-1.5 text-[13px] leading-5 text-fg @5xl:hidden')}></p>
  {/if}
</div>

{#if failed}
  <p transition:fade={QUICK} class={cn(UNDER, 'pb-2 text-[12px] text-[color:var(--theme-danger)]')}>That did not go through.</p>
{/if}
{#if replying && message.comms}
  <div transition:slide={{ duration: 150 }} class={cn(UNDER, 'pb-3')}>
    {#if message.comms.draft}
      <BriefReply
        comms={message.comms}
        peer={message.from}
        onDecide={actions.decideReply}
        onDelegate={(channelId, granted) => void actions.setDelegated(channelId, granted)}
      />
    {:else}
      <div class="flex items-center gap-2">
        <Input bind:ref={input} bind:value={draft} size="sm" class="font-sans" placeholder="Reply to {message.from}" disabled={sending} {onkeydown} />
        <Kbd>⏎ send</Kbd>
      </div>
    {/if}
    {#if sendError}
      <p transition:fade={QUICK} class="mt-2 font-sans text-[12px] text-[color:var(--theme-danger)]">{sendError}</p>
    {/if}
  </div>
{/if}
{#if sent}
  <p in:fade={QUICK} class={cn(UNDER, 'flex items-center gap-1.5 pb-2 text-[12.5px] text-[color:var(--theme-success)]')}>
    <Check size={12} /> Sent
  </p>
{/if}
