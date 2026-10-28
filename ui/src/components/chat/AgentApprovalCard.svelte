<script lang="ts">
  import { Check, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { useSession } from '@/lib/session'
  import { agentApprovalView, clockText, decideChip, type ChatChip } from '@/lib/chips'

  // THE AGENT IS ASKING BEFORE IT GOES ON. A Developer Agent runs its flagged
  // commands without asking, so what still reaches a person is one of the few
  // things Hermes never lets anything wave through. On a Talaria agent today
  // that is an MCP server asking to confirm an action ("Delete record 42?").
  // Hermes' untrusted-server, spend-limit and card-fill gates would arrive the
  // same way, but Talaria configures none of them, so they do not come up. (A
  // write to a project's AGENTS.md / CLAUDE.md outside the agent's data
  // folder is never carded: the write root refuses it whatever the answer, so
  // the door refuses it first.) The card shows Hermes' own words for whichever
  // it is. This conversation's turn is paused on the answer; the agent's other
  // conversations are not.
  //
  // ONE PERSON ANSWERS: the conversation's owner, who manages the agent
  // (`deciderId`). Anyone else reading the thread sees the card and what
  // became of it, never the buttons; the server checks the same rule again.
  // The answers are "once" and "deny" — the agent's "always" would rewrite a
  // config Talaria owns.
  //
  // IT CLOSES ITSELF. Unanswered, the card is denied a few seconds before the
  // agent would stop waiting, and reads expired; the countdown says how long
  // is left, so nobody is surprised by it.
  let { chip, onDecided }: { chip: ChatChip; onDecided?: () => void } = $props()

  const session = useSession()
  let now = $state(Date.now())
  let busy = $state<'approve' | 'deny' | null>(null)
  let error = $state<string | null>(null)
  const view = $derived(agentApprovalView(chip, session.data?.id, now))

  $effect(() => {
    if (!view.waiting) return
    const tick = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(tick)
  })

  async function answer(decision: 'approve' | 'deny') {
    if (!chip.actionId || busy) return
    busy = decision
    error = null
    const result = await decideChip(chip.actionId, decision)
    busy = null
    if (!result.ok) error = result.error ?? 'That did not go through.'
    // Refetch either way: a refusal from the server (the agent stopped
    // waiting) repaints the card as expired.
    onDecided?.()
  }
</script>

<div class="w-full rounded-lg border border-line bg-surface px-3 py-2" role="group" aria-label={chip.summary || 'The agent is asking'}>
  <div class="flex items-center justify-between gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    <span>{view.heading}</span>
    {#if view.secondsLeft !== null}
      <span title="Unanswered, the agent is told no when this runs out">closes in {clockText(view.secondsLeft)}</span>
    {/if}
  </div>
  <p class="mt-1 font-sans text-[13px] text-fg">{chip.summary || 'The agent is asking before it goes on.'}</p>
  {#if chip.command}
    <code class="mt-1 block break-words rounded-md border border-line-subtle bg-raised px-2 py-1 font-mono text-xs text-fg">{chip.command}</code>
  {/if}
  {#if error}
    <p class="mt-1 font-sans text-xs text-[color:var(--theme-danger)]">{error}</p>
  {/if}
  {#if view.canAnswer}
    <div class="mt-2 flex flex-wrap gap-1.5">
      <Button size="xs" variant="outline" disabled={busy !== null} onclick={() => void answer('approve')}>
        <Check size={12} /> {busy === 'approve' ? 'Allowing' : 'Allow once'}
      </Button>
      <Button size="xs" variant="ghost" disabled={busy !== null} onclick={() => void answer('deny')}>
        <X size={12} /> Deny
      </Button>
    </div>
  {:else if view.waiting}
    <p class="mt-1.5 font-sans text-xs text-muted">
      Waiting on the conversation's owner. Unanswered, the agent is told no when the time runs out.
    </p>
  {/if}
</div>
