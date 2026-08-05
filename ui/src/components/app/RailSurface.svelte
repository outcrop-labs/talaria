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
  //   (Centered "page" surfaces — Agents, Models, Admin, and the rest — keep the
  //   `h-full overflow-y-auto p-8` + `mx-auto max-w-5xl` shape; PageTitle
  //   standardizes their heading row.)
  //
  // Rail rows share RailRow; surface headers share StageHeader (the one-line
  // bar Comms channels already had). Contextual actions are IconButtons.
  let { children, class: className }: { children: Snippet; class?: string } = $props()
</script>

<div class={cn('flex h-full min-h-0', className)}>{@render children()}</div>
