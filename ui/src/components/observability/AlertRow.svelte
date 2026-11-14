<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { navigateHref } from '@/router'
  import { SEVERITY, type Alert } from './alerts'

  // One alert: severity, what is wrong, and the way to where it is fixed.
  let {
    alert,
    named = false,
    compact = false,
  }: {
    alert: Alert
    /** Spell the severity out beside its dot, as a column. */
    named?: boolean
    /** One line: the title, then as much of the explanation as fits, with
     *  all of it in a tooltip. For a summary of the list on another page. */
    compact?: boolean
  } = $props()

  const severity = $derived(SEVERITY[alert.severity])
</script>

<!-- The explanation gives way first, and only then the title. -->
{#snippet oneLine()}
  <span class="flex min-w-0 items-baseline gap-2.5">
    <span class="max-w-full shrink-0 truncate">{alert.title}</span>
    {#if alert.detail}
      <span class="min-w-0 truncate text-[13px] font-normal text-muted" title={alert.detail}>{alert.detail}</span>
    {/if}
  </span>
{/snippet}

<SettingsRow
  label={compact ? oneLine : alert.title}
  hint={compact ? undefined : alert.detail}
  disabled={!alert.href}
  onclick={() => alert.href && void navigateHref(alert.href)}
  class={compact ? 'min-h-11 py-1.5' : undefined}
>
  {#snippet lead()}
    {#if named}
      <span class="flex w-[5.5rem] items-center gap-2 font-sans text-[13px] leading-5 text-muted">
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
