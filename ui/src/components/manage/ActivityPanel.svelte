<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { ChevronRight } from '@lucide/svelte'
  import { navigateHref } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { actorLabel } from '@/components/observability/actor'
  import { relativeTime } from '@/lib/fleet'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getList } from '@/lib/fetch-json'

  let {
    allHref,
    below = 0,
  }: {
    /** The Audit tab, for a viewer who can open it. */
    allHref?: string
    /** Height of what follows this section on the first screen, with its gap. */
    below?: number
  } = $props()

  // What changed lately: the audit trail's newest rows, the same read (and
  // cache key) the Observability overview and Home use.
  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const audit = $derived(auditQuery.data)

  const MOST = 5
  const FEWEST = 3
  // Clear space left at the foot of the first screen: the page's own gutter.
  const MARGIN = 32
  let shown = $state(MOST)
  let height = $state<number | null>(null)

  const top = (el: HTMLElement | null): number => (el ? el.offsetTop + top(el.offsetParent as HTMLElement | null) : 0)

  // The first screen ends on a whole row. Every row stays laid out, so each
  // can be measured against the window; the list is cut after the last one
  // that fits above the fold with what follows it, and is never shorter than
  // three rows.
  function endOnWholeRow(list: HTMLElement) {
    const scroller = list.closest<HTMLElement>('.overflow-y-auto')
    if (!scroller) return
    const measure = () => {
      const rows = [...list.children] as HTMLElement[]
      const foot = (list.nextElementSibling as HTMLElement | null)?.offsetHeight ?? 0
      const room = scroller.clientHeight - (top(list) - top(scroller)) - foot - below - MARGIN
      let fits = 0
      while (fits < rows.length && rows[fits]!.offsetTop + rows[fits]!.offsetHeight <= room) fits++
      shown = Math.min(rows.length, Math.max(FEWEST, fits))
      const last = rows[shown - 1]
      height = last ? last.offsetTop + last.offsetHeight : null
    }
    // The column grows when anything above the list loads, and that moves the list without resizing it.
    const observer = new ResizeObserver(measure)
    observer.observe(scroller)
    if (scroller.firstElementChild) observer.observe(scroller.firstElementChild)
    observer.observe(list)
    return { destroy: () => observer.disconnect() }
  }
</script>

<SettingsSection title="Recent activity">
  <SettingsGroup>
    {#if auditQuery.isLoading}
      <div class="px-5 py-4"><SkeletonRows rows={5} /></div>
    {:else if audit === undefined}
      <!-- "Quiet so far." is a fact about the log, not about a failed request. -->
      <div class="px-5 py-4">
        <QueryError
          variant="inline"
          error={auditQuery.error}
          title="Could not load recent activity"
          onRetry={() => void auditQuery.refetch()}
        />
      </div>
    {:else if audit.length === 0}
      <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Quiet so far.</p>
    {:else}
      <div
        use:endOnWholeRow
        class="relative overflow-hidden [&>*+*]:border-t [&>*+*]:border-line-subtle"
        style:max-height={height === null ? undefined : `${height}px`}
      >
        {#each audit.slice(0, MOST) as a, i (i)}
          <button
            type="button"
            inert={i >= shown}
            onclick={() => a.href && void navigateHref(a.href)}
            class="flex min-h-11 w-full items-start gap-6 px-5 py-3 text-left font-sans text-sm leading-5 transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
          >
            <span class="line-clamp-2 min-w-0 flex-1 break-words text-muted" title={a.detail}>
              <span class="whitespace-nowrap font-medium text-fg">{actorLabel(a.actor)}</span>
              {a.detail}
            </span>
            <span class="shrink-0 text-[13px] tabular-nums text-muted">{relativeTime(a.at)}</span>
          </button>
        {/each}
      </div>
      {#if allHref}
        <a
          href={allHref}
          class="flex h-11 items-center gap-1 px-5 font-sans text-[13px] font-medium leading-5 text-muted transition-colors hover:bg-hover hover:text-fg focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-accent"
        >
          All activity<ChevronRight size={14} aria-hidden="true" />
        </a>
      {/if}
    {/if}
  </SettingsGroup>
</SettingsSection>
