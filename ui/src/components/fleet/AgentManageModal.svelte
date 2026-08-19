<script lang="ts">
  import Modal from '@/components/ui/Modal.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import AgentConfigForm from '@/components/fleet/AgentConfigForm.svelte'
  import CronsPanel from '@/components/fleet/CronsPanel.svelte'
  import McpTab from '@/components/fleet/McpTab.svelte'
  import MemoryEditor from '@/components/memory/MemoryEditor.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import ReadOnlyConfig from '@/components/fleet/ReadOnlyConfig.svelte'
  import SecretsTab from '@/components/fleet/SecretsTab.svelte'
  import SkillsLibrary from '@/components/skills/SkillsLibrary.svelte'
  import SummaryTab from '@/components/fleet/SummaryTab.svelte'
  import VersionsTab from '@/components/fleet/VersionsTab.svelte'
  import type { AgentDef, LlmEndpoint } from '@/lib/fleet-defs'
  import { fly } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  type Tab = 'summary' | 'config' | 'skills' | 'memory' | 'crons' | 'secrets' | 'mcp' | 'versions'
  const TABS: TabItem<Tab>[] = [
    { id: 'summary', label: 'Summary' },
    { id: 'config', label: 'Config' },
    { id: 'skills', label: 'Skills' },
    { id: 'memory', label: 'Memory' },
    { id: 'crons', label: 'Crons' },
    { id: 'secrets', label: 'Secrets' },
    { id: 'mcp', label: 'MCP' },
    { id: 'versions', label: 'Versions' },
  ]

  // The whole of an agent's internal stack in one modal: model config, skills,
  // memory, MCP servers, and version history — no more hopping between top-level
  // pages. Read-only for non-admins.
  let {
    open,
    onClose,
    def,
    endpoints,
    isAdmin,
  }: {
    open: boolean
    onClose: () => void
    def: AgentDef
    endpoints: LlmEndpoint[]
    isAdmin: boolean
  } = $props()

  let tab = $state<Tab>('summary')
  // The library tabs are editors that own their scroll; every other tab is a
  // form or a report that scrolls in the frame.
  const fills = $derived(tab === 'skills' || tab === 'memory')
</script>

<Modal {open} {onClose} title={`${def.displayName} · v${def.currentVersion}`} takeover>
  <!-- Takeover modal: the frame owns the height, so every tab renders at
       the SAME size and scrolls internally — no height jank between tabs. -->
  <div class="flex h-full min-h-0 flex-col">
    <Tabs items={TABS} value={tab} onChange={(t) => (tab = t)} class="shrink-0 border-b border-line pb-2" />

    <!-- THE LIBRARY TABS FILL THE FRAME when it fits: their panes take the
         whole tab height, controls pinned, only the document body scrolling.
         Below the surfaces' floor — a short window — this same region scrolls
         the whole surface, controls reachable at the bottom. The form and
         report tabs always scrolled here; the editors join them only when
         they must. -->
    <div class="min-h-0 flex-1 overflow-y-auto pt-4">
      <!-- Tab-pane grammar: the incoming pane rises in, the old one is simply
           replaced (no exit). No AutoHeight — the takeover frame is fixed-height
           by design. No stagger — several panes are dense data lists. -->
      {#key tab}
        <div in:fly={{ y: 6, duration: 200 }} class={cn(fills && 'h-full')}>
          {#if tab === 'summary'}<SummaryTab {def} {isAdmin} />{/if}
          {#if tab === 'config'}
            {#if isAdmin}
              <AgentConfigForm {def} {endpoints} onSaved={onClose} />
            {:else}
              <ReadOnlyConfig {def} />
            {/if}
          {/if}
          {#if tab === 'skills'}<SkillsLibrary owner={def.slug} ownerLabel={def.displayName ?? def.model} canEdit={isAdmin} surface="well" class="h-full" />{/if}
          {#if tab === 'memory'}
            {#if def.managed}
              <MemoryEditor
                id={def.id}
                label={def.displayName ?? def.model}
                canEdit={isAdmin}
                class="h-full"
                note="It lives in the agent's own container."
                museContext={`The memory of the "${def.slug}" agent (${def.role ?? def.department}).`}
              />
            {:else}
              <EmptyState icon="❖" title="Not managed" hint="Memory reads through the managed container. Migrate this agent first." />
            {/if}
          {/if}
          {#if tab === 'crons'}<CronsPanel agentId={def.id} />{/if}
          {#if tab === 'secrets'}
            {#if isAdmin}<SecretsTab agentId={def.id} agentModel={def.model} agentLabel={def.displayName ?? def.model} />{:else}<div class="text-sm text-muted">Admins only.</div>{/if}
          {/if}
          {#if tab === 'mcp'}<McpTab {def} {isAdmin} />{/if}
          {#if tab === 'versions'}<VersionsTab {def} {isAdmin} />{/if}
        </div>
      {/key}
    </div>
  </div>
</Modal>
