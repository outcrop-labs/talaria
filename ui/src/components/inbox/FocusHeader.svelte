<script lang="ts">
  import { CalendarDays, Mail } from '@lucide/svelte'

  // `count: null` means the queue could not be read. It renders as an em dash,
  // never as 0 — a zero here is a claim, and this is the surface that must not
  // make that claim on a failed read.
  let {
    count,
    current,
    onOpenMail,
    onOpenAgenda,
  }: {
    count: number | null
    current: number
    onOpenMail: () => void
    onOpenAgenda: () => void
  } = $props()

  const unknown = $derived(count === null)
</script>

<header class="flex flex-wrap items-center gap-2 border-b border-line pb-4 sm:gap-3">
  <div class="flex min-w-0 flex-1 items-baseline gap-2">
    <h1 class="font-sans text-lg font-medium tracking-tight text-fg">Inbox</h1>
    <span class="font-mono text-[11px] tabular-nums tracking-[0.06em] text-muted">· {unknown ? '—' : count}</span>
  </div>
  <span class="hidden font-mono text-[10px] tabular-nums tracking-[0.08em] text-ink-dim sm:block">
    {unknown ? '—— / ——' : `${String(current).padStart(2, '0')} / ${String(count).padStart(2, '0')}`}
  </span>
  <button type="button" onclick={onOpenMail} class="flex h-8 items-center gap-1.5 rounded-md border border-line px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:bg-hover hover:text-fg" aria-label="Open Mail drawer">
    <Mail size={12} /> Mail
  </button>
  <button type="button" onclick={onOpenAgenda} class="flex h-8 items-center gap-1.5 rounded-md border border-line px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:bg-hover hover:text-fg" aria-label="Open Agenda drawer">
    <CalendarDays size={12} /> Agenda
  </button>
</header>
