<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { fetchInstallRequests, fetchUpdateRequests, type InstalledApp } from './apps'
  import AppsInstallRequests from './AppsInstallRequests.svelte'
  import AppsUpdateRequests from './AppsUpdateRequests.svelte'

  /** The installed apps, so an update shows its app's name. */
  let { apps }: { apps: InstalledApp[] } = $props()

  // What waits on an admin, as one list: the apps agents built and want
  // installed, then the updates to apps already here. Both are a decision
  // about an app with the same two answers, and most days each has one row
  // or none, so two cards of one row each said less than one card of two.
  // A failed poll must never read as "nothing waiting on you".
  const installs = createQuery(() => ({
    queryKey: ['app-install-requests'],
    queryFn: fetchInstallRequests,
    refetchInterval: 60_000,
  }))
  const updates = createQuery(() => ({
    queryKey: ['app-update-requests'],
    queryFn: fetchUpdateRequests,
    refetchInterval: 60_000,
  }))
  const installRequests = $derived(installs.data ?? [])
  const updateRequests = $derived(updates.data ?? [])
  const waiting = $derived(installRequests.length + updateRequests.length)

  /** The last decision of each kind, as it was written on the ticket. */
  type Outcome = { ok: boolean; text: string } | null
  let installOutcome = $state<Outcome>(null)
  let updateOutcome = $state<Outcome>(null)
  const outcomeClass = (o: { ok: boolean }) => cn('font-sans text-[13px] leading-5', waiting > 0 && 'mt-2', o.ok ? 'text-muted' : 'text-danger')
</script>

{#if installs.isError && installs.data === undefined}
  <QueryError variant="inline" error={installs.error} title="Could not load install requests" onRetry={() => void installs.refetch()} />
{/if}
{#if updates.isError && updates.data === undefined}
  <QueryError variant="inline" error={updates.error} title="Could not load plugin updates" onRetry={() => void updates.refetch()} />
{/if}
{#if waiting > 0 || installOutcome || updateOutcome}
  <div data-install-requests data-update-requests>
    <SettingsSection
      title="Waiting for approval"
      description="New apps and updates your agents are asking you to approve."
      info="An approved app starts disabled. An update that fails to load is rolled back."
    >
      {#if waiting > 0}
        <SettingsGroup>
          <AppsInstallRequests requests={installRequests} bind:outcome={installOutcome} />
          <AppsUpdateRequests requests={updateRequests} {apps} bind:outcome={updateOutcome} />
        </SettingsGroup>
      {/if}
      {#if installOutcome}
        <div transition:slide={{ duration: 150 }} class={outcomeClass(installOutcome)} data-install-outcome>{installOutcome.text}</div>
      {/if}
      {#if updateOutcome}
        <div transition:slide={{ duration: 150 }} class={outcomeClass(updateOutcome)} data-update-outcome>{updateOutcome.text}</div>
      {/if}
    </SettingsSection>
  </div>
{/if}
