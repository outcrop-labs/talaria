<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { fade, QUICK } from '@/lib/motion'
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
  {:else}
    <!-- Skeleton → content as one motion: the sketch is one section's
         silhouette — header line, then a panel of event-row shapes (avatar +
         actor/context bars + trailing time) — and the resolved sections
         stagger in over it (count=1: how many sections land depends on the
         data). Materialize direct; the failed branch above keeps owning
         errors, empty stays inside. -->
    <Materialize loading={query.isLoading} count={1} class="space-y-6">
      {#snippet skeleton()}
        <section aria-hidden="true">
          <div class="mb-2 flex min-h-6 items-center gap-2">
            <Skeleton class="h-3.5 w-3.5 rounded" />
            <Skeleton class="h-2.5 w-16 rounded-full" />
            <Skeleton class="h-2.5 w-48 rounded-full" />
            <span class="ml-auto"><Skeleton class="h-2.5 w-6 rounded" /></span>
          </div>
          <div class="rounded-lg border border-line bg-panel">
            <div class="divide-y divide-line">
              {#each [0, 1, 2, 3, 4, 5] as i (i)}
                <div class="flex items-start gap-3 px-6 py-3">
                  <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded-full" />
                  <div class="min-w-0 flex-1 space-y-1.5">
                    <div class="flex items-center gap-2">
                      <Skeleton class={`h-3.5 rounded-full ${['w-24', 'w-20', 'w-28'][i % 3]}`} />
                      <Skeleton class="h-2.5 w-16 rounded" />
                      <span class="ml-auto"><Skeleton class="h-2.5 w-12 rounded" /></span>
                    </div>
                    <Skeleton class={`h-3 rounded-full ${['w-4/5', 'w-3/5', 'w-2/3'][i % 3]}`} />
                  </div>
                </div>
              {/each}
            </div>
          </div>
        </section>
      {/snippet}
      {#if events.length === 0}
      <div in:fade={{ duration: 150 }}>
        <EmptyState
          icon="☰"
          title="Nothing yet"
          hint="Ticket updates, channel messages, agent config changes — and for admins, governance actions — land here."
        />
      </div>
    {:else}
    {#each active.filter((k) => byKind.get(k)?.length) as k (k)}
      <!-- Fires when a source filter chip toggles a section in/out — not on the
           initial content render (local default) and not on poll updates that
           keep the section present. -->
      <section in:fade={{ duration: 150 }} out:fade={QUICK}>
        <!-- §8 section header: mono dim label + right-aligned mono count. -->
        <div class="mb-2 flex min-h-6 items-center gap-2">
          <span class="text-ink-dim">{KIND_META[k].icon}</span>
          <h2 class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{KIND_META[k].label}</h2>
          <span class="min-w-0 truncate font-sans text-xs text-muted">{KIND_META[k].blurb}</span>
          <span class="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted">{String(byKind.get(k)!.length).padStart(2, '0')}</span>
        </div>
        <Panel class="p-0">
          <!-- No listStagger on the rows: Materialize's content branch owns
               the region's cascade (sections rise; rows come with them). -->
          <div class="divide-y divide-line">
            {#each byKind.get(k)! as e, i (`${e.at}-${i}`)}
              <button data-dither-fill
                type="button"
                onclick={() => void navigateHref(e.href)}
                class="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors"
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
    </Materialize>
  {/if}
</div>
