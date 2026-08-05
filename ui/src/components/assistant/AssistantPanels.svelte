<script lang="ts" module>
  // The assistant's working parts — Schedules, Skills, Memory — rendered inline
  // on the Settings › Assistant tab (identity/model/power live in
  // AssistantSection above). All server calls are owner-scoped; nothing here
  // needs the admin role.
  const TABS = ['Schedules', 'Skills', 'Memory'] as const
  type Tab = (typeof TABS)[number]
</script>

<script lang="ts">
  import Tabs from '@/components/ui/Tabs.svelte'
  import CronsPanel from '@/components/fleet/CronsPanel.svelte'
  import SkillsTab from './SkillsTab.svelte'
  import MemoryTab from './MemoryTab.svelte'
  import { type Assistant } from '@/lib/assistant'

  let { assistant }: { assistant: Assistant } = $props()
  let tab = $state<Tab>('Schedules')
</script>

<div class="space-y-5">
  <Tabs items={TABS.map((t) => ({ id: t, label: t }))} value={tab} onChange={(t) => (tab = t)} class="border-b border-line pb-2" />
  {#if tab === 'Schedules'}<CronsPanel agentId={assistant.id} />{/if}
  {#if tab === 'Skills'}<SkillsTab {assistant} />{/if}
  {#if tab === 'Memory'}<MemoryTab {assistant} />{/if}
</div>
