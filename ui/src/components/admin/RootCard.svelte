<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import type { RootHealth } from '@/lib/secrets'

  // ── The root ─────────────────────────────────────────────────────────────────
  // Deliberately the first thing on the page, and deliberately not editable. It
  // lives in the process environment, so the app can only report it — and saying
  // it exists at all is most of the value, because nothing else in the product
  // mentions it until the day it stops matching.
  let { root }: { root: RootHealth } = $props()

  const tone = $derived(
    root.state === 'ok' ? 'ok' : root.state === 'fallback' ? 'warn' : ('danger' as const),
  )
  const headline = $derived(
    root.state === 'ok'
      ? `Set from ${root.name}`
      : root.state === 'fallback'
        ? 'Borrowing AUTH_SECRET'
        : root.state === 'absent'
          ? 'Not set'
          : 'Does not match this database',
  )
  const body = $derived(
    root.state === 'ok'
      ? 'Every secret below is sealed with a data key wrapped by this value. Back it up with the database. A dump restored without it gives an instance that cannot read its own secrets.'
      : root.state === 'fallback'
        ? 'TALARIA_SECRET_KEY is not set, so AUTH_SECRET is doing this job. Rotating AUTH_SECRET would make every secret below unrecoverable. Set TALARIA_SECRET_KEY to the current AUTH_SECRET value to pin it.'
        : root.state === 'absent'
          ? 'Neither TALARIA_SECRET_KEY nor AUTH_SECRET is set, so nothing new can be sealed. Set TALARIA_SECRET_KEY and restart.'
          : (root.failure ??
            'The root secret this process has is not the one these secrets were sealed with.'),
  )
</script>

<div
  class={cn(
    'rounded-md border p-4',
    root.state === 'ok' ? 'border-line' : root.state === 'fallback' ? 'border-warning/40' : 'border-danger/40',
  )}
>
  <div class="mb-1.5 flex flex-wrap items-center gap-2">
    <StatusDot status={tone} />
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Encryption root</span>
    <span class="text-sm font-medium text-fg">{headline}</span>
    <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
      {root.activeVersion ? `key v${root.activeVersion}` : 'no key'}
      {root.storedVersions > root.loadedVersions.length
        ? ` · ${root.storedVersions - root.loadedVersions.length} version${root.storedVersions - root.loadedVersions.length === 1 ? '' : 's'} unreadable`
        : ''}
    </span>
  </div>
  <p class="text-xs leading-relaxed text-muted">{body}</p>
</div>
