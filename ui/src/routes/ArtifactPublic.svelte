<script lang="ts">
  import { route } from '@/router'
  import { buttonClasses } from '@/components/ui/button'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, getJson, HttpError } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import PublicShell from '@/components/kb/PublicShell.svelte'
  import PublicNotFound from '@/components/kb/PublicNotFound.svelte'
  import PublicUnavailable from '@/components/kb/PublicUnavailable.svelte'

  interface PublicArtifact {
    kind: string
    title: string
    icon: string | null
    body: string
    updatedAt: string
  }

  // A publicly shared artifact — no auth. Only artifacts set to public resolve.
  const slug = $derived(route.getParams('/a/:slug').slug)

  // `r.ok ? … : reject('not found')` made EVERY status "not found", including
  // the ones that mean the server is having a bad minute. A visitor with a
  // perfectly good share link was told the page does not exist.
  let loadState = $state<{ a?: PublicArtifact; missing?: boolean; error?: unknown }>({})
  let reload = $state(0)
  $effect(() => {
    void reload
    let live = true
    loadState = {}
    getJson<{ artifact: PublicArtifact }>(`/api/artifacts/public/${slug}`)
      .then((d) => {
        if (live) loadState = { a: d.artifact }
      })
      .catch((e: unknown) => {
        if (!live) return
        // 404 is the only status that means "there is no such page".
        loadState = e instanceof HttpError && e.status === 404 ? { missing: true } : { error: e }
      })
    return () => {
      live = false
    }
  })

  // First-paint skeleton line widths — doc-page shape regardless of kind.
  const skeletonWidths = ['100%', '94%', '98%', '88%', '96%', '73%', '100%', '91%', '97%', '85%', '95%', '60%']

  function parseSheet(body: string): { head: string[]; rows: string[][] } {
    let grid: string[][] = []
    try {
      const g = JSON.parse(body)
      if (Array.isArray(g)) grid = g.map((r: unknown[]) => (Array.isArray(r) ? r.map((c) => String(c ?? '')) : []))
    } catch {
      /* empty */
    }
    const [head = [], ...rows] = grid
    return { head, rows }
  }
</script>

{#if loadState.missing}
  <PublicNotFound />
{:else if loadState.error}
  <PublicUnavailable detail={errorMessage(loadState.error)} onRetry={() => (reload += 1)} />
{:else if !loadState.a}
  <!-- First paint for link recipients — doc-page shape regardless of kind. -->
  <PublicShell>
    <div aria-hidden="true">
      <Skeleton class="mb-8 h-8 w-2/3" />
      <div class="space-y-3.5">
        {#each skeletonWidths as w, i (i)}
          <div style:width={w}>
            <Skeleton class="h-3.5 w-full rounded-full" />
          </div>
        {/each}
      </div>
    </div>
  </PublicShell>
{:else if loadState.a.kind === 'microsite'}
  <!-- A public microsite is hosted full-bleed in a sandboxed iframe (no app chrome). -->
  <div class="min-h-screen bg-white">
    <iframe title={loadState.a.title} srcdoc={loadState.a.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" class="h-screen w-full border-0"></iframe>
  </div>
{:else}
  <PublicShell meta={`Updated ${relativeTime(loadState.a.updatedAt)}`}>
    <h1 class="mb-5 flex items-center gap-2 font-sans text-3xl font-semibold tracking-tight text-fg">
      <span>{loadState.a.icon ?? '📄'}</span>
      {loadState.a.title}
    </h1>
    {#if loadState.a.kind === 'doc'}
      <Markdown class="tiptap" children={loadState.a.body} />
    {:else if loadState.a.kind === 'sheet'}
      {@const sheet = parseSheet(loadState.a.body)}
      <div class="overflow-x-auto">
        <!-- §8 table voice: mono uppercase chrome header, hairline grid, hover fill. -->
        <table class="border-collapse text-sm">
          <thead>
            <tr>
              {#each sheet.head as h, i (i)}
                <th class="border border-line bg-panel px-3 py-1.5 text-left font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-ink-dim">{h}</th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each sheet.rows as row, ri (ri)}
              <tr data-dither-fill class="transition-colors duration-120">
                {#each row as cell, ci (ci)}
                  <td class="border border-line px-3 py-1.5 font-sans text-fg">{cell}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if loadState.a.kind === 'file'}
      <a
        href={`/api/artifacts/public/${slug}/download`}
        target="_blank"
        rel="noreferrer"
        class={buttonClasses({ variant: 'outline' })}
      >
        ⬇ Download {loadState.a.title}
      </a>
    {:else}
      <p class="text-sm text-muted">This file type isn’t viewable here yet.</p>
    {/if}
  </PublicShell>
{/if}
