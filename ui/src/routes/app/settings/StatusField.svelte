<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import type { SessionUser } from '@/lib/session'

  // Status (R13): an emoji plus a short line ("📅 In a meeting") shown beside
  // the person's name in the Comms sidebar and DM header. No expiry; clearing
  // is manual. Bounds mirror the server (KTD7): text ≤ 100 chars.
  let { user }: { user: SessionUser | null | undefined } = $props()

  const MAX_TEXT = 100
  const qc = useQueryClient()
  const savedFlash = useSavedFlash()
  let emoji = $state<string | null>(null)
  let text = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const savedEmoji = $derived(user?.statusEmoji ?? null)
  const savedText = $derived(user?.statusText ?? '')
  $effect(() => {
    emoji = savedEmoji
    text = savedText
  })

  const dirty = $derived(emoji !== savedEmoji || text.trim() !== savedText)
  const hasStatus = $derived(!!savedEmoji || !!savedText)

  const send = async (body: { statusEmoji: string | null; statusText: string | null }) => {
    error = null
    busy = true
    try {
      await putJson<{ ok: true }>('/api/me', body)
      await qc.invalidateQueries({ queryKey: ['session'] })
      await qc.invalidateQueries({ queryKey: ['users'] })
      savedFlash.flash()
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
  const clear = () => {
    emoji = null
    text = ''
    void send({ statusEmoji: null, statusText: null })
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <label for="settings-status-text" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Status</label>
  <div class="flex max-w-md items-center gap-2">
    <EmojiPicker onPick={(e) => (emoji = e)} onClear={() => (emoji = null)}>
      {#snippet trigger()}
        <button
          type="button"
          class="grid h-11 w-11 shrink-0 place-items-center rounded-md border border-line-strong text-lg leading-none transition-colors dither-fill"
          title="Pick a status emoji"
          aria-label="Pick a status emoji"
        >
          {#if emoji}{emoji}{:else}<span class="text-muted">☺</span>{/if}
        </button>
      {/snippet}
    </EmojiPicker>
    <Input
      id="settings-status-text"
      bind:value={text}
      maxlength={MAX_TEXT}
      onkeydown={(e) => e.key === 'Enter' && save()}
      placeholder="What are you up to"
    />
    <Button onclick={save} disabled={busy || !dirty}>Save</Button>
  </div>
  <div class="mt-2 flex items-center gap-3">
    {#if error}
      <p class="text-xs text-danger" role="alert">{error}</p>
    {:else if savedFlash.saved}
      <span class="text-xs text-success">Saved</span>
    {/if}
    {#if hasStatus}
      <Button variant="link" size="xs" disabled={busy} onclick={clear}>Clear status</Button>
    {/if}
  </div>
</div>
