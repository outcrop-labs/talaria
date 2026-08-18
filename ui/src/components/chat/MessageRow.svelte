<script lang="ts">
  import { MessageSquareText, Pencil, SmilePlus, Trash2 } from '@lucide/svelte'
  import MessageAvatar from './MessageAvatar.svelte'
  import HoverAction from './HoverAction.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import MessageAttachments from '@/components/chat/MessageAttachments.svelte'
  import GuardCaveat from '@/components/chat/GuardCaveat.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { resolveAgentMedia } from '@/lib/agent-media'
  import { deleteChannelMessage, editChannelMessage, toggleMessageReaction, type ChannelMessage } from '@/lib/channels.svelte'
  import { actorLabel, REACTION_SET, type MessageCtx } from './channel-view'

  let {
    message: m,
    ctx,
    inThread = false,
    onOpenThread,
    onContextMenu,
  }: {
    message: ChannelMessage
    ctx: MessageCtx
    /** Rendered inside the thread panel: no thread affordances of its own. */
    inThread?: boolean
    onOpenThread?: () => void
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  const name = $derived(m.authorType === 'agent' ? ctx.labelFor(m.author) : ctx.userLabel(m.author))
  const time = $derived(new Date(m.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }))
  const live = $derived(m.status === 'streaming')
  const own = $derived(m.authorType === 'user' && m.author === ctx.me)
  let picking = $state(false)
  let editing = $state(false)
  let draft = $state('')
  let toolbarRef = $state<HTMLDivElement | null>(null)

  // The palette must never trap you: outside click, Esc, or simply moving
  // off the message all dismiss it.
  $effect(() => {
    if (!picking) return
    const onDoc = (e: MouseEvent) => {
      if (!toolbarRef?.contains(e.target as Node)) picking = false
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') picking = false
    }
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  })

  const react = (emoji: string) => {
    picking = false
    void toggleMessageReaction(ctx.channelId, m.id, emoji).catch(() => {})
  }
  const saveEdit = () => {
    const text = draft.trim()
    editing = false
    if (text && text !== m.content) void editChannelMessage(ctx.channelId, m.id, text).catch(() => {})
  }
</script>

<!-- Flattened message row (spec §10): avatar square + name + 10px mono
    timestamp, 14px sans body — no bubble. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  in:fade={{ duration: 150 }}
  out:fade={QUICK}
  class="group relative flex gap-2.5"
  oncontextmenu={onContextMenu}
  onmouseleave={() => (picking = false)}
