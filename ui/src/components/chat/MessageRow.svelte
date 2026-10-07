<script lang="ts">
  import MessageAvatar from './MessageAvatar.svelte'
  import MessageActions from './MessageActions.svelte'
  import ReactionChips, { type ReactionChip } from './ReactionChips.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import ChatWaiting from './ChatWaiting.svelte'
  import MessageAttachments from '@/components/chat/MessageAttachments.svelte'
  import GuardCaveat from '@/components/chat/GuardCaveat.svelte'
  import ChatChips from './ChatChips.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { fade, QUICK } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { resolveAgentMedia } from '@/lib/agent-media'
  import { deleteChannelMessage, editChannelMessage, toggleMessageReaction, type ChannelMessage } from '@/lib/channels.svelte'
  import { actorLabel, threadAvatars, type MessageCtx } from './channel-view'
  import { turnTime } from './chat-view'

  let {
    message: m,
    ctx,
    inThread = false,
    onOpenThread,
    onContextMenu,
    onInvoke,
    onDecided,
  }: {
    message: ChannelMessage
    ctx: MessageCtx
    /** Rendered inside the thread panel: no thread affordances of its own. */
    inThread?: boolean
    onOpenThread?: () => void
    onContextMenu?: (e: MouseEvent) => void
    onInvoke?: (text: string) => void
    onDecided?: () => void
  } = $props()

  const name = $derived(m.authorType === 'agent' ? ctx.labelFor(m.author) : ctx.userLabel(m.author))
  // The viewer's zone, the same one the day dividers use.
  const time = $derived(turnTime(m.createdAt, ctx.timeZone))
  const live = $derived(m.status === 'streaming')
  const own = $derived(m.authorType === 'user' && m.author === ctx.me)
  const picture = $derived(ctx.pictureFor(m.author, m.authorType))
  const chips: ReactionChip[] = $derived(
    (m.reactions ?? []).map((r) => ({
      emoji: r.emoji,
      count: r.actors.length,
      mine: r.actors.some((a, i) => r.actorTypes[i] === 'user' && a === ctx.me),
      title: r.actors.map((a, i) => actorLabel(ctx, a, r.actorTypes[i] ?? 'user')).join(', '),
    })),
  )
  const rollup = $derived(m.thread ? threadAvatars(ctx, m.thread.authors) : [])
  let editing = $state(false)
  let draft = $state('')
  const openProfile = () => ctx.onOpenProfile?.(m.author, m.authorType === 'agent' ? 'agent' : 'user')

  const react = (emoji: string) => {
    void toggleMessageReaction(ctx.channelId, m.id, emoji).catch(() => {})
  }
  const saveEdit = () => {
    const text = draft.trim()
    editing = false
    if (text && text !== m.content) void editChannelMessage(ctx.channelId, m.id, text).catch(() => {})
  }
  const startEdit = () => {
    draft = m.content
    editing = true
  }
  const askDelete = () => {
    void confirm({
      title: 'Delete message',
      message: m.thread?.count
        ? `Delete this message and its ${m.thread.count} thread ${m.thread.count === 1 ? 'reply' : 'replies'}?`
        : 'Delete this message?',
      confirmLabel: 'Delete',
    }).then((ok) => {
      if (ok) void deleteChannelMessage(ctx.channelId, m.id)
    })
  }
</script>

<!-- Flattened message row (spec §10): avatar square + name + 10px mono
    timestamp, 14px sans body — no bubble. The row highlights on hover (the
    rail row's hover token) and on keyboard focus within, which is also what
    reveals the action bar (MessageActions' group/message contract). -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  in:fade={{ duration: 150 }}
  out:fade={QUICK}
  class="group/message relative -mx-2 flex gap-2.5 rounded-md px-2 py-1 transition-colors hover:bg-card2 focus-within:bg-card2"
  oncontextmenu={onContextMenu}
>
  {#if ctx.onOpenProfile}
    <!-- The avatar and the name open the author's profile (Comms' drawer). -->
    <button type="button" class="mt-0.5 shrink-0 self-start rounded" title={`View ${name}'s profile`} onclick={openProfile}>
      <MessageAvatar {name} src={picture} />
    </button>
  {:else}
    <MessageAvatar {name} src={picture} class="mt-0.5" />
  {/if}
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      {#if ctx.onOpenProfile}
        <button type="button" class="font-sans text-[13px] font-medium text-fg hover:underline" onclick={openProfile}>
          {name}
        </button>
      {:else}
        <span class="font-sans text-[13px] font-medium text-fg">{name}</span>
      {/if}
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
        <Markdown
          children={m.authorType === 'agent' ? resolveAgentMedia(m.content, m.author) : m.content}
          selfMentions={ctx.selfMentions}
        />
      {:else if live}
        <!-- Awaiting the agent's first token — the submitting rung (spec §9). -->
        <ChatWaiting id={m.id} role="submitting" class="my-1" />
      {/if}
      {#if m.attachments && m.attachments.length > 0}<MessageAttachments items={m.attachments} />{/if}
      <ChatChips chips={m.chips} content={m.content} {onInvoke} {onDecided} />
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
    <ReactionChips reactions={chips} onToggle={react} />

    <!-- Thread rollup on roots (main flow only, R8): up to three stacked
        participant avatars, "N replies" in the accent, last reply muted. -->
    {#if !inThread && m.thread}
      <button
        type="button"
        onclick={onOpenThread}
        class="-ml-1 mt-1 flex items-center gap-2 rounded-md border border-transparent px-1 py-0.5 text-xs transition-colors hover:border-line hover:bg-raised"
      >
        {#if rollup.length > 0}
          <span class="flex -space-x-1">
            {#each rollup as a (a.key)}
              <MessageAvatar name={a.name} src={a.src} class="h-5 w-5 text-[8px] ring-1 ring-[var(--theme-bg)]" />
            {/each}
          </span>
        {/if}
        <span class="font-medium text-accent">{m.thread.count} {m.thread.count === 1 ? 'reply' : 'replies'}</span>
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">Last reply {relativeTime(m.thread.lastAt)}</span>
      </button>
    {/if}
  </div>

  <!-- Hover toolbar (R4): ✅ 👀 🙌 · picker · thread · edit (own) · delete
      (own or channel owner, behind a confirm). -->
  {#if !live && !editing}
    <MessageActions
      onReact={react}
      onReply={!inThread && !m.threadRootId ? () => onOpenThread?.() : undefined}
      onEdit={own ? startEdit : undefined}
      onDelete={own || ctx.isChannelOwner ? askDelete : undefined}
    />
  {/if}
</div>
