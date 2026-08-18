<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // The two page archetypes — every route uses exactly one, so the app stops
  // feeling like fifteen apps:
  //
  //   <RailSurface>            work surfaces with a list rail + a stage
  //     <Rail title actions>   LEFT rail: w-72 · bg-sidebar · border-r
  //       rows go here
  //     </Rail>
  //     <Stage header={stageHeader}>the main area</Stage>
  //   </RailSurface>
  //
  //   <PageSurface>            centred "page" surfaces — Agents, Models,
  //     sections go here        Admin, Settings, Home and the rest
  //   </PageSurface>
  //
  //   PageSurface is a COMPONENT now, not a described shape. It used to be this
  //   comment, and ten views hand-rolled it into four different widths.
  //
  // Rail rows share RailRow; surface headers share StageHeader (the one-line
  // bar Comms channels already had). Contextual actions are IconButtons.
  let { children, class: className }: { children: Snippet; class?: string } = $props()
</script>

<div class={cn('flex h-full min-h-0', className)}>{@render children()}</div>
