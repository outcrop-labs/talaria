<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import PresenceAvatar from './PresenceAvatar.svelte'

  // Your own status, pinned to the bottom of the Comms rail (R13): you, your
  // presence, and the emoji + line teammates see beside your name. Clicking
  // the bar opens the editor IN the rail, above the bar, rather than in a
  // popover: the emoji picker is itself a popover, and a popover inside a
  // popover closes on the inner one's clicks. No expiry; clearing is manual.
  // Bounds mirror the server (KTD7): text ≤ 100 chars.
  const MAX_TEXT = 100
  const SUGGESTED: Array<[string, string]> = [
    ['📅', 'In a meeting'],
    ['🎧', 'Heads down'],
    ['🚌', 'Commuting'],
    ['🌴', 'Out of office'],
  ]

  const qc = useQueryClient()
  const session = useSession()
  const user = $derived(session.data)

  let open = $state(false)
  let emoji = $state<string | null>(null)
  let text = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const savedEmoji = $derived(user?.statusEmoji ?? null)
  const savedText = $derived(user?.statusText ?? '')
  const hasStatus = $derived(!!savedEmoji || !!savedText)
  const dirty = $derived(emoji !== savedEmoji || text.trim() !== savedText)

  // Opening starts from what is saved; the draft never outlives the editor.
  const toggle = () => {
    if (!open) {
      emoji = savedEmoji
      text = savedText
      error = null
    }
    open = !open
  }

  const send = async (body: { statusEmoji: string | null; statusText: string | null }) => {
    error = null
    busy = true
    try {
      await putJson<{ ok: true }>('/api/me', body)
      await qc.invalidateQueries({ queryKey: ['session'] })
      await qc.invalidateQueries({ queryKey: ['users'] })
      open = false
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
    }
  }

  const save = () => {
    if (!dirty || text.trim().length > MAX_TEXT) return
    void send({ statusEmoji: emoji, statusText: text.trim() || null })
  }
  const clear = () => void send({ statusEmoji: null, statusText: null })
</script>

{#if user}
  {#if open}
    <div transition:slide={{ duration: 150 }} class="mb-2 space-y-2 rounded-md border border-line bg-raised p-2">
      <div class="flex items-center gap-1.5">
        <EmojiPicker onPick={(e) => (emoji = e)} onClear={() => (emoji = null)}>
          {#snippet trigger()}
            <button
              type="button"
              class="grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line-strong text-base leading-none transition-colors dither-fill"
              title="Pick a status emoji"
              aria-label="Pick a status emoji"
            >
              {#if emoji}{emoji}{:else}<span class="text-muted">☺</span>{/if}
            </button>
          {/snippet}
        </EmojiPicker>
        <Input
          size="sm"
          bind:value={text}
          maxlength={MAX_TEXT}
          aria-label="Status"
          placeholder="What's your status?"
          onkeydown={(e) => {
            if (e.key === 'Enter') save()
            else if (e.key === 'Escape') open = false
          }}
        />
      </div>
      <!-- One click to a common status: fills the draft, Save commits it. -->
      <div class="flex flex-wrap gap-1">
        {#each SUGGESTED as [e, t] (t)}
          <button
            type="button"
            onclick={() => {
              emoji = e
              text = t
            }}
            class="flex items-center gap-1 rounded-md border border-line px-1.5 py-0.5 text-xs text-muted transition-colors hover:text-fg dither-fill"
          >
            <span>{e}</span><span>{t}</span>
          </button>
        {/each}
      </div>
      {#if error}<p class="text-xs text-danger" role="alert">{error}</p>{/if}
      <div class="flex items-center gap-2">
        {#if hasStatus}
          <Button variant="link" size="xs" disabled={busy} onclick={clear}>Clear status</Button>
        {/if}
        <span class="ml-auto"></span>
        <Button size="sm" variant="ghost" disabled={busy} onclick={() => (open = false)}>Cancel</Button>
        <Button size="sm" disabled={busy || !dirty} onclick={save}>Save</Button>
      </div>
    </div>
  {/if}

  <button
    type="button"
    onclick={toggle}
    aria-expanded={open}
    title={hasStatus ? 'Edit your status' : 'Set a status'}
    class={cn(
      'flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-card2',
      open && 'bg-card2',
    )}
  >
    <PresenceAvatar name={user.name ?? user.email ?? 'You'} src={user.picture} presence="online" />
    <span class="min-w-0 flex-1">
      <span class="block truncate font-sans text-[13px] font-medium text-fg">{user.name ?? user.email}</span>
      {#if hasStatus}
        <span class="block truncate text-xs text-muted">{savedEmoji ?? ''} {savedText}</span>
      {:else}
        <span class="block truncate text-xs text-ink-dim">Set a status</span>
      {/if}
    </span>
    {#if open}<X size={13} class="shrink-0 text-muted" />{/if}
  </button>
{/if}
