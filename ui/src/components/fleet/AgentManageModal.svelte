<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
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
  import ManagersTab from '@/components/fleet/ManagersTab.svelte'
  import SummaryTab from '@/components/fleet/SummaryTab.svelte'
  import VersionsTab from '@/components/fleet/VersionsTab.svelte'
  import { rotateAgentKey, type AgentDef, type LlmEndpoint } from '@/lib/fleet-defs'
  import { fly, slide } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import { setUiVoice } from '@/lib/ui-voice'

  type Tab = 'summary' | 'config' | 'skills' | 'memory' | 'crons' | 'secrets' | 'mcp' | 'versions' | 'managers'
  const TABS: TabItem<Tab>[] = [
    { id: 'summary', label: 'Overview' },
    { id: 'config', label: 'Configuration' },
    { id: 'skills', label: 'Skills' },
    { id: 'memory', label: 'Memory' },
    { id: 'crons', label: 'Schedules' },
    { id: 'secrets', label: 'Secrets' },
    { id: 'mcp', label: 'MCP' },
    { id: 'versions', label: 'Versions' },
    // Last on purpose: who owns the agent is read far less often than what
    // it is, and it is the one tab that changes who can open the rest.
    { id: 'managers', label: 'Managers' },
  ]

  // The whole of an agent's internal stack in one modal: model config, skills,
  // memory, MCP servers, version history, and who owns it — no more hopping
  // between top-level pages. Read-only for anyone who does not manage this
  // agent (docs/PERMISSIONS.md, "Agent managers"); `canManage` is the def's
  // own answer, the same one the write routes give.
  let {
    open,
    onClose,
    def,
    endpoints,
    canManage,
  }: {
    open: boolean
    onClose: () => void
    def: AgentDef
    endpoints: LlmEndpoint[]
    canManage: boolean
  } = $props()

  // The agent's settings are one surface with one look, wherever they open
  // from: the roster, or an agent's row in the chat sidebar.
  setUiVoice('sans')

  let tab = $state<Tab>('summary')
  // The library tabs are editors that own their scroll; every other tab is a
  // form or a report that scrolls in the frame.
  const fills = $derived(tab === 'skills' || tab === 'memory')

  // ── Rotate credential ──────────────────────────────────────────────────────
  // The agent's `tak_` key, the one its container presents to Talaria. The
  // trigger is a quiet danger link; the deliberate step is an in-place
  // confirm that says what the route does (fleet_defs_id_rotate_key.rs):
  // the old secret is dead the moment the row is replaced, the fleet is
  // re-rendered, and a running managed agent's container is rolled onto the
  // new one (a stopped one stays stopped and takes it when it next starts).
  // The result lands in the same place, because a toast would be gone a
  // frame after the modal moved on.
  let rotateOpen = $state(false)
  let rotating = $state(false)
  let rotated = $state<{ rolling: boolean } | { error: string } | null>(null)
  // The route answers `rolling` only for a managed agent it found running,
  // and the roll itself only touches a managed AND enabled one that is still
  // running when it recreates; a retired or stopped one gets the next-start
  // sentence.
  const willRoll = $derived(def.managed && def.enabled)
  const rotate = async () => {
    rotating = true
    rotated = null
    const r = await rotateAgentKey(def.id)
    rotating = false
    if (r.error) {
      rotated = { error: r.error }
      return
    }
    rotated = { rolling: r.rolling === true }
    rotateOpen = false
  }
</script>

