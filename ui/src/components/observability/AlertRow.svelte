<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { navigateHref } from '@/router'
  import { SEVERITY, type Alert } from './alerts'

  // One alert, in the row grammar of the Manage overview's "Needs attention"
  // list: severity, what is wrong, and the way to where it is fixed.
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

<SettingsRow
  label={alert.title}
  hint={alert.detail}
  disabled={!alert.href}
  onclick={() => alert.href && void navigateHref(alert.href)}
>
  {#snippet lead()}
    {#if named}
      <span class="flex w-[5.5rem] items-center gap-2 font-sans text-[13px] leading-5 text-muted">
        <StatusDot {...severity.dot} class="h-2 w-2" />{severity.label}
      </span>
    {:else}
      <StatusDot {...severity.dot} class="h-2 w-2" />
    {/if}
  {/snippet}
  {#if alert.href}
    <span class="flex items-center gap-1 font-sans text-[13px] font-medium leading-5 text-muted">Fix<ChevronRight size={14} /></span>
  {/if}
</SettingsRow>
