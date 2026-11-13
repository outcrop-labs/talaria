<script lang="ts">
  import type { Snippet } from 'svelte'
  import { CalendarClock } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { clockLabel, dateLabel } from '@/components/brief/daily-brief.svelte'
  import { cn } from '@/lib/cn'
  import type { BriefView } from '@/lib/daily-brief-types'
  import { alsoToday, ledeStops, todayNotes, type InboxCard, type InboxGlyph } from '@/lib/inbox-board'
  import { wordClamp } from '@/lib/word-clamp'
  import InboxAlerts from './InboxAlerts.svelte'
  import InboxMore from './InboxMore.svelte'
  import { LINK, LINK_HOVER } from './inbox-board.svelte'

  let {
    brief,
    needs,
    tally,
    now,
    calm,
    tools,
    onRing,
    onAgenda,
  }: {
    brief: BriefView
    /** The Needs you lane's cards, for the breakdown. */
    needs: readonly InboxCard[]
    /** "4 running · 8 done today", from the lanes. */
    tally: string | null
    now: number
    /** Nothing in Needs you and nobody waiting: the status line says so. */
    calm: boolean
    /** The Timeline / Mail / Agenda links. */
    tools: Snippet
    /** Scroll to and ring the board's cards of one kind of need. */
    onRing: (glyph: InboxGlyph) => void
    onAgenda: () => void
  } = $props()

  const notes = $derived(todayNotes(brief))

  let lede = $state<HTMLElement | null>(null)
  let readAll = $state(false)

  const LINES = 2
  const LINE_PX = 23
  const stops = $derived(ledeStops(brief.lede))
  let fit = $state(0)
  let clipped = $state(false)

  $effect(() => {
    if (!lede || readAll) return
    const el = lede
    const options = stops
    const max = LINES * LINE_PX
    const measure = (): void => {
      const probe = el.cloneNode(false) as HTMLElement
      probe.style.cssText = `position:absolute;visibility:hidden;display:block;-webkit-line-clamp:none;width:${el.clientWidth}px`
      el.after(probe)
      const first = options.findIndex((text) => {
        probe.textContent = text.replace(/[*_`]|\[([^\]]*)\]\([^)]*\)/g, '$1')
        return probe.scrollHeight <= max + 1
      })
      probe.remove()
      clipped = first < 0
      fit = clipped ? options.length - 1 : first
    }
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  })

  const minutesNow = $derived.by(() => {
    try {
      const parts = new Intl.DateTimeFormat('en-US', { timeZone: brief.zone, hour: 'numeric', minute: 'numeric', hourCycle: 'h23' }).formatToParts(now)
      const part = (type: string): number => Number(parts.find((p) => p.type === type)?.value ?? 0)
      return part('hour') * 60 + part('minute')
    } catch {
      return new Date(now).getHours() * 60 + new Date(now).getMinutes()
    }
  })
  const more = $derived(!!brief.lede && (readAll || fit > 0 || clipped))
  const today = $derived(alsoToday(notes, minutesNow))
</script>

<section data-inbox-today aria-labelledby="inbox-today-title" class="overflow-hidden rounded-xl border border-line bg-panel">
  <div class="px-5 pb-2 pt-2">
    <header class="flex flex-wrap items-center gap-x-4 gap-y-1">
      <div class="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-3">
        <h1 id="inbox-today-title" class="font-sans text-[22px] font-semibold leading-7 tracking-[-0.015em] text-fg">
          {dateLabel(brief.date, brief.zone)}
        </h1>
        {#if brief.lede}
          <p class="font-sans text-[12.5px] text-muted">
            Brief by {brief.agent.name ?? 'your assistant'} · {clockLabel(brief.openedAt, brief.zone)}
          </p>
        {/if}
      </div>
      <div class="flex items-center gap-4">{@render tools()}</div>
    </header>

    {#if brief.lede}
      <div bind:this={lede} class={cn('mt-0.5 max-w-[92ch] font-sans text-[15px] leading-[23px] text-fg', !readAll && 'line-clamp-2')}>
        <Markdown children={readAll ? brief.lede : (stops[fit] ?? brief.lede)} />
      </div>
    {/if}

    {#if more || today.next || today.more}
      <div class="mt-1 flex flex-wrap items-center gap-x-5 gap-y-1 font-sans text-[13px] leading-5">
        {#if more}
          <InboxMore expanded={readAll} onclick={() => (readAll = !readAll)}>
            {readAll ? 'Show less' : 'Read full brief'}
          </InboxMore>
        {/if}
        <div class="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
          {#if today.next}
            <p class="flex min-w-0 items-center gap-1.5 text-fg">
              <CalendarClock size={16} aria-hidden="true" class="shrink-0 text-muted" />
              <span class="shrink-0 font-medium">Next:</span>
              <span use:wordClamp={today.next} class="min-w-0 truncate"></span>
            </p>
          {/if}
          {#if today.more}
            <Button variant="link" title="Open the agenda" class={cn(LINK, LINK_HOVER)} onclick={onAgenda}>
              {today.more}
            </Button>
          {/if}
        </div>
      </div>
    {/if}
  </div>

  <InboxAlerts {needs} {tally} {calm} {onRing} />
</section>
