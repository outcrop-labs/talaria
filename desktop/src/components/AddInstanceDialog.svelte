<script lang="ts">
  import { add, shell } from '../lib/state.svelte'

  let { onclose } = $props<{ onclose: () => void }>()

  let url = $state('')
  let submitting = $state(false)

  // Focus without the autofocus attribute (a11y): an action runs once, on mount.
  const focus = (el: HTMLElement) => el.focus()

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    if (!url.trim() || submitting) return
    submitting = true
    const ok = await add(url.trim())
    submitting = false
    if (ok) {
      url = ''
      onclose()
    }
  }
</script>

<div class="fixed inset-0 grid place-items-center bg-black/60 p-4" role="dialog" aria-modal="true">
  <form
    class="w-[28rem] max-w-full rounded-md border border-hairline-strong bg-panel p-5"
    onsubmit={submit}
  >
    <h2 class="text-sm font-semibold text-readout">Add a Talaria instance</h2>
    <label class="mt-4 block">
      <span class="mb-1 block text-xs text-muted">Instance URL</span>
      <input
        class="w-full rounded border border-hairline bg-raised px-3 py-2 font-mono text-[13px] text-readout placeholder:text-dim focus:border-hairline-strong focus:outline-none"
        type="text"
        placeholder="https://talaria.example.com"
        autocomplete="off"
        spellcheck="false"
        bind:value={url}
        use:focus
        disabled={submitting}
      />
    </label>
    <p class="mt-2 text-xs text-dim">
      Bare hosts are treated as https. The shell checks the instance beacon
      before adding — and signs in inside the instance itself.
    </p>
    {#if shell.error}
      <p class="mt-3 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger">
        {shell.error}
      </p>
    {/if}
    <div class="mt-5 flex justify-end gap-2">
      <button
        type="button"
        class="rounded px-3 py-1.5 text-xs text-muted hover:bg-hover hover:text-readout"
        onclick={onclose}
        disabled={submitting}
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
</div>
