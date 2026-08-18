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
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import CronsPanel from '@/components/fleet/CronsPanel.svelte'
  import SkillsLibrary from '@/components/skills/SkillsLibrary.svelte'
  import MemoryPanel from '@/components/memory/MemoryPanel.svelte'
  import { fly } from '@/lib/motion'
  import { type Assistant } from '@/lib/assistant'

  let { assistant }: { assistant: Assistant } = $props()
  let tab = $state<Tab>('Schedules')
</script>

<div class="space-y-5">
  <Tabs items={TABS.map((t) => ({ id: t, label: t }))} value={tab} onChange={(t) => (tab = t)} class="border-b border-line pb-2" />
  <!-- Settings-column pane: it resizes with each tab's content, so the switch
       rides AutoHeight; the incoming pane rises in (no exit, no stagger — these
       are data panes, not section stacks). -->
  <AutoHeight>
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }}>
        {#if tab === 'Schedules'}<CronsPanel agentId={assistant.id} />{/if}
        {#if tab === 'Skills'}<SkillsLibrary owner={assistant.slug} ownerLabel={assistant.displayName} class="h-[32rem]" />{/if}
        {#if tab === 'Memory'}
          <MemoryPanel
            id={assistant.id}
            label={assistant.displayName}
            museContext={`The memory of ${assistant.displayName}, a personal AI assistant.`}
            offline={assistant.running ? null : { title: 'Memory unavailable', hint: 'Start your assistant to read its memory.' }}
          />
        {/if}
      </div>
    {/key}
  </AutoHeight>
</div>
