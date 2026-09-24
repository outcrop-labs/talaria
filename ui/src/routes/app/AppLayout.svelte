<script lang="ts">
  import type { Snippet } from 'svelte'
  import { searchParams } from 'sv-router'
  import { navigate, route } from '@/router'
  import Brand from '@/components/Brand.svelte'
  import WingMark from '@/components/WingMark.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import TopDock from '@/components/app/TopDock.svelte'
  import ManageSidebar from '@/components/app/ManageSidebar.svelte'
  import DesktopTitlebar from '@/components/app/DesktopTitlebar.svelte'
  import FileViewerHost from '@/components/app/FileViewerHost.svelte'
  import TimezoneAdopt from '@/components/app/TimezoneAdopt.svelte'
  import NotificationToasts from '@/components/app/NotificationToasts.svelte'
  import Toasts from '@/components/app/Toasts.svelte'
  import TopStrip from '@/components/app/TopStrip.svelte'
  import InboxFocusShell from '@/components/inbox/InboxFocusShell.svelte'
  import UnreadableSecretsBanner from '@/components/setup/UnreadableSecretsBanner.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import { useDeniedViews, useLogout, useSession } from '@/lib/session'
  import { upgradeDitherSurfaces } from '@/lib/dither-surface'
  import { ADMIN_VIEWS } from '@/lib/nav'
  import { useUserEventInvalidation } from '@/lib/user-events.svelte'
  import { assistantSurface, shouldAttachInboxDecision } from '@/lib/inbox-focus-surface'

  // Authenticated app shell (Mercury, spec §5–6): the dock is a full-width
  // band of nav tiles above everything; below it, the top strip sits above
  // the active view (children). The brand lives in the dock, not the strip.
  // The manage half of the menu lives in the sidebar the dock's gear opens
  // (ManageSidebar, a sibling below).
  let { children }: { children: Snippet } = $props()

  const session = useSession()
  const denied = useDeniedViews()
  const logout = useLogout()
  // THE FIREHOSE'S ONE MOUNT. Everything live that is not a page's own stream
  // rides this: the bell, the rails' badges, a run finishing off-page. One
  // EventSource per tab (see user-events.svelte), opened once from the shell.
  useUserEventInvalidation()

  const user = $derived(session.data)
  // sv-router auto-parses query values (numbers, bare flags) — the inbox
  // decision check compares tab names, so keep it a string.
  const rawTab = $derived(searchParams.get('tab'))
  const tab = $derived(rawTab == null ? undefined : String(rawTab))

  // LEGACY `?r=` RESEARCH LINKS STILL WORK, for the same reason as `?tab=`
  // below: the completion notification's href — and the button in the email
  // it may have been mailed as — pointed at `/research?r=<runId>` until the
  // href moved to the path (`/research/<runId>`), and sent emails are not
  // ours to fix. Without this they resolve to the bare research view with
  // nothing selected: the quiet wrong answer. Indexed-doc payloads in Qdrant
  // carry the old shape too, until the next reindex regenerates them.
  $effect(() => {
    if (route.pathname !== '/research') return
    const r = searchParams.get('r')
    if (r == null || r === true || r === '') return
    void navigate('/research/:runId', { params: { runId: String(r) }, replace: true })
  })

  // LEGACY `?tab=` LINKS STILL WORK.
  //
  // Tabs became path segments (/admin/security), but URLs with the old shape
  // are already out in the world and some of them are not ours to fix:
  // `NOTIFY_SETTINGS_PATH` is embedded in notification EMAILS that have already
  // been sent. Without this they would still resolve — to the view's default
  // tab — which is the quiet wrong answer rather than a visible failure.
  //
  // Written as literal calls rather than a computed path because the router is
  // typed on its route strings; seven explicit cases is the price of that, and
  // it also means a base that stops being tabbed fails to compile here.
  $effect(() => {
    const raw = searchParams.get('tab')
    if (raw == null || raw === true) return
    const t = String(raw)
    const at = route.pathname
    const opts = { params: { tab: t }, replace: true } as const
    if (at === '/settings') void navigate('/settings/:tab', opts)
    else if (at === '/admin') void navigate('/admin/:tab', opts)
    else if (at === '/observability') void navigate('/observability/:tab', opts)
    else if (at === '/models') void navigate('/models/:tab', opts)
    else if (at === '/templates') void navigate('/templates/:tab', opts)
    else if (at === '/agents') void navigate('/agents/:tab', opts)
    else if (at === '/mcp') void navigate('/mcp/:tab', opts)
    // `/` is deliberately absent: Home still keeps its tab in the query.
  })

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

  // THE AUTH GATE PAINTS NOTHING IT MIGHT TAKE BACK. While the session read is
  // in flight — and in the beat between "resolved: nobody" and the /login
  // navigation landing — the screen is the login surface's own ground with the
  // mark centered (sessionHold below). The app chrome used to paint here too
  // (skeleton dock + strip + cards): for a signed-in reload that read as
  // "frame first, content fills in", but for every signed-out visitor it was
  // the dashboard flashing for a moment before the login screen slammed in.
  // One session round-trip of quiet mark is the price both pay — and since the
  // holding ground, the login card, and the app chrome all sit on the same
  // Mercury ground, nothing blanks and nothing BAMs: content arrives on ground
  // that never changed.
  //
  // The ERROR case keeps the real chrome (shellSkeleton below): the read broke,
  // not the session — the person is probably signed in, so they get the frame
  // with a retry in it, not a bounce that reads as "you have been signed out".
  //
  // The dock shell has no collapsed variant to hydrate around (the rail's
  // docked choice, in nav-dock.svelte, belongs to the shell swap's second
  // half and rides the same module-state discipline as the rail's did).

  // EVERY MARKED CONTROL GETS ITS FIELD FROM ONE PLACE. The `dither-*` classes
  // stay in the markup as the statement of intent — 127 call sites already
  // make it correctly — and this is what honours them. Doing it here rather
  // than at each call site is what keeps one rendering rather than two.
  let shell = $state<HTMLElement | null>(null)
  $effect(() => {
    const el = shell
    if (!el) return
    return upgradeDitherSurfaces(el)
  })
