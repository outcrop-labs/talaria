<script lang="ts">
  import { X } from '@lucide/svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { useMySent, type SentMessage } from '@/lib/comms-api'
  import { clearDraft, draftTarget, listDrafts, type CommsDraft } from '@/lib/comms-drafts'
  import { commsSelectionFromPath, type CommsSelection } from '@/lib/comms-selection'
  import type { AgentModel } from '@/lib/agents'
  import type { Channel } from '@/lib/channels.svelte'
  import type { Conversation } from '@/lib/conversations.svelte'

  // /comms/drafts (R16): your unsent composer drafts (local, this browser —
  // comms-drafts.ts), then the messages you recently sent (GET /api/me/sent).
  // Each row opens its conversation; each draft can be discarded.
  let {
    userId,
    channels,
    conversations,
    fleet,
    onOpen,
  }: {
    userId: string | null
    channels: Channel[]
    conversations: Conversation[]
    fleet: AgentModel[]
    /** Null = the conversation is not loaded here; Comms falls back to /comms. */
    onOpen: (sel: CommsSelection | null) => void
  } = $props()

  const sentQuery = useMySent()
  const sent = $derived(sentQuery.data ?? [])

  // Read on mount (and if the person changes); discarding updates it in place.
  let drafts = $state<CommsDraft[]>([])
  $effect(() => {
    drafts = userId ? listDrafts(userId) : []
  })

  const lookup = $derived({ channelIds: channels.map((c) => c.id), conversations })

  const channelLabel = (c: Channel): string =>
    c.kind === 'channel' ? `#${c.name}` : c.kind === 'group' ? `⇄ ${c.name}` : (c.peer?.name ?? c.peer?.email ?? 'Direct message')
  const agentLabel = (model: string) => fleet.find((a) => a.id === model)?.label ?? model

  const draftLabel = (key: string): string => {
    const fresh = /^agent:(.+):new$/.exec(key)
    if (fresh) return `${agentLabel(fresh[1]!)} · new thread`
    const ch = channels.find((c) => c.id === key)
    if (ch) return channelLabel(ch)
    const conv = conversations.find((c) => c.id === key)
    if (conv) return `${agentLabel(conv.agentModel)} · ${conv.title || 'Untitled'}`
    return 'A conversation that is no longer here'
  }

  const draftSel = (key: string): CommsSelection | null => {
    const path = draftTarget(key, lookup)
    return path ? commsSelectionFromPath(path) : null
  }

  const sentSel = (m: SentMessage): CommsSelection | null => {
    if (m.kind === 'channel') return { t: 'channel', id: m.conversationId }
    const conv = conversations.find((c) => c.id === m.conversationId)
    return conv ? { t: 'agent', model: conv.agentModel, conversationId: conv.id } : null
  }

  const sentLabel = (m: SentMessage): string => {
    if (m.kind === 'channel') {
      const ch = channels.find((c) => c.id === m.conversationId)
      return ch ? channelLabel(ch) : (m.title ?? 'Channel')
    }
    const conv = conversations.find((c) => c.id === m.conversationId)
    return conv ? `${agentLabel(conv.agentModel)} · ${m.title || conv.title || 'Untitled'}` : (m.title ?? 'Agent thread')
  }

  const discard = (key: string) => {
    if (!userId) return
    clearDraft(userId, key)
    drafts = drafts.filter((d) => d.key !== key)
  }

  const sentLoading = $derived(sentQuery.isLoading)
  const sentFailed = $derived(sentQuery.isError && sentQuery.data === undefined)
  const nothing = $derived(drafts.length === 0 && !sentLoading && !sentFailed && sent.length === 0)
  const rowCls = 'flex w-full min-w-0 flex-col gap-1 rounded-md px-2 py-2.5 text-left transition-colors hover:bg-card2'
</script>

<div class="flex h-full min-h-0 flex-col">
  <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
    <span class="text-sm font-semibold text-fg">Drafts & sent</span>
  </header>
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if nothing}
      <EmptyState
        icon="◈"
        title="No drafts or sent messages"
        hint="Anything you start typing and leave unsent waits here, beside what you recently sent."
      />
    {:else}
      <div class="mx-auto w-full max-w-[var(--converse-width)] space-y-6 px-5 py-4">
        {#if drafts.length > 0}
          <section>
            <h2 class="mb-1 px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Drafts</h2>
            <ul class="divide-y divide-line-subtle">
              {#each drafts as d (d.key)}
                {@const target = draftSel(d.key)}
                <li class="group flex items-start gap-1">
                  <button
                    type="button"
                    disabled={!target}
                    onclick={() => target && onOpen(target)}
                    class={rowCls + ' disabled:cursor-default disabled:hover:bg-transparent'}
                  >
                    <span class="flex w-full items-center gap-2">
                      <span class="min-w-0 truncate font-sans text-sm font-semibold text-fg">{draftLabel(d.key)}</span>
                      {#if d.updatedAt}
                        <span class="ml-auto shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
                          {relativeTime(new Date(d.updatedAt).toISOString())}
                        </span>
                      {/if}
                    </span>
                    <span class="line-clamp-2 font-sans text-sm text-muted">{d.text}</span>
                  </button>
                  <IconButton size="sm" class="mt-2 shrink-0" title="Discard draft" onclick={() => discard(d.key)}>
                    <X size={14} />
                  </IconButton>
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        <section>
          <h2 class="mb-1 px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Sent</h2>
          {#if sentFailed}
            <QueryError
              variant="compact"
              error={sentQuery.error}
              title="Could not load what you sent"
              onRetry={() => void sentQuery.refetch()}
            />
          {:else if sentLoading}
            <div class="px-2 py-2"><SkeletonRows rows={5} /></div>
          {:else if sent.length === 0}
            <EmptyState variant="inline" class="px-2" title="Nothing sent yet" />
          {:else}
            <ul class="divide-y divide-line-subtle">
              {#each sent as m, i (`${m.kind}:${m.conversationId}:${m.createdAt}:${i}`)}
                {@const target = sentSel(m)}
                <li>
                  <button type="button" onclick={() => onOpen(target)} class={rowCls}>
                    <span class="flex w-full items-center gap-2">
                      <span class="min-w-0 truncate font-sans text-sm font-semibold text-fg">{sentLabel(m)}</span>
                      <span class="ml-auto shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
                        {relativeTime(m.createdAt)}
                      </span>
                    </span>
                    <span class="line-clamp-2 font-sans text-sm text-muted">{m.content}</span>
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>
    {/if}
  </div>
</div>
