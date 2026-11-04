<script lang="ts">
  import { MessageSquareText, X } from '@lucide/svelte'
  import MessageRow from './MessageRow.svelte'
  import DayDivider from './DayDivider.svelte'
  import { dayDividerMap } from '@/lib/day-dividers'
  import { continuationSet, messageBlocks } from '@/lib/message-groups'
  import { bottomStick } from '@/lib/stick-to-bottom'
  import ChannelComposer from './ChannelComposer.svelte'
  import TypingLine from './TypingLine.svelte'
  import { typersIn, typingKey } from '@/lib/comms-typing.svelte'
  import { useUsers } from '@/lib/users'
  import { useSession } from '@/lib/session'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { slide, GROW_X } from '@/lib/motion'
  import { markChannelThreadRead, sendChannelMessage, useThreadMessages } from '@/lib/channels.svelte'
  import { lookingNow, watchLooking } from '@/lib/looking'
  import { NOTHING_POSTED, postedAfter, threadReadFor, trailingReads, type ThreadReadPosted } from '@/lib/thread-read'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { splitAttachments, type Attachment } from '@/lib/attachments'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import type { MessageCtx } from './channel-view'

  // The thread side panel: root + replies + its own composer. Replies are
  // channel messages that hang off the root; @mentioned agents answer HERE.
  let {
    channelId,
    channelName,
    rootId,
    ctx,
    mentionables,
    onClose,
  }: {
    channelId: string
    /** Shown beside "Thread" in the header. */
    channelName?: string
    rootId: string
    ctx: MessageCtx
    mentionables: Mentionable[]
    onClose: () => void
  } = $props()

  // Same trap one level down: `= []` on a rejected thread read rendered an
  // EMPTY thread panel — no root, no replies, no error — beside a rollup that
  // had just said "3 replies".
  const threadQuery = useThreadMessages(() => channelId, () => rootId)

  // Who is typing a reply in THIS thread (comms-typing), by first name.
  const usersQuery = useUsers()
  const session = useSession()
  const typingNames = $derived(
    typersIn(typingKey(channelId, rootId), session.data?.id).map((uid) => {
      const u = (usersQuery.data ?? []).find((r) => r.id === uid)
      return (u?.name ?? u?.email ?? 'Someone').split(' ')[0] ?? 'Someone'
    }),
  )
  const messages = $derived(threadQuery.data ?? [])
  let scrollEl = $state<HTMLDivElement | null>(null)
  // Same day dividers as the main feed (R1) — a thread can span days.
  const dividers = $derived(dayDividerMap(messages, (m) => m.createdAt, { timeZone: ctx.timeZone, now: Date.now() }))
  // Replies from one sender group like the main feed's (lib/message-groups).
  // The root stands alone: the replies rule sits under it, so the first reply
  // always carries its own header.
  const continued = $derived(
    continuationSet(
      messageBlocks(messages, (m, i) => ({ sender: `${m.authorType}:${m.author}`, at: m.createdAt, fresh: i === 1 }), {
        dayStarts: dividers,
      }),
    ),
  )

  const stick = bottomStick()
  $effect(() => stick.attach(scrollEl))
  $effect(() => {
    void messages
    stick.follow()
  })
  // ChannelView swaps rootId without remounting, so opening a second thread
  // would otherwise inherit the first one's hold.
  $effect(() => {
    void rootId
    stick.jump()
  })

  const send = async (text: string, atts: Attachment[]) => {
    stick.jump()
    const { attachmentIds, refs } = splitAttachments(atts)
    await sendChannelMessage(channelId, text, attachmentIds, refs, rootId)
  }

  // Reading the thread = having read its replies: advance the thread's OWN
  // read cursor as replies land, so the channel's unread badge drops (the
  // server excludes replies a thread cursor covers from the count — the main
  // feed's cursor never advances on reply seqs). Same looking rule as
  // ChannelView: only while the person is looking (lib/looking.ts), because
  // a thread read also clears the thread's bell rows — a panel nobody looks
  // at would mark a reply read before it ever alerted.
  let looking = $state(lookingNow())
  $effect(() => watchLooking((v) => (looking = v)))
  const qc = useQueryClient()
  // Both counts a read moves: the sidebar pill (['channels']) and the rail's
  // Comms badge (['unreads']). The rail otherwise waits for the next firehose
  // landing or its 30 s floor, since a read that clears no bell row publishes
  // nothing. Replies landing in a burst are one read (lib/thread-read.ts
  // trailingReads): each post costs those two refetches, the rail's heaviest
  // queries.
  const reads = trailingReads((read) => {
    void markChannelThreadRead(channelId, read.rootId, read.seq ?? undefined)
      .then(() =>
        Promise.all([
          qc.invalidateQueries({ queryKey: ['channels'] }),
          qc.invalidateQueries({ queryKey: ['unreads'] }),
        ]),
      )
      .catch(() => {})
  })
  // Closing the panel sends what it showed.
  $effect(() => () => reads.flush())
  // What was posted is keyed by the ROOT, as ChannelView keys its own by the
  // channel: ChannelView swaps rootId without remounting this panel, and seqs
  // are channel-wide, so a second thread whose newest reply is older than the
  // first's sat below a bare high-water mark and never posted its read. A
  // full page (the thread's oldest 300) posts no seq: threadReadFor says why.
  let posted: ThreadReadPosted = NOTHING_POSTED
  $effect(() => {
    // Oldest first, root first: the last row is the newest reply on the page
    // (or the root itself in a thread with no replies — covered by the
    // channel cursor, and a no-op here).
    const root = rootId
    const fetchedAt = threadQuery.dataUpdatedAt
    const read = threadReadFor({
      rootId: root,
      count: messages.length,
      latest: messages[messages.length - 1]?.seq ?? 0,
      fetchedAt,
      posted,
      looking,
    })
    if (!read) return
    const opened = posted.id !== root
    posted = postedAfter(read, fetchedAt)
    reads.push(read)
    // A thread just opened reads at once; the replies after it wait a beat.
    if (opened) reads.flush()
  })
