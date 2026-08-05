<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigate } from '@/router'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'

  type Kind = 'ticket' | 'channel' | 'fleet' | 'audit'

  interface ActivityEvent {
    at: string
    kind: Kind
    actor: string
    context: string
    detail: string
    type: string
    href: string
  }

  const KIND_META: Record<Kind, { label: string; icon: string; blurb: string }> = {
    ticket: { label: 'Tickets', icon: '⧉', blurb: 'Board activity — status moves, dispatches, comments, gaps.' },
    channel: { label: 'Channels', icon: '⋕', blurb: 'Messages in channels you belong to.' },
    fleet: { label: 'Fleet', icon: '◍', blurb: 'Agent configuration versions.' },
    audit: { label: 'Governance', icon: '⛨', blurb: 'Admin actions — settings, permissions, renders, deletions.' },
  }

  // The event's own type — the second-level answer to "where did this come
  // from" within a source. Warn-tinted for the ones worth a second look.
  const WARN_TYPES = new Set(['gap', 'blocked'])

  // Everything that happened across the workspace, organized by WHERE it came
  // from: one section per source (tickets, channels, fleet, governance), each
  // row labeled with its own event type. Chips narrow to the sources you care
  // about; governance is admin-only and off by default.
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  let kinds = $state<Kind[]>([])
  const query = createQuery(() => ({
    queryKey: ['activity', kinds.join(',')],
    queryFn: (): Promise<ActivityEvent[]> =>
      getList<ActivityEvent>(`/api/activity${kinds.length ? `?kinds=${kinds.join(',')}` : ''}`, 'events'),
    refetchInterval: 30_000,
  }))
  const events = $derived(query.data ?? [])
  // Stale-but-real beats blank: only a failure with nothing to fall back on
  // takes the feed over. "Nothing yet" may only come from a 200.
  const failed = $derived(query.isError && query.data === undefined)

  const available = $derived((Object.keys(KIND_META) as Kind[]).filter((k) => k !== 'audit' || isAdmin))
  const active = $derived(kinds.length ? kinds : available.filter((k) => k !== 'audit'))
  const toggle = (k: Kind) => {
    const cur = kinds.length ? kinds : active
    kinds = cur.includes(k) ? cur.filter((x) => x !== k) : [...cur, k]
  }

  const byKind = $derived.by(() => {
    const map = new Map<Kind, ActivityEvent[]>()
    for (const e of events) {
      const list = map.get(e.kind) ?? []
      list.push(e)
      map.set(e.kind, list)
    }
    return map
  })
</script>

<div class="space-y-6">
  <div class="flex items-center gap-1.5">
    <div class="ml-auto flex gap-1.5">
      <!-- Source filters — the one filter-pill primitive (Chip). -->
      {#each available as k (k)}
        {@const on = active.includes(k)}
        <Chip title={KIND_META[k].blurb} onSelect={() => toggle(k)} selected={on} class="px-2.5 py-1">
          {KIND_META[k].icon} {KIND_META[k].label}
        </Chip>
      {/each}
    </div>
  </div>

  {#if failed}
    <QueryError error={query.error} title="Could not load activity" onRetry={() => void query.refetch()} />
  {:else if query.isLoading}
    <SkeletonRows rows={8} avatar />
  {:else if events.length === 0}
    <EmptyState
      icon="☰"
      title="Nothing yet"
      hint="Ticket updates, channel messages, agent config changes — and for admins, governance actions — land here."
    />
  {:else}
    {#each active.filter((k) => byKind.get(k)?.length) as k (k)}
      <section>
        <!-- §8 section header: mono dim label + right-aligned mono count. -->
        <div class="mb-2 flex min-h-6 items-center gap-2">
          <span class="text-ink-dim">{KIND_META[k].icon}</span>
          <h2 class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{KIND_META[k].label}</h2>
          <span class="min-w-0 truncate font-sans text-xs text-muted">{KIND_META[k].blurb}</span>
          <span class="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted">{String(byKind.get(k)!.length).padStart(2, '0')}</span>
        </div>
        <Panel class="p-0">
          <div class="divide-y divide-line">
            {#each byKind.get(k)! as e, i (`${e.at}-${i}`)}
              <button
                type="button"
                onclick={() => void navigate(e.href)}
                class="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors hover:bg-hover"
              >
                <Avatar name={e.actor} class="mt-0.5 h-6 w-6 shrink-0 text-xs" />
                <div class="min-w-0 flex-1">
                  <div class="flex items-baseline gap-2">
                    <span class="truncate font-sans text-sm font-medium text-fg">{e.actor}</span>
                    <span class="shrink-0 font-mono text-[11px] text-muted">{e.context}</span>
                    {#if e.type}
                      <Chip tone={WARN_TYPES.has(e.type) ? 'warn' : 'neutral'} class="shrink-0">
                        {e.type}
                      </Chip>
                    {/if}
                    <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{relativeTime(e.at)}</span>
                  </div>
                  <div class="truncate font-sans text-sm text-muted">{e.detail}</div>
                </div>
              </button>
            {/each}
          </div>
        </Panel>
      </section>
    {/each}
  {/if}
</div>
