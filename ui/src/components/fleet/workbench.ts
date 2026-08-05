// Shared shapes for the workbench controls inside the agent manage modal
// (WorkbenchControl / WorkbenchTuning / WorkbenchRepos).

export interface WorkbenchProfileLite {
  slug: string
  name: string
  description: string
  harnesses: string[]
  autoAttach: { departments?: string[]; roles?: string[] }
  enabled: boolean
}

export const EFFORTS = [
  ['light', 'Low'],
  ['standard', 'Medium'],
  ['heavy', 'High'],
] as const
