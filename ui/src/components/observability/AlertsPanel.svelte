<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import AlertRow, { ALERT_SEVERITY } from './AlertRow.svelte'
  import { useAlerts, type Alert, type AlertSeverity } from './alerts'
  import StatStripSkeleton from './StatStripSkeleton.svelte'

  // Derived health signals, computed live from container state, the gateway,
  // the token ledger and stuck tickets. Nothing to configure, nothing stored.
  const query = useAlerts()
  const count = (alerts: Alert[], severity: AlertSeverity) => alerts.filter((a) => a.severity === severity).length
</script>

<!-- "All clear" is the single most dangerous thing this app can say wrongly:
     it is the sentence an operator uses to decide NOT to look. It may only
     render off a 200. -->
<QueryState {query} errorTitle="Could not check for alerts">
  {#snippet skeleton()}
    <StatStripSkeleton cells={3} />
  {/snippet}
  {#snippet empty()}
    <EmptyState icon="△" title="All clear" hint="Agents running, gateway answering, ledger priced, no stuck work." />
  {/snippet}
  {#snippet children(alerts)}
    <div class="space-y-8">
      <StatStrip columns={3}>
        <Stat label="Critical" value={count(alerts, 'critical')} sub="Failing now" />
        <Stat label="Warnings" value={count(alerts, 'warning')} sub="Degraded or at risk" />
        <Stat label="Info" value={count(alerts, 'info')} sub="No action needed yet" />
      </StatStrip>
      <SettingsSection title="Open alerts" description="Each alert opens the page where it is fixed.">
        <SettingsGroup>
          <SettingsHeaderRow label="Alert" lead="Severity" leadClass={ALERT_SEVERITY} />
          {#each alerts as a, i (i)}
            <AlertRow alert={a} named />
          {/each}
        </SettingsGroup>
      </SettingsSection>
    </div>
  {/snippet}
</QueryState>