</script>

{#snippet sessionHold()}
  <!-- The gate's holding frame: the ground every auth-adjacent surface sits
       on, the brand mark centered where the login card is about to be. Still,
       not breathing — Mercury is matte, the wait is one round-trip, and motion
       here would perform rather than confirm. -->
  <MercuryBackdrop />
  <div class="flex h-full items-center justify-center">
    <Brand size={40} class="opacity-70" />
  </div>
{/snippet}

{#snippet shellSkeleton(content: Snippet | undefined)}
  <MercuryBackdrop />
  <div class="flex h-full flex-col">
    <!-- Dock-shaped skeleton: the band of tiles above the strip, exactly the
         rows the real chrome paints, so loading never reflows. h-9 tiles and
         the h-14 band match the real dock's geometry; the strip row keeps
         its line box (the strip is the slim personal row now). -->
    <nav
      aria-label="Primary"
      class="flex h-14 shrink-0 items-center gap-2 border-b border-line bg-sidebar px-2"
    >
      <div class="grid h-9 w-9 shrink-0 place-items-center" aria-label="Talaria">
        <WingMark class="h-5 w-5" />
      </div>
      {#each [0, 1, 2, 3, 4, 5] as i (i)}
        <Skeleton class="h-9 w-9 rounded-md" />
      {/each}
      <div class="min-w-0 flex-1"></div>
    </nav>
    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <!-- The strip: one slim row now — the title row is gone, so the skeleton
           holds only its line box (py-2 + h-7). -->
      <header class="flex shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4 py-2">
        <div class="w-40"></div>
        {#if content}<ThemeToggle />{:else}<Skeleton class="h-5 w-40 rounded-full" />{/if}
      </header>
      <div class="min-h-0 min-w-0 flex-1 overflow-hidden p-8">
        {#if content}
          {@render content()}
        {:else}
          <div class="mx-auto w-full max-w-[var(--page-width)]">
            <div class="grid gap-4 xl:grid-cols-3">
              {#each [0, 1, 2] as i (i)}
                <div class="rounded-lg border border-line bg-panel p-6">
                  <Skeleton class="mb-4 h-3 w-24 rounded-full" />
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

<div class="flex h-screen flex-col">
  <DesktopTitlebar />
  <div class="min-h-0 flex-1 overflow-hidden">
{#if session.isError && !user}
  <!-- The session read FAILED (not "signed out" — that is a 200 with a null user).
       Say so inside the real chrome and offer a retry. Shimmering forever would be
       the same lie in slower motion, and bouncing to /login — what a swallowed 500
       used to do — is worse still: it reads as "you have been signed out". -->
  {@render shellSkeleton(sessionFailed)}
{:else if session.isLoading || !user}
  {@render sessionHold()}
{:else}
  <MercuryBackdrop />
  <div bind:this={shell} class="relative flex h-full flex-col">
    <!-- THE DOCK SPANS THE FULL WIDTH ABOVE EVERYTHING — the one layout
         semantics change of the swap. The rail was a peer of the assistant
         drawer; the dock is a band ABOVE it, because horizontal nav and a
         left drawer are different kinds of furniture: the drawer must still
         span the viewport's height to be a peer surface, and a dock band
         above it reads as the shell's top edge rather than a notch cut out
         of the drawer. The strip stays inside the drawer's column, still
         titling the view, and everything below keeps its height chain. -->
    <TopDock {user} />
    <div class="relative flex min-h-0 min-w-0 flex-1 overflow-hidden">
      <ManageSidebar />
      <!-- THE ASSISTANT DRAWER IS A PEER OF THE DOCK'S ROW, not of the page
           body. It used to open inside `vt-view`, below the top strip and
           the banner, so a panel that is conceptually a second rail started
           a strip's height down the screen and left a notch beside the nav.
           Out here it spans the below-dock row, and the strip belongs to
           the view it titles. It also stops being animated by the view
           transition on every nav click, which it never should have been —
           the drawer stays put while the page swaps. -->
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
        <!-- Silent first-run timezone adoption. Renders nothing; see the
             component. Here, next to the banner, because both are "the shell
             quietly makes the workspace honest" — one about secrets, one about
             whose clock the person is on. -->
        <TimezoneAdopt />
        <!-- This region used to carry `vt-view` and be animated by the View
             Transitions API on nav clicks. Removed: the transition paints a
             static snapshot while the incoming view is still loading its data,
             so the live DOM appeared in one jump when the animation ended. See
             the note in styles.css. Content entrance is animated by Materialize
             and listStagger instead, which fire when the data actually lands. -->
        <div class="min-h-0 min-w-0 flex-1 overflow-hidden">
          {@render children()}
        </div>
      </InboxFocusShell>
    </div>
    <!-- Tell-me-now lives in the shell, not in any view: the watcher turns
         live notifications into toasts on every surface, and adds an OS
         notification only when Talaria is not what the person is looking
         at. The host renders the stack; the watcher renders nothing. -->
    <NotificationToasts />
    <Toasts />
  </div>
{/if}
  </div>
  <FileViewerHost />
</div>
