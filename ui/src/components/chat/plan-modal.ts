// The draft-proposal shape shared by PlanModal.svelte and ProposalCard.svelte.
import type { Effort, Priority } from '@/lib/task-const'

export interface Proposal {
  title: string
  description: string
  priority: Priority
  effort: Effort | null
  /** Indices of proposals in this batch that must land first. */
  dependsOn: number[]
  /** Routing labels from the workflow map — trip dispatch classification. */
  tags: string[]
  include: boolean
}
