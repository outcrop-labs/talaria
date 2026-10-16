<script lang="ts">
  import { toolRowLabel } from './tool-cards'
  import MessageAvatar from './MessageAvatar.svelte'
  import MessageActions from './MessageActions.svelte'
  import ReactionChips, { type ReactionChip } from './ReactionChips.svelte'
  import ToolStatus from './ToolStatus.svelte'
  import GuardCaveat from '@/components/chat/GuardCaveat.svelte'
  import StreamText from './StreamText.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Button from '@/components/ui/Button.svelte'
  import ChatWaiting from './ChatWaiting.svelte'
  import ChatChips from './ChatChips.svelte'
  import { fade } from '@/lib/motion'
  import { resolveAgentMedia } from '@/lib/agent-media'
  import { readFlag, writeFlag } from '@/lib/persist'
  import type { DisplayMessage } from './chat-view'

  // Flattened assistant turn (spec §10): avatar square + agent name, compact
  // mono tool rows, 14px sans body.
  let {
    message,
    turn,
    agentModel,
    agentLabel,
    live,
    time,
    reactions = [],
    onReact,
    onContextMenu,
    onInvoke,
    onDecided,
  }: {
    message: DisplayMessage
    /** Index in the thread — the handle that re-rolls the loader per turn. */
    turn: number
    agentModel: string
    agentLabel: string
    live: boolean
    /** Send time beside the name (R2); empty → none shown. */
    time?: string
    reactions?: ReactionChip[]
    /** Toggle the viewer's reaction. Omitted → no action bar: the caller
     *  withholds it while the turn streams or before it has a server id. */
    onReact?: (emoji: string) => void
    onContextMenu?: (e: MouseEvent) => void
    onInvoke?: (text: string) => void
    onDecided?: () => void
  } = $props()

  // Whether THIS viewer wants reasoning open. The trace stays collapsed by
  // default — it is the secondary read — but a reader following an agent's
  // thinking reopened it on every single turn, which is a preference the app
  // can just remember. Per-viewer convenience, so it rides `lib/persist`'s
  // flag pair: that module already owns the two storage facts (access itself
  // throws in a private window; whatever comes back degrades to the fallback),
  // and a browser that refuses storage simply gets the default.
  const REASONING_KEY = 'talaria.chat.reasoning-open'

  let reasoningOpen = $state(readFlag(REASONING_KEY))

  function setReasoningPref(next: boolean) {
    reasoningOpen = next
    writeFlag(REASONING_KEY, next)
  }

  const hasReasoning = $derived(!!message.reasoning?.trim())
  const tools = $derived(message.tools ?? [])
  const hasTools = $derived(tools.length > 0)
  const empty = $derived(!message.content && !hasReasoning && !hasTools)
  // The server writes a dead turn's reason INTO the row's content —
  // "(agent error: …)" — so the explanation survives reloads and reaches the
  // next turn's history. Render it as the error mark, not body prose: it's a
  // system fact about the turn, not something the agent said.
  const agentErrorNote = $derived(
    message.status === 'error' && message.content.startsWith('(agent error:')
      ? message.content.replace(/^\(agent error:\s*/, '').replace(/\)$/, '')
      : null,
  )
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the message context menu has no keyboard path -->
<div
  in:fade={{ duration: 150 }}
  class="group/message relative -mx-2 flex gap-2.5 rounded-md px-2 py-1 transition-colors hover:bg-card2 focus-within:bg-card2"
  oncontextmenu={onContextMenu}
