<script lang="ts">
  import { CalendarClock, Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { navigate } from '@/router'
  import type { BriefAbsent } from './daily-brief.svelte'

  /**
   * No brief — and WHICH kind of no brief.
   *
   * THREE ABSENCES, THREE RENDERINGS, which is the empty-≠-broken rule applied
   * to a surface where the distinction is unusually load-bearing. "Your brief
   * opens at 07:00" and "your brief did not get written" look identical if you
   * collapse them, and the second one is a bug report the person will never
   * file because the screen told them everything was fine.
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
{:else if state.absent === 'pending'}
  <EmptyState
    icon={clock}
    title={nextLabel ? `Your next brief opens ${nextLabel}` : 'Your next brief has not opened yet'}
    hint="It arrives two hours before the workday starts, so it is waiting for you rather than the other way around."
  />
{:else}
  <!-- 'none': the hour passed and nothing was written. NOT dressed up as a
       quiet morning — that is the failure this whole split exists to avoid. -->
  <EmptyState
    icon={clock}
    title="Today’s brief hasn’t been written"
    hint={nextLabel
      ? `The scheduled run should already have happened. The next one is ${nextLabel} — if this keeps happening, the daily-brief job is worth checking on Observability.`
      : 'The scheduled run should already have happened. If this keeps happening, the daily-brief job is worth checking on Observability.'}
  />
{/if}

{#snippet sparkle()}<Sparkles size={22} />{/snippet}
{#snippet clock()}<CalendarClock size={22} />{/snippet}
{#snippet setup()}
  <Button size="sm" variant="outline" onclick={() => void navigate('/agents')}>Set up your assistant</Button>
{/snippet}
