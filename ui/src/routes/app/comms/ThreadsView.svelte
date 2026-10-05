<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { useMyThreads, type MyThread } from '@/lib/comms-api'
  import type { AgentModel } from '@/lib/agents'
  import type { Channel } from '@/lib/channels.svelte'
  import type { DirectoryUser } from '@/lib/users'

  // /comms/threads (R16): the threads you started or replied in, newest
  // activity first. Each opens its channel.
  let {
    channels,
    users,
    fleet,
    onOpen,
  }: {
    channels: Channel[]
    users: DirectoryUser[]
    fleet: AgentModel[]
    onOpen: (channelId: string) => void
  } = $props()

  const threadsQuery = useMyThreads()

  const where = (t: MyThread): string => {
    if (t.channelKind === 'channel') return `#${t.channelName}`
    if (t.channelKind === 'group') return `⇄ ${t.channelName}`
    const peer = channels.find((c) => c.id === t.channelId)?.peer
    return peer?.name ?? peer?.email ?? t.channelName
  }

  const author = (t: MyThread): { name: string; picture: string | null } => {
    const m = t.root
    if (m.authorType === 'agent') return { name: fleet.find((a) => a.id === m.author)?.label ?? m.author, picture: null }
    const u = users.find((x) => x.id === m.author)
    return { name: u?.name ?? u?.email ?? 'Someone', picture: u?.picture ?? null }
  }
</script>

<div class="flex h-full min-h-0 flex-col">
  <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
    <span class="text-sm font-semibold text-fg">Threads</span>
  </header>
  <div class="min-h-0 flex-1 overflow-y-auto">
    <QueryState query={threadsQuery} errorTitle="Could not load your threads">
      {#snippet skeleton()}
        <div class="mx-auto w-full max-w-[var(--converse-width)] p-5"><SkeletonRows rows={5} avatar /></div>
      {/snippet}
      {#snippet empty()}
        <EmptyState
          icon="◈"
          title="No threads yet"
          hint="Reply in thread on any channel message, and the conversation collects here."
        />
      {/snippet}
      {#snippet children(threads)}
        <ul class="mx-auto w-full max-w-[var(--converse-width)] divide-y divide-line-subtle px-5 py-2">
          {#each threads as t (t.root.id)}
            {@const who = author(t)}
            <li>
              <button
                type="button"
                onclick={() => onOpen(t.channelId)}
                class="flex w-full min-w-0 flex-col gap-1.5 rounded-md px-2 py-3 text-left transition-colors hover:bg-card2"
              >
                <span class="flex w-full items-center gap-2">
                  <span class="min-w-0 truncate font-sans text-sm font-semibold text-fg">{where(t)}</span>
                  <span class="ml-auto shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
                    {relativeTime(t.lastAt)}
                  </span>
                </span>
                <span class="flex w-full min-w-0 items-start gap-2">
                  <Avatar name={who.name} src={who.picture} class="mt-0.5 h-5 w-5 text-[9px]" />
                  <span class="min-w-0 flex-1">
                    <span class="font-sans text-xs font-semibold text-fg">{who.name}</span>
                    <span class="line-clamp-2 font-sans text-sm text-muted">{t.root.content}</span>
                  </span>
                </span>
                {#if t.root.thread?.count}
                  <span class="pl-7 font-mono text-[10px] tracking-[0.05em] text-accent">
                    {t.root.thread.count}
                    {t.root.thread.count === 1 ? 'reply' : 'replies'}
                  </span>
                {/if}
              </button>
            </li>
          {/each}
        </ul>
      {/snippet}
    </QueryState>
  </div>
</div>
