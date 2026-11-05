<script lang="ts">
  import {
    closeBrowserTab,
    desktopWindow,
    errorText,
    hostOf,
    openBrowserTab,
    setBrowserTabPinned,
    setBrowseMode,
  } from '../lib/instances'
  import { shell } from '../lib/state.svelte'

  // The browse bar (TALA-83): the strip of shell chrome above a browser tab
  // (layout::BAR, 44 logical px). Same document as the launcher — this is
  // what the `browsebar` webview renders; App.svelte decides by the shell's
  // view — so it shares the launcher's tokens and typography (app.css)
  // rather than carrying its own. Launcher doctrine applies: Rust owns
  // state, this mirrors it; nothing is read once and trusted.
  //
  // The tab's webview has ZERO shell IPC (the plan's locked posture), so the
  // bar is the whole UI a third-party page gets: title, pin, close, address,
  // and the way back. The strip is also the top of an undecorated window:
  // empty space drags it, a double-click maximizes (the Titlebar idiom).
  const current = $derived(shell.browserTabs.find((t) => t.id === shell.activeTab) ?? null)

  let address = $state('')
  let opening = $state(false)
  let error = $state('')

  const drag = (e: MouseEvent) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement | null)?.closest('button, input, form')) return
    void desktopWindow('startDragging')
  }

  function open(e: SubmitEvent) {
    e.preventDefault()
    const url = address.trim()
    if (!url || opening) return
    opening = true
    error = ''
    openBrowserTab(url)
      .then(() => (address = ''))
      .catch((err: unknown) => (error = errorText(err)))
      .finally(() => (opening = false))
  }

  function pin() {
    if (!current) return
    setBrowserTabPinned(current.id, !current.pinned).catch((err: unknown) => (error = errorText(err)))
  }

  function close() {
    if (!current) return
    closeBrowserTab(current.id).catch((err: unknown) => (error = errorText(err)))
  }

  function back() {
    setBrowseMode(false).catch((err: unknown) => (error = errorText(err)))
  }
</script>

<div
  class="flex h-11 shrink-0 items-center gap-2 border-b border-hairline bg-panel px-2 select-none"
  onmousedown={drag}
  ondblclick={() => void desktopWindow('toggleMaximize')}
  role="presentation"
>
  <!-- The way back: the launcher, or the instance the person came from. -->
  <button
    type="button"
    title="Back to Talaria"
    aria-label="Back to Talaria"
    class="grid h-8 w-8 shrink-0 place-items-center rounded-sm text-muted hover:bg-raised hover:text-readout"
    onclick={back}
  >
    ←
  </button>

  <!-- The current tab: the page's own title, or its host until it says one. -->
  {#if current}
    <span class="flex min-w-0 shrink-0 items-center gap-1.5 rounded-sm bg-raised px-2 py-1.5">
      <span class="truncate font-mono text-xs text-readout">
        {current.title || hostOf(current.url)}
      </span>
      <button
        type="button"
        title={current.pinned ? 'Unpin tab' : 'Pin tab'}
        aria-label={current.pinned ? 'Unpin tab' : 'Pin tab'}
        class="shrink-0 rounded-sm p-0.5 text-muted hover:text-readout"
        onclick={pin}
      >
        {#if current.pinned}
          ◉
        {:else}
          ◎
        {/if}
      </button>
      <button
        type="button"
        title="Close tab"
        aria-label="Close tab"
        class="shrink-0 rounded-sm p-0.5 text-muted hover:text-danger"
        onclick={close}
      >
    ×
      </button>
    </span>
  {:else}
    <span class="shrink-0 font-mono text-xs text-muted">No tab open</span>
  {/if}

  <!-- The address: Enter opens a new tab. A bad URL is the shell's own
       sentence (registry::tab_normalize_url), shown at the right edge. -->
  <form onsubmit={open} class="flex min-w-0 flex-1 items-center justify-center gap-1.5">
    <input
      type="text"
      bind:value={address}
      placeholder="Open a web page"
      aria-label="Open a web page"
      spellcheck="false"
      class="h-8 w-full max-w-lg rounded-md border border-hairline bg-ground px-3 font-mono text-xs text-readout placeholder:text-dim outline-none transition-colors focus:border-gold"
    />
    <button
      type="submit"
      title="Open a new tab"
      aria-label="Open a new tab"
      disabled={opening || !address.trim()}
      class="grid h-8 w-8 shrink-0 place-items-center rounded-sm text-muted hover:bg-raised hover:text-readout"
    >
    +
    </button>
  </form>

  {#if error}
    <span class="shrink-0 font-sans text-xs text-danger">{error}</span>
  {/if}
</div>
