<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { CornerDownLeft, Sparkles, X } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { parseAgentStream } from '@/lib/sse-parse'
  import { fade, QUICK } from '@/lib/motion'
  import type { BriefLine, BriefView } from './daily-brief.svelte'

  /**
   * Talking to your assistant about the brief in front of you.
   *
   * EPHEMERAL, AND THE THREAD LIVES IN THIS COMPONENT. Nothing is persisted —
   * no conversation row, no messages, nothing distilled later — for the reason
   * stated in server/daily-brief-chat.ts: a person asks loose, half-formed
   * questions about their own day, and the moment those are minuted next to the
   * document they are about, they stop being asked.
   *
   * `focus` IS WHY THIS IS NOT THE APP-WIDE ASSISTANT DRAWER. Clicking the ask
   * icon on a line sends that line's key with the question, so "why is this
   * stuck?" resolves without the person having to retype which `this` they
   * mean. That is the whole of "chat about the changes easily".
   */
  let {
    brief,
    focus,
    onClearFocus,
  }: {
    brief: BriefView
    /** The line the person clicked "ask" on, if any. */
    focus: BriefLine | null
    onClearFocus: () => void
  } = $props()

  let thread = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  let draft = $state('')
  let replying = $state(false)
  let scroller = $state<HTMLDivElement | null>(null)

  // Suggestions, not a fabricated capability: each one is a question this
  // surface can genuinely answer from what it already holds.
  const PROMPTS = ['What changed since I last looked?', 'What should I do first?', 'What can wait until tomorrow?']

  async function send(content: string): Promise<void> {
    const text = content.trim()
    if (!text || replying || !brief.agent.configured) return
    const history = [...thread]
    draft = ''
    thread = [...thread, { role: 'user', content: text }, { role: 'assistant', content: '' }]
    replying = true
    // The focus is consumed by the question it was raised for. Leaving it
    // pinned would silently re-scope every later question in the thread to a
    // line the person has stopped talking about.
    const sourceKey = focus?.key ?? null
    onClearFocus()
    try {
      const res = await fetch('/api/brief/chat', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: text, history, sourceKey }),
      })
      if (!res.ok || !res.body) throw new Error(`chat failed: ${res.status}`)
      for await (const ev of parseAgentStream(res.body)) {
        if (ev.type !== 'content') continue
        const next = [...thread]
        const last = next[next.length - 1]!
        next[next.length - 1] = { ...last, content: last.content + ev.text }
        thread = next
        scroller?.scrollTo({ top: scroller.scrollHeight })
      }
    } catch {
      // The server's own message is not surfaced here on purpose — a gateway
      // error string in a chat bubble reads as the assistant speaking. What the
      // person needs is that the turn failed and can be retried.
      thread = [...thread.slice(0, -1), { role: 'assistant', content: '_Could not reach your assistant — try again._' }]
    } finally {
      replying = false
      scroller?.scrollTo({ top: scroller.scrollHeight })
    }
  }
</script>

<div class="flex h-full min-h-0 flex-col">
  <div bind:this={scroller} class="min-h-0 flex-1 space-y-4 overflow-y-auto px-1 pb-4">
    {#if thread.length === 0}
      <div class="flex items-start gap-2.5 pt-1">
        <Sparkles size={15} class="mt-0.5 shrink-0 text-accent" />
        <p class="font-sans text-[13px] leading-5 text-muted">
          Ask {brief.agent.name ?? 'your assistant'} about today. Nothing here is saved.
        </p>
      </div>
      <div class="flex flex-wrap gap-1.5">
        {#each PROMPTS as prompt (prompt)}
          <Button variant="outline" size="xs" class="rounded py-1 text-muted hover:border-line-strong hover:text-fg" onclick={() => void send(prompt)}>
            {prompt}
          </Button>
        {/each}
      </div>
    {/if}

    {#each thread as message, i (i)}
      {#if message.role === 'user'}
        <div class="flex justify-end">
          <div
            class="max-w-[85%] rounded-lg border px-3 py-2 font-sans text-[13px] leading-5"
            style:background="var(--chat-user-bg)"
            style:border-color="var(--chat-user-border)"
            style:color="var(--chat-user-foreground)"
          >
            {message.content}
          </div>
        </div>
      {:else}
        <div class="font-sans text-[13px] leading-5">
          <!-- GENERATING, NOT SKELETON. An assistant turn with no tokens yet is
               MODEL OUTPUT being written, not a fetch that has not resolved —
               see the taxonomy in WaitingMark.svelte. The border and padding
               are dropped because this reply is bare text in a thread, not a
               block with a surface of its own; `site` is left at its default
               since it only reaches a WaitingMark when a `label` is given. -->
          {#if message.content}<Markdown children={message.content} />{:else}<Generating lines={2} class="border-none p-0" />{/if}
        </div>
      {/if}
    {/each}
  </div>

  {#if focus}
    <!-- What the next question is ABOUT, shown before it is asked. Without this
         the scoping is invisible and the answer looks like a non-sequitur. -->
    <div transition:fade={QUICK} class="mb-2 flex items-center gap-2 rounded-lg border border-line bg-raised px-2.5 py-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Asking about</span>
      <span class="min-w-0 flex-1 truncate font-sans text-[12px] text-fg">{focus.current.title}</span>
      <IconButton size="sm" title="Ask about the whole day instead" onclick={onClearFocus}>
        <X size={12} />
      </IconButton>
    </div>
  {/if}

  <div class="flex items-end gap-2">
    <Textarea
      autoGrow
      rows={1}
      bind:value={draft}
      disabled={!brief.agent.configured}
      placeholder={focus ? 'Ask about this line…' : 'Ask about your day…'}
      onkeydown={(e: KeyboardEvent) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          void send(draft)
        }
      }}
    />
    <IconButton size="sm" title="Send" disabled={replying || !draft.trim()} onclick={() => void send(draft)}>
      <CornerDownLeft size={13} />
    </IconButton>
  </div>
</div>
