<script lang="ts">
  import type { BriefEvidence } from '@/lib/daily-brief-types'

  /**
   * NO LINK ANYWHERE ELSE WOULD DO. The approval has no page to open: the
   * pending action lives behind an API the brief is the only surface for.
   */
  let { evidence, isEvent }: { evidence: BriefEvidence[]; isEvent: boolean } = $props()
</script>

<div class="rounded-lg border border-line bg-surface p-3">
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
    {isEvent ? 'Your assistant drafted an event' : 'Your assistant drafted an email'}
  </div>

  {#if evidence.length > 0}
    <dl class="mt-1.5 space-y-1">
      {#each evidence as row (row.label + row.text)}
        <div class="flex gap-2">
          <dt class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            {row.label}
          </dt>
          <dd class="min-w-0 flex-1 whitespace-pre-wrap break-words font-sans text-[12.5px] leading-5 text-fg">{row.text}</dd>
        </div>
      {/each}
    </dl>
  {/if}
</div>
