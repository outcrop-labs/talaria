<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { CalendarDays, ExternalLink, Plus } from '@lucide/svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QuickEvent from './QuickEvent.svelte'
  import { slide } from '@/lib/motion'
  import { formatWhen, useAgenda, useGoogleStatus } from './home'

  // The user's Google Calendar agenda, shown only when they've connected Google.
  // Stays invisible otherwise so Home isn't cluttered for the unconnected.
  const qc = useQueryClient()
  const google = useGoogleStatus()
  const connected = $derived(!google.isError && google.data?.connected === true)
  const agenda = useAgenda(() => connected)
  let adding = $state(false)

  const events = $derived(agenda.data?.events ?? [])
</script>

{#if !google.data && !google.isError}
  <!-- Status in flight → hold the space with a skeleton (no pop-in). -->
  <Panel>
    <Skeleton class="mb-4 h-3 w-20 rounded-full" />
    <SkeletonRows rows={3} />
  </Panel>
{:else if !connected}
  <Panel>
    <SectionHeader title="Agenda" action="Google Calendar" />
    <EmptyState variant="inline" title="Connect Google Calendar to see your agenda here." />
  </Panel>
{:else if !agenda.data && !agenda.isError}
  <Panel>
    <Skeleton class="mb-4 h-3 w-20 rounded-full" />
    <SkeletonRows rows={3} />
  </Panel>
{:else if agenda.isError || agenda.data?.error || !agenda.data}
  <Panel>
    <SectionHeader title="Agenda" action="Unavailable" />
    <EmptyState variant="inline" title="Calendar is temporarily unavailable." />
  </Panel>
{:else}
  <Panel>
    <div class="mb-3 flex min-h-6 items-center gap-2">
      <CalendarDays size={14} class="text-ink-dim" />
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agenda</span>
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Google Calendar</span>
      <Button variant="ghost" size="xs" class="ml-auto gap-1 text-accent hover:underline" onclick={() => (adding = !adding)}>
        <Plus size={12} /> New event
      </Button>
    </div>

    {#if adding}
      <div transition:slide={{ duration: 150 }}>
        <QuickEvent
          onDone={async () => {
            adding = false
            await qc.invalidateQueries({ queryKey: ['agenda'] })
          }}
        />
      </div>
    {/if}

    {#if events.length === 0}
      <EmptyState variant="inline" class="py-3" title="Nothing on the calendar coming up." />
    {:else}
      <div class="divide-y divide-line">
        {#each events as e (e.id)}
          <a
            href={e.htmlLink ?? '#'}
            target="_blank"
            rel="noreferrer"
            class="group flex items-center gap-3 py-2 transition-colors hover:bg-card2"
          >
            <span class="w-32 shrink-0 font-mono text-[11px] text-muted">{formatWhen(e)}</span>
            <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{e.summary}</span>
            {#if e.location}<span class="hidden shrink-0 truncate font-sans text-[11px] text-muted sm:block sm:max-w-[8rem]">{e.location}</span>{/if}
            <ExternalLink size={12} class="shrink-0 text-muted opacity-0 group-hover:opacity-100" />
          </a>
        {/each}
      </div>
    {/if}
  </Panel>
{/if}
