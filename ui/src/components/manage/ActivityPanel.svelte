<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { actorLabel } from '@/components/observability/actor'
  import { relativeTime } from '@/lib/fleet'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getList } from '@/lib/fetch-json'

  // What changed lately: the audit trail's newest rows, the same read (and
  // cache key) the Observability overview and Home use.
  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const audit = $derived(auditQuery.data)
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
      {#each audit.slice(0, 7) as a, i (i)}
        <button
          type="button"
          onclick={() => a.href && void navigateHref(a.href)}
          class="flex h-11 w-full items-center gap-6 px-5 text-left font-sans text-sm leading-5 transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
        >
          <span class="min-w-0 flex-1 truncate text-muted">
            <span class="font-medium text-fg">{actorLabel(a.actor)}</span>
            {a.detail}
          </span>
          <span class="shrink-0 text-[13px] tabular-nums text-muted">{relativeTime(a.at)}</span>
        </button>
      {/each}
    {/if}
  </SettingsGroup>
</SettingsSection>
