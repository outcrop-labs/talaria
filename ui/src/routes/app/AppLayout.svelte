<script lang="ts">
  import type { Snippet } from 'svelte'
  import { searchParams } from 'sv-router'
  import { navigate, route } from '@/router'
  import Brand from '@/components/Brand.svelte'
  import WingMark from '@/components/WingMark.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import NavRail from '@/components/app/NavRail.svelte'
  import { useNavCollapsed } from '@/components/app/nav-rail.svelte'
  import TopStrip from '@/components/app/TopStrip.svelte'
  import InboxFocusShell from '@/components/inbox/InboxFocusShell.svelte'
  import UnreadableSecretsBanner from '@/components/setup/UnreadableSecretsBanner.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import { useDeniedViews, useLogout, useSession } from '@/lib/session'
  import { ADMIN_VIEWS } from '@/lib/nav'
  import { assistantSurface, shouldAttachInboxDecision } from '@/lib/inbox-focus-surface'

  // Authenticated app shell (Mercury, spec §5–6): the collapsible nav rail
  // spans the full height on the left; the top strip sits above the active view
  // (children) on the right. The brand lives in the rail, not the strip.
  let { children }: { children: Snippet } = $props()

  const session = useSession()
  const denied = useDeniedViews()
  const logout = useLogout()

  const user = $derived(session.data)
  // sv-router auto-parses query values (numbers, bare flags) — the inbox
  // decision check compares tab names, so keep it a string.
  const rawTab = $derived(searchParams.get('tab'))
  const tab = $derived(rawTab == null ? undefined : String(rawTab))

  // Only a SUCCESSFUL session read saying "nobody is signed in" sends anyone to
  // /login. /api/auth/session answers 200 with `{ user: null }` when you're
  // signed out, so a non-2xx means the backend blipped — and `isSuccess` is
  // false for it now, which is the whole point: a blip is not a logout.
  $effect(() => {
    if (session.isSuccess && !session.data) void navigate('/login')
  })

  // Native context menus are suppressed app-wide — Talaria surfaces provide
  // their own. Editable fields keep the native menu (paste, spellcheck,
  // dictionary) — taking that away breaks real workflows.
  $effect(() => {
    const onCtx = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null
      if (t?.closest('input, textarea, [contenteditable="true"], [contenteditable=""]')) return
      e.preventDefault()
    }
    document.addEventListener('contextmenu', onCtx)
    return () => document.removeEventListener('contextmenu', onCtx)
  })

  // Route gate: a denied or role-gated view isn't just hidden from the nav —
  // reaching it by URL bounces to Home. (Match prefixes, e.g. /boards/x.)
  $effect(() => {
    const u = session.data
    if (!u) return
    const blocked = u.role === 'admin' ? denied.current : [...denied.current, ...ADMIN_VIEWS]
    const pathname = route.pathname
    if (blocked.some((v) => pathname === v || pathname.startsWith(v + '/'))) {
      void navigate('/')
    }
  })

  // The session gate used to blank the WHOLE app ("no content, then BAM").
  // Render the real frame immediately — rail placeholder (honoring the persisted
  // collapse state so the frame never jumps), top strip, a skeleton page — and
  // only content fills in.
  //
  // `content` swaps the skeleton page body for a real message: the session read
  // broke. Same frame either way, so "still loading" and "this failed" are the
  // same shape and only the words differ — and the theme toggle stays reachable
  // because that frame is the whole app until the session comes back.
  //
  // Hydration-safe: server snapshot is "expanded"; the persisted client value
  // swaps in right after hydration (same store NavRail uses, so no jump).
  const nav = useNavCollapsed()
</script>

{#snippet shellSkeleton(content: Snippet | undefined)}
  <MercuryBackdrop />
  <div class="flex h-screen">
    {#if nav.collapsed}
      <nav class="flex h-full w-16 shrink-0 flex-col items-center gap-3 border-r border-line bg-sidebar pb-5 pt-3">
        <div class="grid h-9 w-9 place-items-center">
          <WingMark class="h-5 w-5" />
        </div>
        {#each [0, 1, 2, 3, 4, 5] as i (i)}
          <Skeleton class="h-9 w-9 rounded-md" delay={i * 0.08} />
        {/each}
      </nav>
    {:else}
      <nav class="flex h-full w-[208px] shrink-0 flex-col gap-5 border-r border-line bg-sidebar px-3 pb-4 pt-5">
        <div class="flex h-6 items-center">
          <Brand />
        </div>
        {#each [0, 1, 2] as g (g)}
          <div class="space-y-2 px-2">
            <Skeleton class="h-2 w-14 rounded-full" delay={g * 0.1} />
            <SkeletonRows rows={4} />
          </div>
        {/each}
      </nav>
    {/if}
    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <header class="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4">
        <Skeleton class="h-2.5 w-56 rounded-full" />
        {#if content}<ThemeToggle />{:else}<Skeleton class="h-5 w-40 rounded-full" delay={0.15} />{/if}
      </header>
      <div class="min-h-0 min-w-0 flex-1 overflow-hidden p-8">
        {#if content}
          {@render content()}
        {:else}
          <div class="mx-auto max-w-6xl space-y-6">
            <Skeleton class="h-6 w-64 rounded-full" />
            <div class="grid gap-4 xl:grid-cols-3">
              {#each [0, 1, 2] as i (i)}
                <div class="rounded-lg border border-line bg-panel p-6">
                  <Skeleton class="mb-4 h-3 w-24 rounded-full" delay={i * 0.1} />
                  <SkeletonRows rows={4} />
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/snippet}

{#snippet sessionFailed()}
  <QueryError
    error={session.error}
    title="Could not reach your session"
    onRetry={() => void session.refetch()}
  />
{/snippet}

{#if session.isError && !user}
  <!-- The session read FAILED (not "signed out" — that is a 200 with a null user).
       Say so inside the real chrome and offer a retry. Shimmering forever would be
       the same lie in slower motion, and bouncing to /login — what a swallowed 500
       used to do — is worse still: it reads as "you have been signed out". -->
  {@render shellSkeleton(sessionFailed)}
{:else if session.isLoading || !user}
  {@render shellSkeleton(undefined)}
{:else}
  <MercuryBackdrop />
  <div class="flex h-screen">
    <NavRail {user} />
    <!-- THE ASSISTANT DRAWER IS A PEER OF THE NAV RAIL, not of the page body.
         It used to open inside `vt-view`, below the top strip and the banner,
         so a panel that is conceptually a second rail started a strip's height
         down the screen and left a notch beside the nav. Out here it spans the
         viewport, and the strip belongs to the view it titles. It also stops
         being animated by the view transition on every nav click, which it
         never should have been — the drawer stays put while the page swaps. -->
    <InboxFocusShell
      attachActiveDecision={shouldAttachInboxDecision(route.pathname, tab)}
      surface={assistantSurface(route.pathname, tab)}
    >
      <TopStrip {user} onLogout={() => void logout()} />
      <!-- Above the content, below the strip: unreadable secrets fail at USE
           time, so without a standing signal an admin learns about it from a
           confused colleague days later. Renders nothing for members, and
           nothing at all when there is nothing to say. -->
      <UnreadableSecretsBanner />
      <!-- vt-view: the View Transitions API animates ONLY this region on nav
           clicks (styles.css) — the rail, drawer and strip stay planted. -->
      <div class="vt-view min-h-0 min-w-0 flex-1 overflow-hidden">
        {@render children()}
      </div>
    </InboxFocusShell>
  </div>
{/if}
