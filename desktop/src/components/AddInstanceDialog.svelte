<script lang="ts">
  import { addInstance, cancelAddInstance, CANCELLED, errorText } from '../lib/instances'
  import { open } from '../lib/state.svelte'
  import Modal from './Modal.svelte'

  // The error lives HERE, not in launcher state: it belongs to this attempt,
  // and it goes when the dialog does. (It used to be global — shown twice,
  // kept on the welcome screen after Cancel, and back in the next dialog.)
  let { onclose } = $props<{ onclose: () => void }>()

  let url = $state('')
  let submitting = $state(false)
  let error = $state('')
  let closed = false

  function close() {
    if (closed) return
    closed = true
    // Cancel works mid-probe: Rust aborts the probe and the add never lands.
    if (submitting) void cancelAddInstance()
    onclose()
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    if (!url.trim() || submitting) return
    submitting = true
    error = ''
    try {
      const instance = await addInstance(url.trim())
      if (closed) return
      // Close first, open after the launcher has painted without the dialog:
      // the frame it shows when it comes back is the last one it painted.
      closed = true
      onclose()
      void open(instance.id, { fresh: true, settle: true })
    } catch (err) {
      const text = errorText(err)
      if (!closed && text !== CANCELLED) error = text
    } finally {
      submitting = false
    }
  }
</script>

<Modal label="Add a Talaria instance" onescape={close} class="w-[28rem] max-w-full">
  <form class="rounded-md border border-hairline-strong bg-panel p-5" onsubmit={submit}>
    <h2 class="text-sm font-semibold text-readout">Add a Talaria instance</h2>
    <label class="mt-4 block">
      <span class="mb-1 block text-xs text-muted">Instance URL</span>
      <input
        class="w-full rounded border border-hairline bg-raised px-3 py-2 font-mono text-[13px] text-readout placeholder:text-dim read-only:text-muted focus:border-hairline-strong focus:outline-none"
        type="text"
        placeholder="https://talaria.example.com"
        autocomplete="off"
        spellcheck="false"
        bind:value={url}
        data-autofocus
        readonly={submitting}
        aria-busy={submitting}
        oninput={() => (error = '')}
      />
    </label>
    <p class="mt-2 text-xs text-dim">
      Bare hosts are treated as https. The shell checks the instance beacon
      before adding — and signs in inside the instance itself.
    </p>
    {#if error}
      <p class="mt-3 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger" role="alert">
        {error}
      </p>
    {/if}
    <div class="mt-5 flex justify-end gap-2">
      <button
        type="button"
        class="rounded px-3 py-1.5 text-xs text-muted hover:bg-hover hover:text-readout"
        onclick={close}
      >
        Cancel
      </button>
      <button
        type="submit"
        class="rounded bg-gold px-4 py-1.5 text-xs font-semibold text-ground hover:brightness-110 disabled:opacity-50"
        disabled={submitting || !url.trim()}
      >
        {submitting ? 'Checking…' : 'Connect'}
      </button>
    </div>
  </form>
</Modal>
