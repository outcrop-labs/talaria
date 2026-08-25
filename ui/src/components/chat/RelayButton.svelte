<script lang="ts">
  // HAND THIS AGENT A CREDENTIAL WITHOUT TYPING IT INTO THE CHAT.
  //
  // The paste this exists to replace is the ordinary one — somebody needs their
  // agent to do a thing that takes a token, so they type the token into the
  // message box. From that moment it is in the transcript, in the database, in
  // the prompt of every later turn, and on its way to whichever provider serves
  // the next reply. Nothing downstream un-sends it.
  //
  // So: the same gesture, with the value routed around the conversation. It goes
  // straight to `/api/secrets/relay` on its own request and what comes back is a
  // HANDLE, which is what gets inserted into the editor. The value is never in
  // the editor's document, so it cannot be in the message — that is a property of
  // where it travelled, not a rule anybody has to remember.
  //
  // The field is cleared the instant the request returns, success or failure.
  // On success it is stored; on failure it is a credential sitting in a browser
  // tab, and retyping is a better outcome than leaving it there.
  import { KeyRound } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { popPanel, tileBase } from '@/components/chat/chat-chrome'
  import { pop, POPOVER } from '@/lib/motion'

  let {
    agentModel,
    agentLabel,
    onMinted,
    disabled,
  }: {
    agentModel: string
    /** Named in the copy so it is obvious WHO is being handed the credential —
     *  a relay is granted to exactly one agent, and that is the fact worth
     *  making unmissable at the moment somebody pastes a live token. */
    agentLabel: string
    /** The handle to drop into the composer. Never a value; there isn't one to
     *  pass — the response has no field that could carry it. */
    onMinted: (handle: string) => void
    disabled?: boolean
  } = $props()

  let wrapRef = $state<HTMLDivElement | null>(null)
  let open = $state(false)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let label = $state('')
  let value = $state('')
  let host = $state('')

  const close = () => {
    open = false
    error = null
    label = ''
    value = ''
    host = ''
  }

  const mint = async () => {
    if (!label.trim() || !value) return
    busy = true
    error = null
    const body = JSON.stringify({
      agentModel,
      label: label.trim(),
      value,
      // PIN IT TO A HOST if the human named one. A relay is already bounded three
      // ways — one agent, one use, one hour — and this is the fourth: the only
      // one that survives the agent being talked into spending it elsewhere.
      ...(host.trim() ? { allowedHosts: [host.trim().toLowerCase()] } : {}),
    })
    // Cleared before the await resolves rather than after: nothing below reads
    // it again, and the shorter it is bound to a live component the better.
    value = ''
    const r = await fetch('/api/secrets/relay', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body,
    }).catch(() => null)
    busy = false
    const j = (await r?.json().catch(() => ({}))) as { handle?: string; error?: string }
    if (!r?.ok || !j.handle) {
      error = j.error ?? 'could not hand that over; try again'
      return
    }
    onMinted(j.handle)
    close()
  }
</script>

<svelte:document
  onmousedown={(e) => {
    if (open && !wrapRef?.contains(e.target as Node)) close()
  }}
/>

<div bind:this={wrapRef} class="relative">
  <button
    type="button"
    title="Hand {agentLabel} a credential: one use, never in the transcript"
    disabled={disabled || busy}
    onclick={() => (open ? close() : (open = true))}
    class={tileBase}
  >
    {#if busy}
      <WaitingMark site="chat/relay" size={12} />
    {:else}
      <KeyRound size={14} aria-hidden="true" />
    {/if}
  </button>

  {#if open}
    <div in:pop={POPOVER} class={cn(popPanel, 'absolute bottom-full left-0 z-30 mb-1.5 w-80 p-2.5')}>
      <p class="font-sans text-[13px] text-fg">Hand {agentLabel} a credential</p>
      <p class="mt-1 font-sans text-xs text-muted">
        The value goes straight to the vault; it never enters this message, the transcript, or any model's context.
        {agentLabel} gets a handle it can spend <strong class="text-fg">once</strong>, within the hour{host.trim() ? ', and only at that host' : ''}.
      </p>

      <label class="mt-2.5 block font-sans text-xs text-muted">
        What is it
        <input
          bind:value={label}
          placeholder="Stripe test key"
          class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg"
        />
      </label>
      <label class="mt-2 block font-sans text-xs text-muted">
        Value
        <input
          bind:value
          type="password"
          autocomplete="off"
          spellcheck="false"
          placeholder="paste it here, not in the message"
          onkeydown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              void mint()
            }
          }}
          class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg"
        />
      </label>

      <label class="mt-2 block font-sans text-xs text-muted">
        Only for (optional)
        <input
          bind:value={host}
          placeholder="api.stripe.com"
          spellcheck="false"
          class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg"
        />
      </label>

      <div class="mt-2.5 flex items-center gap-2">
        <button
          type="button"
          disabled={busy || !label.trim() || !value}
          onclick={() => void mint()}
          class="rounded-md border border-line-strong px-2.5 py-1 font-sans text-xs text-fg transition-colors dither-fill disabled:opacity-40"
        >
          Hand it over
        </button>
        <button type="button" onclick={close} class="rounded-md px-2 py-1 font-sans text-xs text-muted transition-colors hover:text-fg">Cancel</button>
      </div>

      {#if error}<p class="mt-2 font-sans text-xs text-danger">{error}</p>{/if}
    </div>
  {/if}
</div>
