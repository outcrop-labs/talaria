<script lang="ts">
  import { route } from '@/router'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, getJson, HttpError } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import PublicShell from '@/components/kb/PublicShell.svelte'
  import PublicNotFound from '@/components/kb/PublicNotFound.svelte'
  import PublicUnavailable from '@/components/kb/PublicUnavailable.svelte'

  interface PublicDoc {
    title: string
    body: string
    updatedAt: string
  }

  // A publicly shared KB doc — no auth. Only docs set to "public" resolve.
  const slug = $derived(route.getParams('/kb/:slug').slug)

  // `r.ok ? … : reject('not found')` made EVERY status "not found", including
  // the ones that mean the server is having a bad minute. A visitor with a
  // perfectly good share link was told the page does not exist.
  let state = $state<{ doc?: PublicDoc; missing?: boolean; error?: unknown }>({})
  let reload = $state(0)
  $effect(() => {
    void reload
    let live = true
    state = {}
    getJson<{ doc: PublicDoc }>(`/api/kb/public/${slug}`)
      .then((d) => {
        if (live) state = { doc: d.doc }
      })
      .catch((e: unknown) => {
        if (!live) return
        // 404 is the only status that means "there is no such page".
        state = e instanceof HttpError && e.status === 404 ? { missing: true } : { error: e }
      })
    return () => {
      live = false
    }
  })

  // First-paint skeleton line widths — hold the document's shape.
  const skeletonWidths = ['100%', '94%', '98%', '88%', '96%', '73%', '100%', '91%', '97%', '85%', '95%', '60%']
</script>

{#if state.missing}
  <PublicNotFound />
{:else if state.error}
  <PublicUnavailable detail={errorMessage(state.error)} onRetry={() => (reload += 1)} />
{:else if !state.doc}
  <!-- First paint for link recipients — hold the document's shape. -->
  <PublicShell>
    <div aria-hidden="true">
      <Skeleton class="mb-8 h-8 w-2/3" />
      <div class="space-y-3.5">
        {#each skeletonWidths as w, i (i)}
          <div style:width={w}>
            <Skeleton class="h-3.5 w-full rounded-full" delay={i * 0.08} />
          </div>
        {/each}
      </div>
    </div>
  </PublicShell>
{:else}
  <PublicShell meta={`Updated ${relativeTime(state.doc.updatedAt)}`}>
    <h1 class="mb-5 font-sans text-3xl font-semibold tracking-tight text-fg">{state.doc.title}</h1>
    <Markdown class="tiptap" children={state.doc.body} />
  </PublicShell>
{/if}
