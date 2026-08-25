<script lang="ts">
  import { Check, Send, UserCheck, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { fade, QUICK } from '@/lib/motion'
  import type { CommsState } from './daily-brief.svelte'

  /**
   * The drafted reply, and the decision about it.
   *
   * IT LIVES ON THE LINE IT ANSWERS. A drafted reply was nearly filed with the
   * other approvals, and that is unanswerable as a card: "should this go?"
   * cannot be decided without the message it replies to directly above it. So
   * the draft renders inside the conversation row, quoting itself.
   *
   * A STALE DRAFT SHOWS ITS SEND CONTROL DISABLED rather than hiding it. The
   * person came here to send something; a control that vanished would read as a
   * bug, and the sentence next to it is the actual information — they said
   * something else, so this would answer the wrong message.
   */
  let {
    comms,
    peer,
    onDecide,
    onDelegate,
  }: {
    comms: CommsState
    peer: string
    onDecide: (draftId: string, decision: 'approve' | 'reject') => Promise<{ ok: boolean; error?: string }>
    onDelegate: (channelId: string, granted: boolean) => void
  } = $props()

  let busy = $state<'approve' | 'reject' | null>(null)
  let error = $state<string | null>(null)

  async function decide(decision: 'approve' | 'reject'): Promise<void> {
    if (!comms.draft || busy) return
    busy = decision
    error = null
    const result = await onDecide(comms.draft.id, decision)
    if (!result.ok) error = result.error ?? 'That did not go through.'
    busy = null
  }
</script>

<div class="mt-2 rounded-lg border border-line bg-surface p-3">
  {#if comms.draft}
    <div class="flex items-center gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Your assistant drafted a reply
      </span>
      {#if comms.draft.stale}
        <!-- Not a warning about the draft's quality — about its timing. -->
        <Chip tone="warn">OUT OF DATE</Chip>
      {/if}
    </div>

    <!-- Quoted, in the reading voice, because this is prose somebody is about
         to send to a colleague and has to actually read before they do. -->
    <p class="mt-1.5 border-l-2 border-line-strong pl-3 font-sans text-[13px] leading-5 text-fg">
      {comms.draft.content}
    </p>

    {#if comms.draft.stale}
      <p class="mt-2 font-sans text-[12px] leading-5 text-muted">
        {peer} has said something since this was written, so it would answer the wrong message. Your assistant will draft a
        fresh one on the next pass.
      </p>
    {/if}

    {#if error}
      <p transition:fade={QUICK} class="mt-2 font-sans text-[12px] leading-5 text-[color:var(--theme-danger)]">{error}</p>
    {/if}

    <div class="mt-2.5 flex flex-wrap items-center gap-1.5">
      <Button size="xs" variant="outline" disabled={comms.draft.stale || busy !== null} onclick={() => void decide('approve')}>
        <Send size={12} /> {busy === 'approve' ? 'Sending…' : 'Send it'}
      </Button>
      <Button size="xs" variant="ghost" disabled={busy !== null} onclick={() => void decide('reject')}>
        <X size={12} /> Discard
      </Button>
      <span class="ml-auto"></span>
      {@render delegateToggle()}
    </div>
  {:else}
    <div class="flex flex-wrap items-center gap-2">
      <span class="font-sans text-[12.5px] leading-5 text-muted">
        {#if comms.delegated}
          Your assistant answers this conversation for you.
        {:else}
          Your assistant will draft a reply here for you to approve.
        {/if}
      </span>
      <span class="ml-auto"></span>
      {@render delegateToggle()}
    </div>
  {/if}
</div>

{#snippet delegateToggle()}
  <!-- THE ONLY CONTROL HERE THAT CHANGES WHAT HAPPENS WITHOUT YOU. Worded as
       what it does rather than as a setting name, and its "on" state says so
       plainly, because the cost of misreading this one is a message sent to a
       colleague that the owner never saw. -->
  {#if comms.delegated}
    <Button size="xs" variant="ghost" onclick={() => onDelegate(comms.channelId, false)}>
      <UserCheck size={12} /> Assistant handles this (stop)
    </Button>
  {:else}
    <Button size="xs" variant="ghost" onclick={() => onDelegate(comms.channelId, true)}>
      <Check size={12} /> Let my assistant reply here
    </Button>
  {/if}
{/snippet}
