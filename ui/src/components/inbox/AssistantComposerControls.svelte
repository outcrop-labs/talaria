<script lang="ts">
  import AgentChip from '@/components/chat/AgentChip.svelte'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import type { Attachment } from '@/lib/attachments'
  import type { GatewayModel } from '@/lib/muse.svelte'
  import type { AssistantMode, RailItem } from './assistant-composer-controls'
  import CapabilityPicker from './CapabilityPicker.svelte'
  import HelpButton from './HelpButton.svelte'
  import ModelPicker from './ModelPicker.svelte'
  import ModePicker from './ModePicker.svelte'
  import VoiceButton from './VoiceButton.svelte'

  let {
    agents,
    agentValue,
    onAgentChange,
    models,
    modelValue,
    onModelChange,
    modelsLoading,
    mode,
    onModeChange,
    mcpItems,
    skillItems,
    onAttach,
    onTranscript,
    disabled,
  }: {
    agents: Array<{ id: string; label: string; role?: string }>
    agentValue: string
    onAgentChange: (value: string) => void
    models: GatewayModel[]
    modelValue: string
    onModelChange: (value: string) => void
    modelsLoading: boolean
    mode: AssistantMode
    onModeChange: (mode: AssistantMode) => void
    mcpItems: RailItem[]
    skillItems: RailItem[]
    onAttach: (attachment: Attachment) => void
    onTranscript: (text: string) => void
    disabled: boolean
  } = $props()
</script>

<div class="flex min-w-[608px] items-center gap-1.5">
  <AttachButton {onAttach} {disabled} />
  <AgentChip {agents} value={agentValue} onChange={onAgentChange} class="w-[108px] justify-between" />
  <ModelPicker {models} value={modelValue} onChange={onModelChange} loading={modelsLoading} />
  <ModePicker value={mode} onChange={onModeChange} />
  <CapabilityPicker kind="mcp" items={mcpItems} />
  <CapabilityPicker kind="skills" items={skillItems} />
  <span class="flex-1"></span>
  <HelpButton />
  <VoiceButton {onTranscript} {disabled} />
</div>
