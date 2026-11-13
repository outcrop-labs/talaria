<script lang="ts">
  import { cn } from '@/lib/cn'

  // A capacity bar. It stays neutral while there is room: the theme's warning
  // is the accent gold, so an accent fill could not be told from a warning.
  let {
    percent,
    label,
    pressure = 'ok',
    class: className,
  }: {
    percent: number
    /** The accessible name, e.g. "/var 82% full". */
    label: string
    pressure?: 'ok' | 'warn' | 'danger'
    class?: string
  } = $props()

  const FILL = { ok: 'bg-muted', warn: 'bg-warning', danger: 'bg-danger' }
  const clamped = $derived(Math.min(100, Math.max(0, percent)))
</script>

<div
  class={cn('h-1.5 overflow-hidden rounded-full bg-line', className)}
  role="meter"
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(clamped)}
  aria-label={label}
>
  <div class={cn('h-full rounded-full', FILL[pressure])} style:width="{clamped}%"></div>
</div>
