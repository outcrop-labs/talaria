<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { FitnessBand } from './fitness'

  // A band drawn small: the cell of the matrix, the legend, a verdict line.
  // Shape carries it as well as colour, so Ready and Workable are told apart
  // without either: a full dot, a half dot, a ring, a speck, a dash.
  let { band, class: className }: { band: FitnessBand; class?: string } = $props()

  const MARK: Record<FitnessBand, string> = {
    ready: 'h-2 w-2 rounded-full bg-success',
    workable:
      'h-2 w-2 rounded-full border-[1.5px] border-warning bg-[linear-gradient(to_right,var(--theme-warning)_50%,transparent_50%)]',
    unfit: 'h-2 w-2 rounded-full border-[1.5px] border-danger',
    untested: 'h-1 w-1 rounded-full bg-muted/60',
    unbound: 'h-px w-2 bg-muted',
  }
</script>

<span aria-hidden="true" class={cn('inline-block shrink-0', MARK[band], className)}></span>
