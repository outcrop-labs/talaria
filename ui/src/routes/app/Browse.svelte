<script lang="ts">
  import { onMount } from 'svelte'
  import { Pin, PinOff, Plus, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { inDesktopShell, type BrowserTab } from '@/lib/desktop-shell'
  import {
    shellCloseBrowserTab,
    shellFocusBrowserTab,
    shellListBrowserTabs,
    shellOpenBrowserTab,
    shellSetBrowserTabPinned,
    shellSetBrowseMode,
  } from '@/lib/desktop-shell'
  import { assertPinnedFirst, browseFallback, displayTitle, tabGroups } from '@/lib/browse-tabs'

  // P3: the honest fallback a browser gets — derived in browse-tabs.ts and
  // only rendered here. In the shell it is null and this branch never runs.

  // The Browse view (TALA-83): tabs and pinned web apps, rendered by the
  // DESKTOP SHELL as native webviews — never iframes (the research verdict:
  // X-Frame-Options / CSP frame-ancestors make in-browser embedding of
  // third-party sites a dead end). This page is Talaria's own control
  // surface: the tab strip that drives the shell's browtab-* webviews.
  //
  // A browser gets the placeholder below until P3's honest fallback ships;
  // in the shell it renders the strip. The shell's Browse BAR (the strip of
  // chrome above a tab) is a separate surface in desktop/src — this page is
  // the /browse route.

  const shell = inDesktopShell()
  let tabs = $state<BrowserTab[]>([])
  let openUrl = $state('')
  let working = $state(false)
  let error = $state<string | null>(null)

  onMount(() => {
    if (!shell) return
    void shellListBrowserTabs().then((list) => (tabs = assertPinnedFirst(list)))
  })

  const groups = $derived(tabGroups(tabs))
  const strip = $derived([...groups.pinned, ...groups.unpinned])

  function refresh(): void {
    void shellListBrowserTabs().then((list) => (tabs = assertPinnedFirst(list)))
  }

  function open(e: SubmitEvent): void {
    e.preventDefault()
    if (!openUrl.trim() || working) return
    working = true
    error = null
    void shellOpenBrowserTab(openUrl.trim())
      .then((tab) => {
        if (tab) {
          openUrl = ''
          refresh()
        } else {
          error = 'The tab could not be opened'
        }
      })
      .finally(() => (working = false))
  }

  function pin(tab: BrowserTab): void {
    void shellSetBrowserTabPinned(tab.id, !tab.pinned).then(refresh)
  }

  function close(tab: BrowserTab): void {
    void shellCloseBrowserTab(tab.id).then(refresh)
  }

  function focus(tab: BrowserTab): void {
    void shellFocusBrowserTab(tab.id)
  }

  function leave(): void {
    void shellSetBrowseMode(false)
  }

  const fallback = browseFallback(shell)
</script>

{#if fallback}
  <!-- The honest fallback (P3): a browser cannot render native webviews, and
       pretending otherwise is worse than saying so. The hook attribute is the
       stable QA contract (TALA-4's convention): "visible" once this real
       explainer is up, never "pending" again. -->
  <div
    data-browse-fallback={fallback.hook}
    class="mx-auto max-w-xl px-8 py-16 font-sans"
  >
    <div class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Browse</div>
    <h1 class="mt-2 text-lg font-medium text-fg">Browse lives in the desktop app</h1>
    <p class="mt-3 text-sm text-muted">{fallback.why}</p>
    <a
      href={fallback.linkUrl}
      class="mt-5 inline-flex items-center gap-2 rounded-md border border-line bg-panel px-3 py-1.5 text-sm text-fg transition-colors hover:bg-raised"
      rel="noreferrer"
    >
      {fallback.linkLabel}
    </a>
  </div>
{:else}
  <div class="flex h-full min-h-0 flex-col">
    <form onsubmit={open} class="flex items-center gap-2 border-b border-line p-3">
      <Input
        placeholder="Open a web page"
        bind:value={openUrl}
        class="max-w-md"
        aria-label="Open a web page"
      />
      <Button size="sm" type="submit" disabled={working || !openUrl.trim()}>
        <Plus size={14} /> Open
      </Button>
      {#if error}
        <span class="font-sans text-xs text-muted">{error}</span>
      {/if}
    </form>
    {#if strip.length === 0}
      <div class="p-8 font-sans text-sm text-muted">No tabs yet.</div>
    {:else}
      <ul class="min-h-0 flex-1 overflow-y-auto">
        {#each strip as tab (tab.id)}
          {@const pinned = tab.pinned}
          <li
            class="flex items-center gap-3 border-b border-line-subtle px-4 py-2 font-sans text-sm"
          >
            <button
              type="button"
              onclick={() => focus(tab)}
              class="min-w-0 flex-1 truncate text-left text-fg hover:underline"
              title={tab.url}
            >
              {displayTitle(tab)}
            </button>
            <span class="shrink-0 text-xs text-muted">{tab.url}</span>
            <Button variant="ghost" size="sm" onclick={() => pin(tab)} title={pinned ? 'Unpin' : 'Pin'}>
              {#if pinned}
                <PinOff size={14} />
              {:else}
                <Pin size={14} />
              {/if}
            </Button>
            <Button variant="ghost" size="sm" onclick={() => close(tab)} title="Close">
              <X size={14} />
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
    <div class="border-t border-line p-3">
      <Button variant="link" size="sm" onclick={leave}>Back to Talaria</Button>
    </div>
  </div>
{/if}
