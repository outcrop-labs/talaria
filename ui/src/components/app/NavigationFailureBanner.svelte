<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { navigateHref, route } from '@/router'
  import { slide } from '@/lib/motion'
  import { clearNavigationFailure, retryNavigation, useNavigationFailure } from '@/lib/nav-failure.svelte'

  // The recovery affordance for a navigation that silently died (see
  // nav-failure.svelte.ts for the mechanism). Mounted in the shell next to
  // UnreadableSecretsBanner — both are "the frame tells you when something
  // under it broke" — and rendered inside the real chrome so a dead sidebar
  // click never reads as a dead app.
  const failure = useNavigationFailure()

  // A LATER NAVIGATION THAT SUCCEEDS RETIRES THE NOTICE: the failure was about
  // getting somewhere, and the person got somewhere else instead. The failed
  // navigation itself never changes the path (the URL is the thing that did
  // not update), so this only fires on real arrivals.
  let seenPath: string | null = null
  $effect(() => {
    const at = route.pathname
    if (seenPath === null) {
      seenPath = at
      return
    }
    if (at !== seenPath) {
      seenPath = at
      clearNavigationFailure()
    }
  })

  let retrying = $state(false)
  async function retry(): Promise<void> {
    retrying = true
    try {
      await retryNavigation((href) => navigateHref(href))
    } finally {
      retrying = false
    }
  }
</script>


{#if failure.current}
  <div
    transition:slide={{ duration: 150 }}
    role="alert"
    class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-danger/40 bg-danger/5 px-4 py-2"
  >
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Page didn't open</span>
    <span class="min-w-0 flex-1 text-xs text-muted">
      Could not open {failure.current.href}. {failure.current.message}
    </span>
    <Button variant="ghost" size="xs" class="shrink-0 text-ink-dim" disabled={retrying} onclick={() => void retry()}>
      {retrying ? 'Trying…' : 'Try again'}
    </Button>
    <Button variant="ghost" size="xs" class="shrink-0 text-ink-dim" onclick={() => location.reload()}>
      Reload
    </Button>
  </div>
{/if}
