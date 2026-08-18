<script lang="ts">
  import { bottomStick } from '@/lib/stick-to-bottom'
  import Button from '@/components/ui/Button.svelte'
  import { CornerDownLeft, Sparkles, X } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { parseAgentStream } from '@/lib/sse-parse'
  import { fade, QUICK } from '@/lib/motion'
  import type { BriefLine, BriefView } from './daily-brief.svelte'

  /**
   * Talking to your assistant about the brief in front of you.
   *
   * THE THREAD IS SAVED, PER LINE. It began as component state on an
   * ephemerality argument, and the argument lost to what the surface is for:
   * the answers here END IN A NAVIGATION — you ask why a ticket is stuck, you
   * go and look at it, you come back — so a thread held in this component was
   * destroyed by the one action it existed to prompt. It now loads from
   * `/api/brief/chat?sourceKey=` and every turn is persisted server-side.
   *
   * ONE THREAD PER LINE, plus one for the day (`focus === null`). Switching
   * focus swaps threads rather than continuing one, which is why the load is
   * keyed on the focus rather than done once at mount.
   *
   * `focus` IS ALSO WHY THIS IS NOT THE APP-WIDE ASSISTANT DRAWER. Clicking the ask
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
  let loading = $state(false)
  let scroller = $state<HTMLDivElement | null>(null)
  // Was an unconditional scrollTo on every token, which yanked anyone who
  // scrolled up mid-reply straight back down. Same rule as the other chats now.
  const stick = bottomStick()
  $effect(() => stick.attach(scroller))

  // Which thread is on screen. Tracked separately from `focus` so a reply
  // streaming into the ledger thread cannot be appended to Dana's because the
  // person clicked away mid-answer.
  let loadedKey = $state<string | null | undefined>(undefined)

  $effect(() => {
    const key = focus?.key ?? null
    if (key === loadedKey || replying) return
    loadedKey = key
    thread = []
    loading = true
    const url = key ? `/api/brief/chat?sourceKey=${encodeURIComponent(key)}` : '/api/brief/chat'
    void fetch(url, { credentials: 'same-origin' })
      .then((r) => (r.ok ? (r.json() as Promise<{ messages: Array<{ role: 'user' | 'assistant'; content: string }> }>) : null))
      .then((payload) => {
        // Guard on the key: a load still in flight for a thread the person has
        // navigated away from must not paint over the one now on screen.
        if (loadedKey !== key) return
        thread = payload?.messages ?? []
      })
      .catch(() => {})
      .finally(() => {
        if (loadedKey === key) loading = false
      })
  })

  // Suggestions, not a fabricated capability: each one is a question this
  // surface can genuinely answer from what it already holds.
  const PROMPTS = ['What changed since I last looked?', 'What should I do first?', 'What can wait until tomorrow?']

  async function send(content: string): Promise<void> {
    const text = content.trim()
    if (!text || replying || !brief.agent.configured) return
    const history = [...thread]
    draft = ''
    stick.jump() // your own message is always worth showing you
    thread = [...thread, { role: 'user', content: text }, { role: 'assistant', content: '' }]
    replying = true
    // The focus is NOT cleared on send any more. It used to be, on the reasoning
    // that leaving it pinned would silently re-scope later questions — but now
    // that the thread is saved per line, the focus IS the thread's identity.
    // Clearing it would move the next question into the day-level conversation
    // and strand the answer the person just got.
    const sourceKey = focus?.key ?? null
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
        stick.follow()
      }
    } catch {
      // The server's own message is not surfaced here on purpose — a gateway
      // error string in a chat bubble reads as the assistant speaking. What the
      // person needs is that the turn failed and can be retried.
      thread = [...thread.slice(0, -1), { role: 'assistant', content: '_Could not reach your assistant — try again._' }]
    } finally {
      replying = false
      stick.follow()
    }
  }
</script>

<div class="flex h-full min-h-0 flex-col">
  <div bind:this={scroller} class="min-h-0 flex-1 space-y-4 overflow-y-auto px-1 pb-4">
    {#if loading}
      <!-- A FETCH, so signal static rather than the generating bars below:
           these are the person's own saved words being read back, not a model
           writing new ones. -->
      <SkeletonRows rows={3} />
    {:else if thread.length === 0}
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
