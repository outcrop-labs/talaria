<script lang="ts">
  import MessageRow from './MessageRow.svelte'
  import ThreadPanel from './ThreadPanel.svelte'
  import ChannelComposer from './ChannelComposer.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { bottomStick } from '@/lib/stick-to-bottom'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { slide } from '@/lib/motion'
  import {
    markChannelRead,
    sendChannelMessage,
    useChannelDetail,
    useChannelEvents,
    useChannelMessages,
  } from '@/lib/channels.svelte'
  import { useUsers } from '@/lib/users'
  import { useSession } from '@/lib/session'
  import { userMentionInsert, type Mentionable } from '@/components/chat/mentions.svelte'
  import { splitAttachments, type Attachment } from '@/lib/attachments'
  import type { AgentModel } from '@/lib/agents'
  import { rowMenuEntries, type MessageCtx } from './channel-view'

  // One channel: live message feed + composer, Slack-shaped. Messages take
  // reactions (agents react too), spawn threads (agents @mentioned in a thread
  // reply in the thread), and carry pasted/dropped files. Agents reply when
  // @mentioned; their streamed replies arrive over the channel's SSE feed.
  // Embeddable as-is: a ticket's discussion room is this component on a
  // task-linked channel — the detail fetch serves the board's roster and
  // agents there, so the mention picker just works.
  let {
    channelId,
    channelName,
    fleet,
    onLiveMessage,
    zeroTitle,
    zeroHint,
    composerPlaceholder,
  }: {
    channelId: string
    channelName: string
    fleet: AgentModel[]
    /** A tick for embedders riding this component's one SSE subscription. */
    onLiveMessage?: () => void
    /** The composer's placeholder — same reason as the zero state: a room's
     *  composer should not claim to message "#<ticket title>". */
    composerPlaceholder?: string
    /** Override the zero state's words — an embedded room is not "a channel",
     *  and what an empty discussion should say depends on the ticket. */
    zeroTitle?: string
    zeroHint?: string
  } = $props()

  // The whole query, not `{ data: messages = [] }`. That default turned a
  // rejected fetch into "this channel has no messages" — `isLoading` is false
  // during an error, so the skeleton below (whose own comment says "never the
  // 'Welcome' hero") never ran, and a 500 rendered the hero over four real
  // messages.
  const messagesQuery = useChannelMessages(() => channelId)
  const messages = $derived(messagesQuery.data ?? [])
  // Fetched here (not passed down) so the message pane never waits on the
  // parent header's detail fetch — svelte-query dedupes the shared key.
  const detailQuery = useChannelDetail(() => channelId)
  const detail = $derived(detailQuery.data)
  // Same shape, quieter blast radius: a failed detail read empties the agent
  // and member lists, which the hero below reads back as "add people & agents"
  // and the composer as "nobody here to @mention".
  const detailFailed = $derived(detailQuery.isError && detail === undefined)
  /**
   * RESOLVED-empty, which is not the same as "no messages on screen": a failed
   * first read and a read still in flight both have zero messages and neither
   * is an empty channel. This is the one branch that goes full bleed, so it has
   * to mean exactly the thing EmptyState is allowed to say.
   */
  const zero = $derived(
    !(messagesQuery.isError && messagesQuery.data === undefined) &&
      !messagesQuery.isLoading &&
      messages.length === 0,
  )
  const channelAgents = $derived(detail?.agents ?? [])
  const members = $derived(detail?.members ?? [])
  // The directory only maps an author's email to their display name, and the
  // fallback below derives the label from the message's OWN author field, so a
  // failed lookup shows "jon" instead of "Jon Iler" — degraded, not a claim
  // about anything. It was still written `useUsers().data ?? []`, which throws
  // the query away on the line that made it: the degradation was reasoned
  // about, but nothing downstream could ever SAY it was degraded.
  const usersList = listQuery(useUsers(), { title: 'Names may be showing as email addresses', variant: 'inline' })
  /** Degraded reads worth saying out loud — they show above the zero state too. */
  const notices = $derived(
    (messagesQuery.isError && messagesQuery.data !== undefined) ||
      detailFailed ||
      (usersList.failed && !!usersList.notice),
  )
  const sessionQuery = useSession()
  useChannelEvents(() => channelId, () => onLiveMessage?.())
  let error = $state<string | null>(null)
  let threadRoot = $state<string | null>(null)
  const menu = useContextMenu()
  let scrollEl = $state<HTMLDivElement | null>(null)
  /** Measured height of the floating composer — what the transcript reserves. */
  let composerH = $state(0)
  const qc = useQueryClient()

  // Having the channel open = having read it: advance the read cursor as
  // messages land, so the sidebar badge clears live.
  let lastReadPosted: { id: string; seq: number } = { id: '', seq: 0 }
  $effect(() => {
    const latest = messages[messages.length - 1]?.seq ?? 0
    if (!latest) return
    const prev = lastReadPosted
    if (prev.id === channelId && latest <= prev.seq) return
    lastReadPosted = { id: channelId, seq: latest }
    void markChannelRead(channelId, latest).then(() => qc.invalidateQueries({ queryKey: ['channels'] })).catch(() => {})
  })

  // A thread panel only survives while its root does.
  $effect(() => {
    if (threadRoot && messages.length > 0 && !messages.some((m) => m.id === threadRoot)) threadRoot = null
  })

  // Instant, hold-aware follow (lib/stick-to-bottom): smooth scrolling
  // rubber-bands into a bounce under streamed flushes, and reading history
  // must never be yanked back down.
  const stick = bottomStick()
  $effect(() => stick.attach(scrollEl))
  $effect(() => {
    void messages
    stick.follow()
  })
  // Comms keys this component per channel so a switch remounts — but Channels
  // does not, and a stale hold would silently freeze the new channel.
  $effect(() => {
    void channelId
    stick.jump()
  })

  const labelFor = (model: string) => fleet.find((a) => a.id === model)?.label ?? model
  // Human authors are stored by email (stable identity); show their display name.
  const userLabel = (author: string) =>
    usersList.rows.find((u) => u.email === author)?.name ?? (author.split('@')[0] || author)

  const ctx: MessageCtx = $derived({
    channelId,
    me: sessionQuery.data?.email ?? sessionQuery.data?.name ?? '',
    isChannelOwner: detail?.role === 'owner',
    labelFor,
    userLabel,
  })

  const mentionables: Mentionable[] = $derived(
    [
      ...channelAgents.map((id) => ({ insert: labelFor(id), label: labelFor(id), sub: id })),
      // Tier mentions: "@Dex:opus" routes the reply to that model tier.
      ...channelAgents.flatMap((id) =>
        (fleet.find((a) => a.id === id)?.tiers ?? []).map((t) => ({
          insert: `${labelFor(id)}:${t}`,
          label: `${labelFor(id)}:${t}`,
          sub: 'tier',
        })),
      ),
      ...members.map((m) => ({
        insert: userMentionInsert(m),
        label: m.name ?? m.email ?? m.userId,
        sub: m.email ?? undefined,
      })),
    ].filter((m) => m.insert),
  )

  const send = async (text: string, atts: Attachment[]) => {
    error = null
    stick.jump()
    try {
      const { attachmentIds, refs } = splitAttachments(atts)
      await sendChannelMessage(channelId, text, attachmentIds, refs)
    } catch (e) {
      error = (e as Error).message
    }
  }
