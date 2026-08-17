<script lang="ts">
  import { createRawSnippet, type Snippet } from 'svelte'
  import { navigate } from '@/router'
  import Tabs from '@/components/ui/Tabs.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useSession } from '@/lib/session'
  import { useChannels } from '@/lib/channels.svelte'
  import { useHome, type HomeTab } from './home'

  // THE HOME TAB STRIP, in one place because it belongs on BOTH sides of Home.
  //
  // It used to live inside ConsoleHome, which renders only for the non-inbox
  // tabs — so the strip was a one-way door. Inbox is the landing tab AND is
  // listed in the strip, so you could navigate to it and then had no way back
  // to Boards, Comms or anything else without editing the URL. The console
  // could see the whole of Home; the surface most people actually land on
  // could see none of it.
  //
  // Both panes mount their own copy. The queries behind the badges are shared
  // by key, so this is one fetch either way.
  let { value }: { value: HomeTab } = $props()

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const home = useHome()
  // Feeds the Comms badge count. Defaulted, a failed read renders "no unread" —
  // the one thing a badge exists to deny. Suppressing the badge is only half the
  // fix: a missing badge is indistinguishable from a genuine zero, and the lie
  // is told from EVERY tab, so the marker goes under the strip, where it is
  // visible wherever you are standing.
  const channelsList = listQuery(useChannels(), { title: 'Unread counts unavailable', variant: 'inline' })
  const unreadComms = $derived(
    channelsList.failed ? 0 : channelsList.rows.reduce((n, c) => n + (c.unreadCount ?? 0), 0),
  )

  // Inbox is `/`; the console tabs live under `/home/<tab>` rather than at the
  // root, because their names — boards, comms, research — are the names of real
  // views. `/boards` is the board list; `/home/boards` is Home's summary of it.
  const setTab = (t: HomeTab) => {
    if (t === 'inbox') void navigate('/')
    else void navigate('/home/:tab', { params: { tab: t } })
  }

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
  // Tabs.svelte expects.
  const tabLabel = (label: string, badge?: number): Snippet =>
    createRawSnippet(() => ({
      render: () =>
        `<span class="flex items-center gap-1.5">${label}${
          badge !== undefined ? `<span class="font-medium text-accent">${badge}</span>` : ''
        }</span>`,
    }))
</script>

<Tabs {value} onChange={setTab} items={tabs.map((t) => ({ id: t.id, label: tabLabel(t.label, t.badge) }))} />
{#if channelsList.notice}
  <QueryError {...channelsList.notice} />
{/if}
