<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'

  /**
   * The board page's silhouette — built to hold ITS EXACT LAYOUT, not to
   * resemble it, because everything this file gets wrong is layout shift when
   * the board lands.
   *
   * Two rules make that hold, and both cost more markup than an approximation:
   *
   * 1. THE CONTAINER CHAIN IS COPIED VERBATIM from its counterparts —
   *    `BoardHeader`'s frame, `Board.svelte`'s two toolbar rows and the
   *    `relative min-h-0 min-w-0 flex-1` canvas slot, `Kanban`/`BoardList`/
   *    `Gantt`'s own roots. Same padding, same borders, same gaps, same flex
   *    math, nesting included. An intermediate wrapper that looks redundant is
   *    usually the one deciding a height.
   *
   * 2. A TEXT SLOT KEEPS THE REAL TEXT'S LINE BOX. `text-lg` is 28px tall
   *    whatever it says; a 24px bar in its place shortens the row by 4px, and
   *    a header, two toolbar rows and every card title do that together. So
   *    `line` renders the real typography classes around an invisible glyph —
   *    the browser computes the box exactly as it will for the real string —
   *    and floats the dither inside it. The ink is a bar; the BOX is the text's.
   *
   * The view comes from the URL (`search.view`), so it is known before any
   * query resolves: a board opened on the list view must not be handed a
   * kanban skeleton and then reflow the entire canvas. That is the largest
   * shift available on this page and it costs one prop to avoid.
   */
  let { view = 'board' }: { view?: 'board' | 'list' | 'gantt' } = $props()

  const COLUMNS: Array<Array<{ title: string; body: boolean }>> = [
    [{ title: 'w-4/5', body: true }, { title: 'w-3/5', body: false }],
    [{ title: 'w-11/12', body: true }, { title: 'w-2/3', body: true }, { title: 'w-1/2', body: false }],
    [{ title: 'w-3/4', body: true }, { title: 'w-5/6', body: false }],
    [{ title: 'w-2/3', body: false }],
    [{ title: 'w-5/6', body: true }, { title: 'w-1/2', body: false }],
    [{ title: 'w-3/5', body: false }, { title: 'w-4/5', body: true }, { title: 'w-2/3', body: false }],
  ]
  const PILLS = ['w-8', 'w-10', 'w-6', 'w-9']
  const LIST_COLS = ['w-16', 'w-56', 'w-20', 'w-16', 'w-24', 'w-20', 'w-16']
  const ROWS = ['w-3/5', 'w-4/5', 'w-2/5', 'w-3/4', 'w-1/2', 'w-5/6', 'w-2/3', 'w-3/5', 'w-4/5', 'w-1/2']
</script>

<!--
  One text slot. `typo` is the real element's typography (that is what sizes the
  line box); `w` its width; `bar` the height of the ink inside. The invisible
  glyph is load-bearing — without it the span has no line box and the row
  collapses by however much the real text would have added.
-->
<!--
  `block` is not cosmetic. An inline-flex line inside a BLOCK parent sits on
  that parent's baseline and collects descender space underneath — a few px per
  line, which on a card title plus a table cell plus a gantt label is exactly
  the shift this file exists to remove. Inside a flex ROW it is a flex item and
  the gap never appears, so those stay inline.
