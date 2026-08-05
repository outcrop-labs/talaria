<script lang="ts">
  import MessageRow from './MessageRow.svelte'
  import ThreadPanel from './ThreadPanel.svelte'
  import ChannelComposer from './ChannelComposer.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useQueryClient } from '@tanstack/svelte-query'
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
  let {
    channelId,
    channelName,
    fleet,
  }: {
    channelId: string
    channelName: string
    fleet: AgentModel[]
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
  const channelAgents = $derived(detail?.agents ?? [])
  const members = $derived(detail?.members ?? [])
  // The directory only maps an author's email to their display name, and the
  // fallback below derives the label from the message's OWN author field, so a
  // failed lookup shows "jon" instead of "Jon Iler" — degraded, not a claim
  // about anything. It was still written `useUsers().data ?? []`, which throws
  // the query away on the line that made it: the degradation was reasoned
  // about, but nothing downstream could ever SAY it was degraded.
  const usersList = listQuery(useUsers(), { title: 'Names may be showing as email addresses', variant: 'inline' })
  const sessionQuery = useSession()
  useChannelEvents(() => channelId)
  let error = $state<string | null>(null)
  let threadRoot = $state<string | null>(null)
  const menu = useContextMenu()
  let scrollEl = $state<HTMLDivElement | null>(null)
  let prevCount = 0
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

  // Instant, pinned-only follow (see ChatView): streamed flushes + smooth
  // scrolling rubber-band into a bounce, and reading history must never be
  // yanked back down. A fresh channel load always lands at the end.
  $effect(() => {
    void messages
    const el = scrollEl
    if (!el) return
    const loaded = prevCount === 0 && messages.length > 0
    prevCount = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
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
    try {
      const { attachmentIds, refs } = splitAttachments(atts)
      await sendChannelMessage(channelId, text, attachmentIds, refs)
    } catch (e) {
      error = (e as Error).message
    }
  }
</script>

<div class="flex h-full min-h-0">
  <!-- One chat width everywhere: the same content token agent DMs use. -->
  <div class="mx-auto flex h-full min-h-0 w-full max-w-[var(--chat-content-max-width)] flex-1 flex-col">
    <div bind:this={scrollEl} class="flex-1 space-y-4 overflow-y-auto px-6 py-5">
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
              <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" delay={i * 0.12} />
              <div class="min-w-0 flex-1 space-y-2 pt-1">
                <Skeleton class="h-2.5 w-24 rounded-full" delay={i * 0.12} />
                <div style:width={['82%', '64%', '90%', '71%', '58%'][i]}>
                  <Skeleton class="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.06} />
                </div>
                <div style:width={['55%', '78%', '40%', '62%', '84%'][i]}>
                  <Skeleton class="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.12} />
                </div>
              </div>
            </div>
          {/each}
        </div>
      {:else if messages.length === 0}
        <EmptyState
          icon="#"
          title={`Welcome to #${channelName}`}
          hint={channelAgents.length
            ? `Say something. @mention ${channelAgents.map(labelFor).join(', ')} to bring the agents in.`
            : detailFailed
              ? // The channel really is empty, but who is IN it came from
                // a read that failed — "add people & agents" would be a
                // claim about a roster nobody managed to fetch.
                'Say something.'
              : 'Say something, or add people & agents.'}
        />
      {:else}
        {#each messages as m (m.id)}
          <MessageRow
            message={m}
            {ctx}
            onOpenThread={() => (threadRoot = m.threadRootId ?? m.id)}
            onContextMenu={(e) => menu.openMenu(e, rowMenuEntries(m, ctx, () => (threadRoot = m.id)))}
          />
        {/each}
      {/if}
      {#if error}
        <div class="text-center text-sm" style:color="var(--theme-danger)">
          {error}
        </div>
      {/if}
    </div>

    <ChannelComposer {channelName} {mentionables} onSend={send} />
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