>
  <!-- Agents have no image field yet (R19): initials. -->
  <MessageAvatar name={agentLabel} class="mt-0.5" />
  <div class="min-w-0 flex-1 space-y-2">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{agentLabel}</span>
      <span class="rounded border border-line px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">
        agent
      </span>
      {#if time}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{time}</span>{/if}
    </div>

    {#if hasReasoning}
      <!-- The preference is what the Disclosure's DEFAULT is, and Disclosure
           reads that once per mount (by design — a caller wanting another
           default remounts), so flipping it keys a remount here. That is also
           the behaviour a reader expects: choosing "keep open" opens this one
           too, rather than promising something about the next turn. -->
      <div class="flex items-start gap-2">
        <div class="min-w-0 flex-1">
          {#key reasoningOpen}
            <Disclosure title="Thinking" defaultOpen={reasoningOpen}>
              {#snippet icon()}<span>✦</span>{/snippet}
              <div class="whitespace-pre-wrap text-xs text-muted">{message.reasoning}</div>
            </Disclosure>
          {/key}
        </div>
        <Button
          variant="link"
          size="xs"
          class="mt-1 shrink-0 text-[11px]"
          title={reasoningOpen ? 'Collapse thinking by default' : 'Keep thinking open on every turn'}
          onclick={() => setReasoningPref(!reasoningOpen)}
        >
          {reasoningOpen ? 'collapse by default' : 'keep open'}
        </Button>
      </div>
    {/if}

    {#if hasTools}
      <Disclosure title={`${tools.length} tool ${tools.length === 1 ? 'call' : 'calls'}`}>
        {#snippet icon()}<span>⚙</span>{/snippet}
        <!-- Compact mono tool rows, hairline separated (spec §10). -->
        <ul class="divide-y divide-line">
          {#each tools as t, i (t.id ?? `${t.name}-${i}`)}
            {@const rowLabel = toolRowLabel(t.label)}
            <li class="flex items-start gap-2 py-1.5 font-mono text-[11px] first:pt-0 last:pb-0">
              <span class="mt-0.5 shrink-0">
                <ToolStatus status={t.status} />
              </span>
              <span class="min-w-0 flex-1">
                <span class="text-fg">{t.name}</span>
                {#if rowLabel}
                  <span class="mt-0.5 block whitespace-pre-wrap break-words text-muted">{rowLabel}</span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      </Disclosure>
    {/if}

    <!-- The auto-resume's visible half: the row the person watched die is
        back to streaming (metadata.resumed), so say THAT instead of letting a
        cleared-and-refilling turn read as a fresh unexplained one. -->
    {#if live && message.resumed}
      <div class="font-mono text-[10px] tracking-[0.05em] text-muted">
        ↻ stream dropped — picking the turn back up…
      </div>
    {/if}
    {#if message.content && agentErrorNote === null}
      <StreamText
        content={resolveAgentMedia(message.content, agentModel)}
        live={live}
        class="font-sans text-[13px] text-fg"
      />
    {/if}
    <!-- Mounted unconditionally (findings nulled while live) so the caveat's
        slide fires on the live→settled flip; behind an `{#if !live}` the
        local transition would be suppressed by the ancestor block toggling. -->
    <GuardCaveat findings={live ? null : message.guard} />
    <ChatChips chips={message.chips} content={message.content} {onInvoke} {onDecided} />

    <!-- Spec §9 state mapping. Which mark each of these draws is dealt per
        session by lib/waiting; the ROLE is fixed here — submitting while the
        first token is outstanding, reasoning once tools stream but no prose
        has landed — and the role is what sets the tempo. -->
    {#if empty && live}<ChatWaiting id={String(turn)} role="submitting" class="my-1" />{/if}
    {#if !empty && !message.content && live}<ChatWaiting id={String(turn)} role="reasoning" class="my-1" />{/if}
    {#if message.content && live}<span class="gd-pulse ml-0.5 inline-block h-4 w-1.5 bg-accent align-middle"></span>{/if}
    {#if !live && message.status === 'streaming'}
      <div class="font-mono text-[11px] text-muted">· saved (was in progress)</div>
    {/if}
    {#if !live && message.resumed}
      <div class="font-mono text-[10px] tracking-[0.05em] text-ink-dim">↻ resumed after a dropped stream</div>
    {/if}
    <!-- A stop is a decision, not a fault: the reader chose to end the turn,
        so it says so in the muted voice. It used to arrive as danger-red "·
        interrupted" — the typography of something going wrong — and an empty
        stop drew the routing-error line, which blamed the model's config for
        a key the person pressed. A turn the SERVER wrote a reason into still
        shows that reason: it is news, and stopping does not explain it. -->
    {#if !live && message.stopped && agentErrorNote === null}
      <div class="font-mono text-[11px] text-muted">· stopped</div>
    {:else if !live && message.status === 'error'}
      <div class="text-xs" style:color="var(--theme-danger)">
        {#if agentErrorNote}
          {agentErrorNote}
        {:else if empty}
          The agent returned nothing. Its model may not be routable; check its config and /models.
        {:else}
          · interrupted
        {/if}
      </div>
    {/if}
    {#if onReact}<ReactionChips {reactions} onToggle={onReact} />{/if}
  </div>
  <!-- Reactions only (R6): an agent DM has no threads, edits or deletes. -->
  {#if onReact}<MessageActions {onReact} />{/if}
</div>
