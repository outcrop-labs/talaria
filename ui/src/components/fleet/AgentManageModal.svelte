<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
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

  type Tab = 'summary' | 'config' | 'skills' | 'memory' | 'crons' | 'secrets' | 'mcp' | 'versions' | 'managers'
  const TABS: TabItem<Tab>[] = [
    { id: 'summary', label: 'Summary' },
    { id: 'config', label: 'Config' },
    { id: 'skills', label: 'Skills' },
    { id: 'memory', label: 'Memory' },
    { id: 'crons', label: 'Crons' },
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
          {#if tab === 'summary'}
            <SummaryTab {def} {canManage} />
            {#if canManage}
              <!-- Below the summary, at the edge of the section it affects: a
                   credential action is read rarely and must not compete with
                   the agent's identity above it. -->
              <div class="mt-6 border-t border-line pt-3 text-sm">
                <div class="flex items-center gap-3">
                  <div class="min-w-0 flex-1">
                    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Credential</div>
                    <div class="font-sans text-xs text-muted">
                      The key this agent presents to Talaria. Rotate it if it may have leaked; the new one is never shown.
                    </div>
                  </div>
                  {#if !rotateOpen}
                    <DangerLink onClick={() => ((rotateOpen = true), (rotated = null))} disabled={rotating}>Rotate credential</DangerLink>
                  {/if}
                </div>
                {#if rotateOpen}
                  <div transition:slide={{ duration: 150 }} class="mt-3 rounded-lg border border-line bg-surface p-3 font-sans text-xs text-muted">
                    <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-fg">Rotate {def.displayName}'s credential?</div>
                    <ul class="list-disc space-y-1 pl-4">
                      <li>The current key stops working the moment you confirm. There is no undo and no grace period.</li>
                      {#if willRoll}
                        <li>
                          The fleet is re-rendered on the new key and, if the agent is running, its container is rolled: it
                          stops (finishing what it can), a fresh one starts in its place, and the roll waits for it to report
                          healthy. Until then the agent is locked out, so a toolkit call or a workbench session in flight on
                          the old key fails and has to be retried; conversations wait for the new container and carry on
                          there. A stopped agent stays stopped and takes the new key when it next starts.
                        </li>
                        <li>If the roll fails, the agent is left on a dead key: roll it again from the roster, or stop and start it.</li>
                      {:else}
                        <li>
                          {def.managed ? 'This agent is retired' : 'This agent is not run by Talaria'}, so nothing is rolled:
                          the new key is rendered into the fleet files and takes effect the next time the agent starts.
                          Anything still presenting the old key is refused from now on.
                        </li>
                      {/if}
                      <li>The audit trail records who rotated it and which key, never the value.</li>
                    </ul>
                    <div class="mt-3 flex items-center gap-2">
                      <Button size="sm" variant="ghost" disabled={rotating} onclick={() => (rotateOpen = false)}>Cancel</Button>
                      <Button size="sm" variant="danger-outline" disabled={rotating} onclick={() => void rotate()}>
                        {rotating ? 'Rotating' : 'Rotate credential'}
                      </Button>
                    </div>
                    {#if rotated && 'error' in rotated}
                      <div transition:slide={{ duration: 150 }} class="mt-2 text-danger">Could not rotate: {rotated.error}</div>
                    {/if}
                  </div>
                {:else if rotated && 'rolling' in rotated}
                  <div transition:slide={{ duration: 150 }} class="mt-2 font-sans text-xs text-success">
                    {rotated.rolling && willRoll
                      ? 'Rotated. The old key is dead; the container is rolling onto the new one and is locked out until the roll completes.'
                      : 'Rotated. The old key is dead; the new one takes effect the next time the agent starts.'}
                  </div>
                {/if}
              </div>
            {/if}
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
              <EmptyState icon="❖" title="Not managed" hint="Memory reads through the managed container. Migrate this agent first." />
            {/if}
          {/if}
          {#if tab === 'crons'}<CronsPanel agentId={def.id} {canManage} />{/if}
          {#if tab === 'secrets'}
            {#if canManage}<SecretsTab agentId={def.id} agentModel={def.model} agentLabel={def.displayName ?? def.model} />{:else}<div class="text-sm text-muted">Only this agent's managers can see its secrets.</div>{/if}
          {/if}
          {#if tab === 'mcp'}<McpTab {def} {canManage} />{/if}
          {#if tab === 'versions'}<VersionsTab {def} {canManage} />{/if}
          {#if tab === 'managers'}<ManagersTab {def} {canManage} />{/if}
        </div>
      {/key}
    </div>
  </div>
</Modal>
