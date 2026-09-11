<script lang="ts">
  import { Check, Send, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { fade, QUICK } from '@/lib/motion'
  import type { BriefEvidence } from '@/lib/daily-brief-types'

  /**
   * A pending outbound action — an email or an event the assistant drafted —
   * and the decision about it.
   *
   * THE PAYLOAD IS THE CONFIRMATION. This block quotes the exact rows that
   * will leave the building (who receives it, what it says, who drafted it)
   * directly above the button that sends it, so "approve" is a click made
   * with the thing on screen — the same contract the drafted-reply block
   * next door keeps, and the reason neither carries a modal of its own.
   *
   * NO LINK ANYWHERE ELSE WOULD DO. The approval has no page to open: the
   * pending action lives behind an API the brief is the only surface for,
   * so deciding it here is not a shortcut past a destination — it is the
   * destination.
   */
  let {
    evidence,
    isEvent,
    onDecide,
  }: {
    /** The outbound payload, as the source recorded it. */
    evidence: BriefEvidence[]
    /** The verb on the approve button: an event is created, an email sent. */
    isEvent: boolean
    onDecide: (decision: 'approve' | 'reject') => Promise<{ ok: boolean; error?: string }>
  } = $props()

  let busy = $state<'approve' | 'reject' | null>(null)
  let error = $state<string | null>(null)

  async function decide(decision: 'approve' | 'reject'): Promise<void> {
    if (busy) return
    busy = decision
    error = null
    const result = await onDecide(decision)
    if (!result.ok) error = result.error ?? 'That did not go through.'
    busy = null
  }
</script>

<div class="mt-2 rounded-lg border border-line bg-surface p-3">
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    {isEvent ? 'Your assistant drafted an event' : 'Your assistant drafted an email'}
  </div>

  {#if evidence.length > 0}
    <dl class="mt-1.5 space-y-1">
      {#each evidence as row (row.label + row.text)}
        <div class="flex gap-2">
          <dt class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
            {row.label}
          </dt>
          <dd class="min-w-0 flex-1 whitespace-pre-wrap break-words font-sans text-[12.5px] leading-5 text-fg">{row.text}</dd>
        </div>
      {/each}
    </dl>
  {/if}

  {#if error}
    <p transition:fade={QUICK} class="mt-2 font-sans text-[12px] leading-5 text-[color:var(--theme-danger)]">{error}</p>
  {/if}

  <div class="mt-2.5 flex flex-wrap items-center gap-1.5">
    <Button size="xs" variant="outline" disabled={busy !== null} onclick={() => void decide('approve')}>
      {#if isEvent}
        <Check size={12} /> {busy === 'approve' ? 'Creating…' : 'Approve event'}
      {:else}
        <Send size={12} /> {busy === 'approve' ? 'Sending…' : 'Approve send'}
      {/if}
    </Button>
    <Button size="xs" variant="ghost" disabled={busy !== null} onclick={() => void decide('reject')}>
      <X size={12} /> Reject
    </Button>
  </div>
</div>