{#snippet title()}
  <div class="flex min-w-0 items-center gap-3">
    <Avatar name={def.displayName} class="h-9 w-9" />
    <div class="min-w-0">
      <div class="flex min-w-0 items-center gap-2">
        <span class="truncate font-sans text-[15px] font-semibold leading-5 text-fg">{def.displayName}</span>
        <Chip title={`Version ${def.currentVersion}`}>v{def.currentVersion}</Chip>
      </div>
      <div class="truncate font-sans text-[13px] font-normal leading-5 text-muted">{def.role ?? 'No role yet'}</div>
    </div>
  </div>
{/snippet}

<Modal {open} {onClose} {title} width="max-w-5xl" height="h-[88vh]" padded={false}>
  <!-- The frame owns the height, so every tab renders at the SAME size: the
       tab strip is pinned on a full-width hairline and only the pane under
       it scrolls. -->
  <div class="flex h-full min-h-0 flex-col">
    <div class="shrink-0 border-b border-line px-7 pt-1">
      <Tabs items={TABS} value={tab} onChange={(t) => (tab = t)} class="-mb-px" />
    </div>

    <!-- THE LIBRARY TABS FILL THE FRAME when it fits: their panes take the
         whole tab height, controls pinned, only the document body scrolling.
         Below the surfaces' floor — a short window — this same region scrolls
         the whole surface, controls reachable at the bottom. -->
    <div class="min-h-0 flex-1 overflow-y-auto px-7 py-6">
      <!-- Tab-pane grammar: the incoming pane rises in, the old one is simply
           replaced (no exit). No stagger — several panes are dense data lists. -->
      {#key tab}
        <div in:fly={{ y: 6, duration: 200 }} class={cn(fills && 'h-full')}>
          {#if tab === 'summary'}
            <div class="space-y-10">
              <SummaryTab {def} {canManage} />
              {#if canManage}
                <!-- Last on the page: a credential action is read rarely and
                     must not compete with the agent's identity above it. -->
                <SettingsSection title="Credential">
                  <SettingsGroup>
                    <SettingsRow label="Agent key" hint="What this agent presents to Talaria. Rotate it if it may have leaked.">
                      {#if !rotateOpen}
                        <DangerLink onClick={() => ((rotateOpen = true), (rotated = null))} disabled={rotating}>Rotate key</DangerLink>
                      {/if}
                    </SettingsRow>
                    {#if rotateOpen}
                      <div transition:slide={{ duration: 150 }} class="px-5 py-4">
                        <p class="font-sans text-sm font-medium leading-5 text-fg">Rotate {def.displayName}'s key?</p>
                        <ul class="mt-2 list-disc space-y-1 pl-5 font-sans text-[13px] leading-5 text-muted">
                          <li>The current key stops working at once. There is no undo, and the new key is never shown.</li>
                          {#if willRoll}
                            <li>A running agent restarts on a fresh container to pick up the new key. Work in flight on the old key fails and has to be retried.</li>
                            <li>If that restart fails, restart the agent from the roster.</li>
                          {:else}
                            <li>
                              {def.managed ? 'This agent is retired' : 'This agent is not run by Talaria'}, so the new key takes
                              effect the next time it starts.
                            </li>
                          {/if}
                          <li>The audit trail records who rotated it, never the value.</li>
                        </ul>
                        <div class="mt-4 flex items-center gap-2">
                          <Button size="sm" variant="danger-outline" disabled={rotating} onclick={() => void rotate()}>
                            {rotating ? 'Rotating' : 'Rotate key'}
                          </Button>
                          <Button size="sm" variant="ghost" disabled={rotating} onclick={() => (rotateOpen = false)}>Cancel</Button>
                        </div>
                        {#if rotated && 'error' in rotated}
                          <p transition:slide={{ duration: 150 }} class="mt-3 font-sans text-[13px] leading-5 text-danger" role="alert">Could not rotate: {rotated.error}</p>
                        {/if}
                      </div>
                    {:else if rotated && 'rolling' in rotated}
                      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-success">
                        {rotated.rolling && willRoll
                          ? 'Rotated. The old key is dead, and the agent is restarting on the new one.'
                          : 'Rotated. The old key is dead, and the new one takes effect the next time the agent starts.'}
                      </p>
                    {/if}
                  </SettingsGroup>
                </SettingsSection>
              {/if}
            </div>
          {/if}
          {#if tab === 'config'}
            {#if canManage}
              <AgentConfigForm {def} {endpoints} onSaved={onClose} />
            {:else}
              <ReadOnlyConfig {def} />
            {/if}
          {/if}
          {#if tab === 'skills'}<SkillsLibrary owner={def.slug} ownerLabel={def.displayName ?? def.model} canEdit={canManage} surface="well" class="h-full" />{/if}
          {#if tab === 'memory'}
            {#if def.managed}
              <MemoryEditor
                id={def.id}
                label={def.displayName ?? def.model}
                canEdit={canManage}
                class="h-full"
                note="It lives in the agent's own container."
                museContext={`The memory of the "${def.slug}" agent (${def.role ?? def.department}).`}
              />
            {:else}
              <EmptyState title="Memory is not available" hint="Talaria reads memory through the container it runs. This agent runs outside Talaria." />
            {/if}
          {/if}
          {#if tab === 'crons'}<CronsPanel agentId={def.id} {canManage} />{/if}
          {#if tab === 'secrets'}
            {#if canManage}
              <SecretsTab agentId={def.id} agentModel={def.model} agentLabel={def.displayName ?? def.model} />
            {:else}
              <EmptyState title="Secrets are hidden" hint="Only this agent's managers can see its secrets." />
            {/if}
          {/if}
          {#if tab === 'mcp'}<McpTab {def} {canManage} />{/if}
          {#if tab === 'versions'}<VersionsTab {def} {canManage} />{/if}
          {#if tab === 'managers'}<ManagersTab {def} {canManage} />{/if}
        </div>
      {/key}
    </div>
  </div>
</Modal>
