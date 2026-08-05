<script lang="ts">
  import { navigate } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import ActivityRow from '@/components/app/ActivityRow.svelte'
  import type { OrgActivity } from './home'

  let { activity }: { activity: OrgActivity[] } = $props()
</script>

<Panel>
  <SectionHeader title="Pulse" class="mb-2" />
  {#if activity.length === 0}
    <EmptyState variant="inline" title="Quiet so far." hint="Activity across boards, comms, and the fleet shows here." />
  {:else}
    <ul>
      {#each activity as a, i (i)}
        <ActivityRow actor={a.actor} detail={a.detail} at={a.at} context={a.context} onClick={() => a.href && void navigate(a.href)} />
      {/each}
    </ul>
  {/if}
</Panel>
