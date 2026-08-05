<script lang="ts">
  import { createRawSnippet, type Snippet } from 'svelte'
  import { navigate } from '@/router'
  import Tabs from '@/components/ui/Tabs.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useSession } from '@/lib/session'
  import { useChannels } from '@/lib/channels.svelte'
  import { greeting, useHome, type HomeTab } from './home'
  import BoardsTab from './BoardsTab.svelte'
  import CommsTab from './CommsTab.svelte'
  import PlansTab from './PlansTab.svelte'
  import ResearchTab from './ResearchTab.svelte'
  import DocsTab from './DocsTab.svelte'
  import FleetTab from './FleetTab.svelte'

  let { tab }: { tab: Exclude<HomeTab, 'inbox'> } = $props()

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const home = useHome()
  // Feeds the Comms badge count. Defaulted, a failed read renders "no unread" —
  // the one thing a badge exists to deny. Suppressing the badge is only half the
  // fix: a missing badge is indistinguishable from a genuine zero, and the lie
  // is told from EVERY tab, not just Comms, so CommsTab's own notice cannot
  // cover for it. The marker goes under the tab strip, where it is visible
  // wherever you are standing.
  const channelsList = listQuery(useChannels(), { title: 'Unread counts unavailable', variant: 'inline' })
  const setTab = (t: HomeTab) => void navigate('/', { search: t === 'inbox' ? {} : { tab: t } })

  // Console posture: one tab per work area, badge = where attention is needed.
  const unreadComms = $derived(
    channelsList.failed ? 0 : channelsList.rows.reduce((n, c) => n + (c.unreadCount ?? 0), 0),
  )
  const tabs = $derived.by((): { id: HomeTab; label: string; badge?: number }[] => [
    { id: 'inbox', label: 'Inbox', badge: home.data?.unread || undefined },
    { id: 'boards', label: 'Boards', badge: home.data?.queues.triage.count || undefined },
    { id: 'comms', label: 'Comms', badge: unreadComms || undefined },
    { id: 'plans', label: 'Plans' },
    { id: 'research', label: 'Research' },
    { id: 'docs', label: 'Docs' },
    ...(isAdmin ? [{ id: 'fleet' as const, label: 'Fleet' }] : []),
  ])

  // Tab labels carry an optional badge — bridged into the zero-arg snippet
  // Tabs.svelte expects (the React version passed a <span> inline).
  const tabLabel = (label: string, badge?: number): Snippet =>
    createRawSnippet(() => ({
      render: () =>
        `<span class="flex items-center gap-1.5">${label}${
          badge !== undefined ? `<span class="font-medium text-accent">${badge}</span>` : ''
        }</span>`,
    }))
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto max-w-6xl space-y-6">
    <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">{greeting(session.data?.name ?? session.data?.email)}</h1>

    <Tabs
      value={tab}
      onChange={setTab}
      items={tabs.map((t) => ({ id: t.id, label: tabLabel(t.label, t.badge) }))}
    />
    {#if channelsList.notice}
      <QueryError {...channelsList.notice} />
    {/if}

    <!-- The inbox tab is the focus queue now (Home.svelte renders
         <FocusInbox> before this console ever mounts), which is why the
         old inbox stack — briefing, notifications, approvals — is gone
         from here. `home` goes into BoardsTab whole, not pre-unwrapped:
         the tab needs `isError`/`refetch` to tell an empty queue from a
         queue it could not read. -->
    {#if tab === 'boards'}<BoardsTab {home} />{/if}
    {#if tab === 'comms'}<CommsTab />{/if}
    {#if tab === 'plans'}<PlansTab />{/if}
    {#if tab === 'research'}<ResearchTab />{/if}
    {#if tab === 'docs'}<DocsTab />{/if}
    {#if tab === 'fleet' && isAdmin}<FleetTab {home} />{/if}
  </div>
</div>