</script>

<div class="flex h-full min-h-0">
  <!-- The transcript owns the surface; the composer floats over it. One chat
       width everywhere still holds — the token just moved off this column and
       onto the message list, so the channel fills its stage while its text
       keeps the same measure agent DMs use. -->
  <div class="relative flex h-full min-h-0 w-full flex-1 flex-col" style:--chat-composer="{composerH}px">
    <div
      bind:this={scrollEl}
      class="flex-1 overflow-y-auto"
    >
      {#if notices}
        <div class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 px-6 pt-5">
      <!-- Transcripts refresh off the channel's SSE ticks. A failed refresh
          keeps the messages already on screen — yanking a conversation
          away over a blip is worse — but says so rather than presenting
          them as the live feed. -->
      {#if messagesQuery.isError && messagesQuery.data !== undefined}
        <QueryError
          variant="inline"
          title="Messages may be out of date"
          error={messagesQuery.error}
          onRetry={() => void messagesQuery.refetch()}
        />
      {/if}
      {#if detailFailed}
        <QueryError
          variant="inline"
          title="Could not load this channel's people & agents"
          error={detailQuery.error}
          onRetry={() => void detailQuery.refetch()}
        />
      {/if}
      <!-- Deliberately quiet: without the directory each message still names
          its own author, so this degrades a display name rather than
          inventing an absence. It says so anyway. -->
      {#if usersList.failed && usersList.notice}
        <QueryError {...usersList.notice} />
      {/if}
        </div>
      {/if}

      <!-- EDGE TO EDGE, and this is why it is its own branch rather than a
           class on the shared wrapper: negative margins escape the gutter but
           not the max-width, so a zero state inside the measure paints its
           vignette as a 900px card in the middle of the channel. It has to be
           OUTSIDE the column, not merely wider than its padding. -->
      {#if zero}
        <EmptyState
          class="h-full"
          icon="#"
          title={zeroTitle ?? `Welcome to #${channelName}`}
          hint={zeroHint ??
            (channelAgents.length
              ? `Say something. @mention ${channelAgents.map(labelFor).join(', ')} to bring the agents in.`
              : detailFailed
                ? 'Say something.'
                : 'Say something, or add people & agents.')}
        />
      {:else}
      <!-- The clearance is on the MESSAGE LIST, not on this scroll box. On the
           box it applied to every branch — so the transcript stopped short of
           the composer instead of running under it, and the zero state was
           held off the bottom of its own void. Only content that can be read
           needs to clear the float; the surface behind it should not. -->
      <div
        class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 px-6 py-5"
        style:padding-bottom="calc(var(--chat-composer, 0px) + 0.5rem)"
      >
      {#if messagesQuery.isError && messagesQuery.data === undefined}
        <QueryError
          title="Could not load messages"
          error={messagesQuery.error}
          onRetry={() => void messagesQuery.refetch()}
        />
      {:else if messagesQuery.isLoading}
        <!-- Transcript-shaped shimmer while the history loads — never the
            "Welcome" hero, which reads as an empty channel it isn't. -->
        <div aria-hidden="true" class="space-y-5">
          {#each Array.from({ length: 5 }) as _, i (i)}
            <div class="flex gap-2.5">
              <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" />
              <div class="min-w-0 flex-1 space-y-2 pt-1">
                <Skeleton class="h-2.5 w-24 rounded-full" />
                <div style:width={['82%', '64%', '90%', '71%', '58%'][i]}>
                  <Skeleton class="h-2.5 w-full rounded-full" />
                </div>
                <div style:width={['55%', '78%', '40%', '62%', '84%'][i]}>
                  <Skeleton class="h-2.5 w-full rounded-full" />
                </div>
              </div>
            </div>
          {/each}
        </div>
      {:else}
        <!-- Keyed on the channel so a warm-cache channel switch (no skeleton
            pass) rebuilds the list wholesale instead of cross-fading two
            transcripts — the rows' local fades then mark only messages that
            genuinely arrive/leave while you watch. -->
        {#key channelId}
          {#each messages as m (m.id)}
            <MessageRow
              message={m}
              {ctx}
              onOpenThread={() => (threadRoot = m.threadRootId ?? m.id)}
              onContextMenu={(e) => menu.openMenu(e, rowMenuEntries(m, ctx, () => (threadRoot = m.id)))}
            />
          {/each}
        {/key}
      {/if}
        {#if error}
          <div transition:slide={{ duration: 150 }} class="text-center text-sm" style:color="var(--theme-danger)">
            {error}
          </div>
        {/if}
      </div>
      {/if}
    </div>

    <!-- pointer-events-none gutter, auto panel (ChannelComposer's own root) —
         otherwise the transparent margin either side eats clicks on the
         transcript behind it. -->
    <!-- Opaque: a transparent gutter let the transcript show through beneath
         the composer panel. -->
    <div bind:clientHeight={composerH} class="pointer-events-none absolute inset-x-0 bottom-0 z-10 bg-surface">
      <div class="mx-auto w-full max-w-[var(--converse-width)]">
        <ChannelComposer {channelName} placeholder={composerPlaceholder} {mentionables} onSend={send} />
      </div>
    </div>
    <ContextMenu {menu} />
  </div>

  {#if threadRoot}
    <ThreadPanel
      {channelId}
      rootId={threadRoot}
      {ctx}
      {mentionables}
      onClose={() => (threadRoot = null)}
    />
  {/if}
</div>