>
  <MessageAvatar {name} class="mt-0.5" />
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{name}</span>
      {#if m.authorType === 'agent'}
        <span class="rounded border border-line px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">
          agent
        </span>
      {/if}
      <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{time}</span>
      {#if m.editedAt}<span class="font-mono text-[10px] text-ink-dim">(edited)</span>{/if}
    </div>
    <div class="font-sans text-sm">
      {#if editing}
        <div class="mt-1">
          <Textarea
            autofocus
            autoGrow
            rows={1}
            bind:value={draft}
            onkeydown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                saveEdit()
              } else if (e.key === 'Escape') {
                editing = false
              }
            }}
            class="max-h-40"
          />
          <div class="mt-1 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">enter to save · esc to cancel</div>
        </div>
      {:else if m.content}
        <Markdown children={m.authorType === 'agent' ? resolveAgentMedia(m.content, m.author) : m.content} />
      {:else if live}
        <!-- Awaiting the agent's first token — the submitting rung (spec §9). -->
        <WaitingMark site="chat/message-first-token" class="py-1 text-accent" />
      {/if}
      {#if m.attachments && m.attachments.length > 0}<MessageAttachments items={m.attachments} />{/if}
      <!-- Mounted unconditionally (findings nulled while live) so the caveat's
          slide fires on the live→settled flip; behind an `{#if !live}` the
          local transition would be suppressed by the ancestor block toggling. -->
      <GuardCaveat findings={live ? null : m.guard} />
      {#if m.content && live}<span class="gd-pulse ml-0.5 inline-block h-4 w-1.5 bg-accent align-middle"></span>{/if}
      {#if m.status === 'error'}
        <div class="text-xs" style:color="var(--theme-danger)">
          · interrupted
        </div>
      {/if}
    </div>

    <!-- Reaction chips: click toggles yours; hover names the reactors. -->
    {#if (m.reactions?.length ?? 0) > 0}
      <div in:fade={{ duration: 150 }} out:fade={QUICK} class="mt-1.5 flex flex-wrap items-center gap-1">
        {#each m.reactions ?? [] as r (r.emoji)}
          {@const mine = r.actors.some((a, i) => r.actorTypes[i] === 'user' && a === ctx.me)}
          <button
            in:fade={{ duration: 150 }}
            out:fade={QUICK}
            type="button"
            title={r.actors.map((a, i) => actorLabel(ctx, a, r.actorTypes[i] ?? 'user')).join(', ')}
            onclick={() => react(r.emoji)}
            class={cn(
              'flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs transition-colors',
              mine
                ? 'border-accent bg-accent-soft text-fg'
                : 'border-line bg-raised text-muted hover:dither-fill hover:text-fg',
            )}
          >
            <span>{r.emoji}</span>
            <span class="font-mono text-[10px] tracking-[0.05em]">{r.actors.length}</span>
          </button>
        {/each}
      </div>
    {/if}

    <!-- Thread rollup on roots (main flow only). -->
    {#if !inThread && m.thread}
      <button
        type="button"
        onclick={onOpenThread}
        class="mt-1.5 flex items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-1 text-xs text-accent transition-colors hover:border-accent"
      >
        <MessageSquareText size={12} />
        {m.thread.count} {m.thread.count === 1 ? 'reply' : 'replies'}
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">· {relativeTime(m.thread.lastAt)}</span>
      </button>
    {/if}
  </div>

  <!-- Hover toolbar: react · thread · edit (own) — delete lives in the
      context menu behind a confirm. -->
  {#if !live && !editing}
    <div
      bind:this={toolbarRef}
      class={cn(
        'absolute -top-2 right-0 items-center gap-0.5 rounded-md border border-line bg-raised p-0.5 shadow-[var(--theme-shadow-1)]',
        picking ? 'flex' : 'hidden group-hover:flex',
      )}
    >
      {#if picking}
        {#each REACTION_SET as e (e)}
          <button
            type="button"
            onclick={() => react(e)}
            class="grid h-7 w-7 place-items-center rounded-md text-base transition-colors hover:dither-fill"
          >
            {e}
          </button>
        {/each}
      {:else}
        <HoverAction title="Add reaction" onClick={() => (picking = true)}>
          <SmilePlus size={14} />
        </HoverAction>
        {#if !inThread && !m.threadRootId}
          <HoverAction title="Reply in thread" onClick={() => onOpenThread?.()}>
            <MessageSquareText size={14} />
          </HoverAction>
        {/if}
        {#if own}
          <HoverAction
            title="Edit message"
            onClick={() => {
              draft = m.content
              editing = true
            }}
          >
            <Pencil size={14} />
          </HoverAction>
        {/if}
        {#if own || ctx.isChannelOwner}
          <HoverAction
            title="Delete message"
            danger
            onClick={() => {
              void confirm({
                title: 'Delete message',
                message: m.thread?.count
                  ? `Delete this message and its ${m.thread.count} thread ${m.thread.count === 1 ? 'reply' : 'replies'}?`
                  : 'Delete this message?',
                confirmLabel: 'Delete',
              }).then((ok) => {
                if (ok) void deleteChannelMessage(ctx.channelId, m.id)
              })
            }}
          >
            <Trash2 size={14} />
          </HoverAction>
        {/if}
      {/if}
    </div>
  {/if}
</div>