-->
{#snippet line(typo: string, w: string, bar = 'h-2.5', block = false)}
  <span class={cn('relative items-center', block ? 'flex' : 'inline-flex', typo, w)}>
    <span class="invisible" aria-hidden="true">&nbsp;</span>
    <Skeleton class={cn('absolute inset-y-0 left-0 my-auto w-full rounded-full', bar)} />
  </span>
{/snippet}

{#snippet pill(w: string)}
  <!-- FieldPill's own box: border + px-1.5 py-0.5 + leading-3. -->
  <span class="relative inline-flex items-center gap-1.5 rounded-md border border-line-subtle px-1.5 py-0.5 font-mono text-[10px] uppercase leading-3">
    <span class={cn('invisible', w)} aria-hidden="true">&nbsp;</span>
    <Skeleton class={cn('absolute inset-y-0 my-auto h-2 rounded-full', w)} />
  </span>
{/snippet}

{#snippet card(spec: { title: string; body: boolean })}
  <!-- KanbanCard's frame, verbatim. -->
  <div class="relative w-full overflow-hidden rounded-lg border border-line bg-panel p-4 shadow-[var(--theme-shadow-1)]">
    <div class="flex items-start gap-2.5">
      <!-- Priority dot: every card has one, same place, same size — a rail, so
           it is drawn flat rather than dithered (UI-CONVENTIONS, Loading). -->
      <span class="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-line"></span>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5">
          {@render line('font-mono text-[11px] tracking-[0.05em]', 'w-14', 'h-2')}
        </div>
        {@render line('font-sans text-[15px] font-medium leading-snug', spec.title, 'h-3', true)}
        {#if spec.body}
          <!-- `line-clamp-3` means a description that exists almost always
               renders at exactly three lines, so three is the box to hold —
               and they are BLOCK lines, because two inline ones would share a
               line box whenever they fit side by side. -->
          <div class="mt-1">
            {@render line('font-sans text-xs leading-relaxed', 'w-full', 'h-2', true)}
            {@render line('font-sans text-xs leading-relaxed', 'w-11/12', 'h-2', true)}
            {@render line('font-sans text-xs leading-relaxed', 'w-2/3', 'h-2', true)}
          </div>
        {/if}
      </div>
    </div>
    <div class="mt-2.5 flex flex-wrap items-center gap-1">
      {#each PILLS as w (w)}{@render pill(w)}{/each}
    </div>
  </div>
{/snippet}

<div aria-hidden="true" class="flex h-full min-w-0 flex-col">
  <!-- BoardHeader -->
  <div class="flex items-center gap-3 border-b border-line-subtle px-5 py-3">
    <div class="min-w-0 flex-1">
      {@render line('font-sans text-lg font-semibold', 'w-52', 'h-4', true)}
    </div>
    <div class="flex -space-x-2">
      {#each [0, 1, 2] as i (i)}
        <Skeleton class="h-7 w-7 rounded-full ring-2 ring-[color:var(--theme-panel)]" />
      {/each}
    </div>
    <Skeleton class="h-8 w-8 shrink-0 rounded-md" />
  </div>

  <!-- Row 1 — the VIEW: mode toggle, saved views, ticket count. The toggle
       frame and its three cells always exist, so the frame is drawn. -->
  <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-2">
    <div class="flex rounded-md border border-line p-0.5">
      {#each [0, 1, 2] as i (i)}
        <Skeleton class="h-7 w-7 rounded-md" />
      {/each}
    </div>
    {@render line('font-mono text-[10px] uppercase tracking-[0.05em]', 'w-20', 'h-2')}
    <span class="ml-auto">
      {@render line('font-mono text-[10px] uppercase tracking-[0.05em]', 'w-16', 'h-2')}
    </span>
  </div>

  <!-- Row 2 — the QUERY: search, filter bar, display options. Both controls are
       h-9, which is what the board itself draws here while its facets load. -->
  <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-1.5">
    <Skeleton class="h-9 w-40 rounded-md" />
    <Skeleton class="h-9 w-64 rounded-md" />
    <span class="ml-auto flex items-center gap-1.5">
      <Skeleton class="h-7 w-7 rounded-md" />
    </span>
  </div>

  <!-- The canvas slot, exactly as Board.svelte declares it. -->
  <div class="relative min-h-0 min-w-0 flex-1">
    {#if view === 'board'}
      <!-- Kanban: root, then fixed-width columns on a horizontal scroll. NOT a
           responsive grid — a grid stretches its columns to the viewport and
           every one of them changes width when the real board lands. -->
      <div class="flex h-full flex-col">
        <div class="flex min-h-0 flex-1 gap-3 overflow-x-auto p-4">
          {#each COLUMNS as cards, c (c)}
            <div class="flex w-80 shrink-0 flex-col rounded-lg bg-panel/60 ring-1 ring-transparent">
              <div class="flex items-center gap-2 px-3 py-2">
                <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></span>
                {@render line('font-mono text-[10px] font-medium uppercase tracking-[0.08em]', 'w-20', 'h-2')}
                {@render line('font-mono text-[10px] tracking-[0.05em]', 'w-3', 'h-2')}
              </div>
              <div class="flex-1 space-y-2 overflow-hidden px-2 pb-2">
                {#each cards as spec, i (i)}{@render card(spec)}{/each}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {:else if view === 'gantt'}
      <!-- Gantt: a 220px label gutter (`labelW`), a two-tier scale header
           (months over days), then h-12 rows carrying an `absolute top-2 h-8`
           bar. Every one of those numbers is copied, not chosen. -->
      <div class="flex h-full flex-col">
        <div class="min-h-0 flex-1 overflow-hidden">
          <div class="flex min-h-full flex-col">
            <div class="flex border-b border-line bg-surface/95">
              <div class="flex w-[220px] shrink-0 items-center gap-0.5 border-r border-line-subtle px-2">
                <Skeleton class="h-5 w-5 rounded" />
                <Skeleton class="h-5 w-5 rounded" />
                <span class="ml-1">
                  {@render line('font-mono text-[10px] uppercase tracking-[0.08em]', 'w-8', 'h-2')}
                </span>
              </div>
              <div class="relative flex-1">
                <div class="flex">
                  {#each [0, 1, 2] as m (m)}
                    <div class="flex-1 overflow-hidden border-r border-line-subtle px-2 py-1">
                      {@render line('font-mono text-[10px] font-medium uppercase tracking-[0.08em]', 'w-16', 'h-2', true)}
                    </div>
                  {/each}
                </div>
                <div class="flex border-t border-line-subtle">
                  {#each { length: 30 } as _, i (i)}
                    <div class="w-[26px] shrink-0 overflow-hidden py-0.5 text-center font-mono text-[9px]">
                      <span class="invisible" aria-hidden="true">&nbsp;</span>
                    </div>
                  {/each}
                </div>
              </div>
            </div>
            {#each ROWS as w, i (i)}
              <div class="flex h-12 items-center border-b border-line-subtle/60">
                <div class="flex h-full w-[220px] shrink-0 items-center gap-1.5 border-r border-line-subtle px-2 text-xs">
                  {@render line('font-sans text-xs', w, 'h-2.5')}
                </div>
                <div class="relative h-full flex-1">
                  <!-- The bar's own box: `absolute top-2 h-8`, offset along
                       the timeline so the rows do not read as a bar chart. -->
                  <div
                    class={cn('absolute top-2 h-8', ['w-1/4', 'w-1/3', 'w-1/2', 'w-1/5', 'w-2/5'][i % 5])}
                    style="left: {(i % 4) * 12}%"
                  >
                    <Skeleton class="h-full w-full rounded-md" />
                  </div>
                </div>
              </div>
            {/each}
          </div>
        </div>
      </div>
    {:else}
      <!-- BoardList: a real table, so the skeleton is a real table — column
           widths are negotiated by the browser and only a table reproduces
           them. -->
      <div class="flex h-full flex-col">
        <div class="relative min-h-0 flex-1 overflow-hidden p-4">
          <table class="w-full text-sm">
            <thead class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              <tr>
                <th class="w-7 py-2 pl-2"></th>
                {#each LIST_COLS as _c, i (i)}
                  <th class="px-3 py-2 font-medium">
                    {@render line('font-mono text-[10px] uppercase tracking-[0.08em]', 'w-12', 'h-2', true)}
                  </th>
                {/each}
                <th class="w-8 py-1 pr-2"></th>
              </tr>
            </thead>
            <tbody class="divide-y divide-line-subtle">
              {#each ROWS as w, r (r)}
                <tr>
                  <td class="pl-2"></td>
                  {#each LIST_COLS as cw, i (i)}
                    <td class="px-3 py-2">
                      {@render line('font-sans text-sm', i === 1 ? w : cw, 'h-2.5', true)}
                    </td>
                  {/each}
                  <td></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </div>
    {/if}
  </div>
</div>