</script>

<!-- Thread side panel — a proper panel surface (spec §8) beside the feed.
     IN-FLOW panel: slide={GROW_X} on both legs so the feed glides as the panel
     grows/shrinks instead of snapping when the width lands (ANIMATIONS.md).
     |global: ChannelView mounts this whole component per thread, so local legs
     on the component root never play (the |global rule). The inner wrapper is
     pinned to the resting width so message text clips instead of rewrapping. -->
<div transition:slide|global={GROW_X} class="shrink-0 border-l border-line bg-panel">
<div class="flex h-full w-[380px] flex-col">
  <div class="flex items-center gap-2 border-b border-line px-4 py-2.5">
    <MessageSquareText size={14} class="text-muted" />
    <span class="font-sans text-sm font-semibold text-fg">Thread</span>
    {#if channelName}
      <span class="min-w-0 truncate font-sans text-xs text-muted">{channelName}</span>
    {/if}
    <button
      type="button"
      onclick={onClose}
      class="ml-auto grid h-7 w-7 place-items-center rounded-md text-muted transition-colors dither-fill hover:text-fg"
      title="Close thread"
    >
      <X size={14} />
    </button>
  </div>
  <div bind:this={scrollEl} class="flex-1 space-y-4 overflow-y-auto px-4 py-4 [&>[data-continued]]:-mt-4">
    {#if threadQuery.isError && threadQuery.data !== undefined}
      <QueryError
        variant="inline"
        title="Thread may be out of date"
        error={threadQuery.error}
        onRetry={() => void threadQuery.refetch()}
      />
    {/if}
    {#if threadQuery.isError && threadQuery.data === undefined}
      <QueryError
        variant="compact"
        title="Could not load this thread"
        error={threadQuery.error}
        onRetry={() => void threadQuery.refetch()}
      />
    {:else if threadQuery.isLoading}
      <div aria-hidden="true" class="space-y-4">
        {#each Array.from({ length: 3 }) as _, i (i)}
          <div class="flex gap-2.5">
            <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" />
            <div class="min-w-0 flex-1 space-y-2 pt-1">
              <Skeleton class="h-2.5 w-20 rounded-full" />
              <Skeleton class="h-2.5 w-4/5 rounded-full" />
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <!-- Keyed on the root so a warm-cache thread switch (no skeleton pass)
          rebuilds the list instead of cross-fading two threads (see
          ChannelView's message list). -->
      {#key rootId}
        {#each messages as m, i (m.id)}
          {@const day = dividers.get(i)}
          <div data-continued={continued.has(i) || undefined}>
            {#if day}<DayDivider label={day.label} class="mb-3" />{/if}
            <MessageRow message={m} {ctx} inThread continued={continued.has(i)} onInvoke={(text) => void send(text, [])} />
            <!-- The replies rule under the root (R7): label, then a hairline. -->
            {#if i === 0 && messages.length > 1}
              <div class="mt-3 flex items-center gap-2">
                <span class="shrink-0 font-sans text-xs text-muted">
                  {messages.length - 1} {messages.length - 1 === 1 ? 'reply' : 'replies'}
                </span>
                <div class="h-px flex-1 bg-line"></div>
              </div>
            {/if}
          </div>
        {/each}
      {/key}
    {/if}
  </div>
  <div class="relative">
    <TypingLine names={typingNames} class="absolute -top-5 left-0 px-7" />
    <ChannelComposer
      channelName="thread"
      {channelId}
      threadRootId={rootId}
      placeholder="Reply in thread. @mention an agent to bring it in"
      {mentionables}
      onSend={send}
    />
  </div>
</div>
</div>
