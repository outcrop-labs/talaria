<script lang="ts">
  import type { DitherSource } from '@dither'
  import {
    checkForUpdate,
    errorText,
    installUpdate,
    type TitlebarMode,
    type DesktopUpdate,
  } from '../lib/instances'
  import DitherField from './DitherField.svelte'
  import InstanceList from './InstanceList.svelte'
  import WingMark from './WingMark.svelte'
  import { cancelOpen, dismissNotice, open, setTitlebar, shell } from '../lib/state.svelte'

  // The signature ambient field, wearing the same shape the nav rail wears in
  // the product: a soft bloom off the top edge, a heavier foot at the bottom,
  // a whisper of grain everywhere. Static here — DitherField says why.
  const ambient: DitherSource[] = [
    { id: 'chrome', kind: 'edge', side: 'top', depth: 96, strength: 0.16 },
    { id: 'grain', kind: 'uniform', strength: 0.04 },
    { id: 'foot', kind: 'edge', side: 'bottom', depth: 128, strength: 0.1 },
  ]

  const modes: { id: TitlebarMode; label: string }[] = [
    { id: 'themed', label: 'Themed' },
    { id: 'os', label: 'OS' },
    { id: 'none', label: 'None' },
  ]

  const onadd = () => (shell.adding = true)

  let update = $state<DesktopUpdate | null>(null)
  let updateBusy = $state(false)
  let updateMsg = $state('')

  const check = async () => {
    updateBusy = true
    updateMsg = ''
    update = null
    try {
      const found = await checkForUpdate()
      if (found) update = found
      else updateMsg = 'You are on the latest version'
    } catch (e) {
      updateMsg = errorText(e)
    } finally {
      updateBusy = false
    }
  }

  const install = async () => {
    updateBusy = true
    updateMsg = ''
    try {
      await installUpdate()
    } catch (e) {
      updateMsg = errorText(e)
      updateBusy = false
    }
  }
</script>

<!-- The field sits BEHIND the scroller, not in it, so its bloom and foot stay
     on the window's edges when a long instance list scrolls (inside, it
     covered only the first screen of the scroll area). -->
<div class="relative h-full w-full bg-ground">
  <DitherField sources={ambient} organic={0.5} alphaFloor={0.04} maxAlpha={0.16} />
  <main class="relative grid h-full w-full place-items-center overflow-y-auto">
    <div class="relative z-10 w-[24rem] max-w-full p-6 text-center">
      <WingMark class="mx-auto h-20 w-20" />
      <h1 class="mt-5 font-display text-sm font-semibold tracking-[0.18em] text-readout">TALARIA</h1>

      {#each shell.notices as notice (notice)}
        <div class="mt-4 flex items-start gap-2 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-left text-xs text-danger" role="alert">
          <p class="min-w-0 flex-1 break-words">{notice}</p>
          <button
            type="button"
            class="shrink-0 rounded px-1 text-muted hover:text-readout"
            aria-label="Dismiss"
            onclick={() => dismissNotice(notice)}>×</button
          >
        </div>
      {/each}

      {#if shell.instances.length > 0}
        <p class="mt-2 text-xs text-muted">Switch below, or add another instance.</p>
        <div class="mt-4 overflow-hidden rounded-md border border-hairline bg-panel text-left">
          <InstanceList />
        </div>
        <button
          class="mt-4 w-full rounded bg-gold px-5 py-2 text-xs font-semibold text-ground hover:brightness-110"
          onclick={onadd}
        >
          Add instance
        </button>
      {:else if shell.ready}
        <p class="mt-2 text-xs text-muted">
          Your instances, one window. Add one to get started — each keeps its own
          signed-in session, and switching is a click.
        </p>
        <button
          class="mt-6 rounded bg-gold px-5 py-2 text-xs font-semibold text-ground hover:brightness-110"
          onclick={onadd}
        >
          Add instance
        </button>
      {:else}
        <p class="mt-6 text-xs text-dim">…</p>
      {/if}

      {#if shell.opening}
        <div class="mt-4 flex items-center justify-between gap-3 rounded border border-hairline bg-panel px-3 py-2 text-left text-xs text-muted" role="status">
          <span class="min-w-0 truncate">Opening {shell.opening.label}…</span>
          <button type="button" class="shrink-0 rounded px-2 py-0.5 text-muted hover:bg-hover hover:text-readout" onclick={cancelOpen}>
            Stay here
          </button>
        </div>
      {/if}

      {#if shell.problem}
        {@const problem = shell.problem}
        <div class="mt-4 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-left text-xs" role="alert">
          <p class="text-danger break-words">{problem.message}</p>
          <div class="mt-2 flex justify-end gap-2">
            <button type="button" class="rounded px-2 py-0.5 text-muted hover:bg-hover hover:text-readout" onclick={() => (shell.problem = null)}>
              Dismiss
            </button>
            {#if problem.probe}
              <!-- The check is not the page: an instance behind a sign-in gate,
                   or reachable only through a proxy the webview knows, can
                   fail it and still load. On Linux a page that really does
                   not load comes back here as a load failure. -->
              <button
                type="button"
                class="rounded px-2 py-0.5 text-muted hover:bg-hover hover:text-readout"
                onclick={() => void open(problem.id, { fresh: true, settle: true })}
              >
                Open anyway
              </button>
            {/if}
            <button type="button" class="rounded border border-hairline-strong px-2 py-0.5 text-readout hover:bg-hover" onclick={() => void open(problem.id)}>
              Retry
            </button>
          </div>
        </div>
      {/if}

      {#if shell.error}
        <p class="mt-4 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-left text-xs text-danger">
          {shell.error}
        </p>
      {/if}

      <div class="mt-8">
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Titlebar</div>
        <div class="inline-flex rounded-md border border-hairline p-0.5">
          {#each modes as m (m.id)}
            <button
              type="button"
              aria-pressed={shell.titlebar === m.id}
              class="rounded px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.05em] {shell.titlebar === m.id
                ? 'bg-raised text-readout'
                : 'text-muted hover:text-readout'}"
              onclick={() => void setTitlebar(m.id)}
            >
              {m.label}
            </button>
          {/each}
        </div>
      </div>

      <div class="mt-6">
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Updates</div>
        {#if update}
          <button
            type="button"
            class="rounded bg-gold px-5 py-2 text-xs font-semibold text-ground hover:brightness-110 disabled:opacity-50"
            disabled={updateBusy}
            onclick={() => void install()}
          >
            {updateBusy ? 'Installing' : `Install ${update.version}`}
          </button>
        {:else}
          <button
            type="button"
            class="rounded border border-hairline px-5 py-2 text-xs text-muted hover:text-readout disabled:opacity-50"
            disabled={updateBusy}
            onclick={() => void check()}
          >
            {updateBusy ? 'Checking' : 'Check for updates'}
          </button>
        {/if}
        {#if updateMsg}
          <p class="mt-2 text-xs text-muted">{updateMsg}</p>
        {/if}
      </div>
    </div>
  </main>
</div>
