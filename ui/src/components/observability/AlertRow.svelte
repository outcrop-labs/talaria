<script lang="ts" module>
  // The severity column: the list's caption row and each named row's lead share this width.
  export const ALERT_SEVERITY = 'w-[5.5rem]'
</script>

<script lang="ts">
  import { cn } from '@/lib/cn'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { navigateHref } from '@/router'
  import { SEVERITY, type Alert } from './alerts'

  // One alert: severity, what is wrong on one line, why it matters under it,
  // and the way to where it is fixed.
  let {
    alert,
    named = false,
  }: {
    alert: Alert
    /** Spell the severity out beside its dot, as a column. */
    named?: boolean
  } = $props()

  const severity = $derived(SEVERITY[alert.severity])
</script>

<!-- py-2: at this height three alerts leave the overview's first screen whole. -->
{#snippet impact()}
  <span class="line-clamp-2" title={alert.detail}>{alert.detail}</span>
{/snippet}

<SettingsRow
  label={alert.title}
  wrap
  hint={alert.detail ? impact : undefined}
  disabled={!alert.href}
  onclick={() => alert.href && void navigateHref(alert.href)}
  class="py-2"
>
  {#snippet lead()}
    {#if named}
      <span class={cn(ALERT_SEVERITY, 'flex items-center gap-2 font-sans text-[13px] leading-5 text-muted')}>
        <StatusDot status={severity.dot} class="h-2 w-2" />{severity.label}
      </span>
    {:else}
      <StatusDot status={severity.dot} class="h-2 w-2" />
    {/if}
  {/snippet}
  {#if alert.href}
    <SettingsRowFix text={severity.action} />
  {/if}
</SettingsRow>
