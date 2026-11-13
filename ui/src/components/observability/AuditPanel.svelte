<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { actorLabel } from './actor'
  import { fade, QUICK } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { table } from './grammar'

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

  const KIND_META: Record<Kind, { label: string; blurb: string }> = {
    ticket: { label: 'Tickets', blurb: 'Status moves, dispatches, comments and gaps on the boards.' },
    channel: { label: 'Channels', blurb: 'Messages in channels you belong to.' },
    fleet: { label: 'Agents', blurb: 'Changes to agent configuration.' },
    audit: { label: 'Admin actions', blurb: 'Settings, permissions, agent config updates and deletions.' },
  }

  // The event's own type, the second-level answer to "where did this come
  // from" within a source. Warn-tinted for the ones worth a second look.
  const WARN_TYPES = new Set(['gap', 'blocked'])
  const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)

  // Who, what, its type, where, when. One template so every source's columns line up.
  const COLUMNS = 'grid grid-cols-[13rem_minmax(0,1fr)_7rem_10rem_5.5rem] items-center gap-x-4 px-5'

  // Everything that happened across the workspace, one section per source,
  // each row labeled with its own event type. The chips narrow to the sources
  // you care about. Admin actions are admin-only and off by default.
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

{#snippet columnHeads()}
  <div class={cn(COLUMNS, 'h-10 font-sans text-xs font-medium leading-4 text-muted')}>
    <span>Who</span>
    <span>What</span>
    <span>Type</span>
    <span>Where</span>
    <span class="text-right">When</span>
  </div>
{/snippet}

<div class="space-y-10">
  <div class="flex flex-wrap items-center gap-2" role="group" aria-label="Sources shown">
    <span class="mr-1 font-sans text-[13px] leading-5 text-muted">Show</span>
    {#each available as k (k)}
      <Chip title={KIND_META[k].blurb} onSelect={() => toggle(k)} selected={active.includes(k)} class="h-8 px-3 text-[13px]">
        {KIND_META[k].label}
      </Chip>
    {/each}
  </div>

  {#if failed}
    <QueryError error={query.error} title="Could not load activity" onRetry={() => void query.refetch()} />
  {:else}
    <!-- Skeleton to content as one motion: the sketch is one section's
         silhouette and the resolved sections stagger in over it (count=1: how
         many sections land depends on the data). The failed branch above keeps
         owning errors, empty stays inside. -->
    <Materialize loading={query.isLoading} count={1} class="space-y-10">
      {#snippet skeleton()}
        <section aria-hidden="true">
          <div class="mb-3 space-y-2">
            <Skeleton class="h-4 w-20 rounded-full" />
            <Skeleton class="h-3 w-64 rounded-full" />
          </div>
          <div class="rounded-lg border border-line bg-panel">
            {@render columnHeads()}
            {#each [0, 1, 2, 3, 4, 5] as i (i)}
              <div class={cn(COLUMNS, 'h-11 border-t border-line-subtle')}>
                <span class="flex items-center gap-2.5">
                  <Skeleton class="h-6 w-6 shrink-0 rounded-full" />
                  <Skeleton class={`h-3 rounded-full ${['w-24', 'w-20', 'w-28'][i % 3]}`} />
                </span>
                <Skeleton class={`h-3 rounded-full ${['w-4/5', 'w-3/5', 'w-2/3'][i % 3]}`} />
                <Skeleton class="h-3 w-12 rounded-full" />
                <Skeleton class="h-3 w-16 rounded-full" />
                <span class="flex justify-end"><Skeleton class="h-3 w-12 rounded-full" /></span>
              </div>
            {/each}
          </div>
        </section>
      {/snippet}
      {#if events.length === 0}
        <div in:fade={{ duration: 150 }}>
          <EmptyState
            icon="☰"
            title="Nothing yet"
            hint="Ticket updates, channel messages, agent changes and admin actions land here."
          />
        </div>
      {:else}
        {#each active.filter((k) => byKind.get(k)?.length) as k (k)}
          <!-- Fires when a source chip toggles a section in or out, not on the
               initial render and not on poll updates that keep it present. -->
          <div in:fade={{ duration: 150 }} out:fade={QUICK}>
            <SettingsSection title={KIND_META[k].label} description={KIND_META[k].blurb}>
              <SettingsGroup>
                {@render columnHeads()}
                {#each byKind.get(k)! as e, i (`${e.at}-${i}`)}
                  <button
                    type="button"
                    onclick={() => void navigateHref(e.href)}
                    class={cn(
                      COLUMNS,
                      'h-11 w-full text-left font-sans text-sm leading-5 transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
                    )}
                  >
                    <span class="flex min-w-0 items-center gap-2.5">
                      <Avatar name={actorLabel(e.actor)} class="h-6 w-6 shrink-0" />
                      <span class="min-w-0 truncate font-medium text-fg" title={e.actor}>{actorLabel(e.actor)}</span>
                    </span>
                    <span class="min-w-0 truncate text-fg" title={e.detail}>{e.detail}</span>
                    <span class="flex min-w-0">
                      {#if e.type}<Chip tone={WARN_TYPES.has(e.type) ? 'warn' : 'neutral'}>{sentence(e.type)}</Chip>{/if}
                    </span>
                    <span class="min-w-0 truncate text-[13px] text-muted" title={e.context}>{e.context}</span>
                    <span class={cn(table.num, 'text-[13px] text-muted')}>{relativeTime(e.at)}</span>
                  </button>
                {/each}
              </SettingsGroup>
            </SettingsSection>
          </div>
        {/each}
      {/if}
    </Materialize>
  {/if}
</div>
