<script lang="ts">
  import { CalendarClock, PenLine, Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { navigate } from '@/router'
  import type { BriefAbsent } from './daily-brief.svelte'

  /**
   * No brief — and WHICH kind of no brief.
   *
   * THREE ABSENCES, THREE RENDERINGS, which is the empty-≠-broken rule applied
   * to a surface where the distinction is unusually load-bearing. "Your brief
   * opens at 07:00", "you have no assistant" and "it is being written right
   * now" look identical if you collapse them, and the first two are states to
   * read while the third is a state to wait through.
   *
   * There is deliberately no fourth state for "the scheduled run missed and
   * nothing will write it": the read itself opens a missing brief that is due
   * (see getBrief), so the reader can no longer arrive at a blank page that
   * stays blank all day. What used to render here as a bug report now renders
   * as 'writing' — because it is being fixed in the same request that noticed.
   */
  let { state }: { state: BriefAbsent } = $props()

  const nextLabel = $derived(
    state.nextAt
      ? new Intl.DateTimeFormat(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' }).format(new Date(state.nextAt))
      : null,
  )
</script>

{#if state.absent === 'no-agent'}
  <!-- The only one of the three with an action, because it is the only one the
       person can do anything about. A brief is written by YOUR assistant and by
       no other model — see the note in server/daily-brief.ts — so without one
       there is nothing to wait for. -->
  <EmptyState
    icon={sparkle}
    title="You don’t have a personal assistant yet"
    hint="Your daily brief is written by your own assistant, and by nothing else — it reads your private work, so it never runs on a model somebody else picked. Set one up and your first brief opens tomorrow morning."
    action={setup}
  />
{:else if state.absent === 'writing'}
  <!-- The one state that moves. The page is polling fast; the moment the
       document's first batch lands, this whole state is replaced by the brief.
       No action, because there is nothing to do but let it finish — and a
       manual refresh control would imply the page is not already doing that. -->
  <EmptyState
    icon={pen}
    title="{state.agent.name ?? 'Your assistant'} is writing today’s brief"
    hint="The lede, your calendar, what needs you and who is waiting — it takes a few seconds, and the page fills itself in the moment it lands."
    action={writing}
  />
{:else}
  <!-- 'pending': the hour has not come. Honest, and it says WHEN. -->
  <EmptyState
    icon={clock}
    title={nextLabel ? `Your next brief opens ${nextLabel}` : 'Your next brief has not opened yet'}
    hint="It arrives two hours before the workday starts, so it is waiting for you rather than the other way around."
  />
{/if}

{#snippet sparkle()}<Sparkles size={22} />{/snippet}
{#snippet clock()}<CalendarClock size={22} />{/snippet}
{#snippet pen()}<PenLine size={22} />{/snippet}
{#snippet writing()}
  <div class="flex items-center justify-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    <WaitingMark site="brief/writing" size={12} /> writing
  </div>
{/snippet}
{#snippet setup()}
  <Button size="sm" variant="outline" onclick={() => void navigate('/agents')}>Set up your assistant</Button>
{/snippet}
