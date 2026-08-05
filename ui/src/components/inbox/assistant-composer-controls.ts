// Shared types for the assistant composer's control rail (see
// AssistantComposerControls.svelte and its picker components).

export type AssistantMode = 'normal' | 'fast' | 'plan'

export interface RailItem {
  id: string
  label: string
  detail?: string
}
