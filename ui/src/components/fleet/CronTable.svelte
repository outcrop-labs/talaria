<script lang="ts">
  import type { Snippet } from 'svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import { CRON_AGENT, CRON_FREQUENCY, CRON_NEXT, CRON_ON } from './CronRow.svelte'

  // The schedules card: a caption row over `CronRow`s. It is the container
  // the rows' cells size against, so the fleet's list and the narrower one in
  // an agent's dialog each show the columns they have room for.
  let {
    agents = false,
    children,
  }: {
    /** The rows name their agent (the fleet's list). */
    agents?: boolean
    children: Snippet
  } = $props()
</script>

<div class="@container">
  <SettingsGroup>
    <SettingsHeaderRow label="Schedule" menu expands class="hidden @3xl:flex">
      {#if agents}<span class={CRON_AGENT}>Agent</span>{/if}
      <span class={CRON_FREQUENCY}>Frequency</span>
      <span class={CRON_NEXT}>Next run</span>
      <span class={CRON_ON}>On</span>
    </SettingsHeaderRow>
    {@render children()}
  </SettingsGroup>
</div>
