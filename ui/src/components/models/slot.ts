// The Roles pane's list rows, shared between the pane and its detail editor.
//
// The pane MERGED the old Platform tab: a model role and a platform worker
// are the same question — "which model runs this" — asked over two catalogs,
// so they answer it side by side in one library list instead of two tabs
// whose layouts differed for no reason the question could explain. A ROLE is
// a class of activity (research recon, heavy coding) that any number of
// surfaces share; a WORKER is one of Talaria's named internal jobs (Muse,
// Distiller) with its own harness. The `kind` tag is what the detail editor
// narrows on.

export interface ModelRoleRow {
  role: string
  label: string
  hint: string
  wired: boolean
  requires: string[]
}

export interface PlatformAgentRow {
  id: string
  label: string
  job: string
  skills: string[]
  auto: string
  assignable: boolean
}

/** List identity: the kind-prefixed catalog id, unique across both catalogs. */
export type Slot = { kind: 'role'; id: string; row: ModelRoleRow } | { kind: 'agent'; id: string; row: PlatformAgentRow }

/** The short mono spelling of a slot's state: who runs it, or auto. A fixed
 *  worker (the user's own assistant) is neither — it names WHO, because
 *  "auto" would claim a chain it deliberately does not follow. */
export function slotState(slot: Slot, assigned: string | undefined): string {
  if (slot.kind === 'agent' && !slot.row.assignable) return 'your assistant'
  return assigned ?? 'auto'
}

/** Audit 1.6: a role assignment whose model is KNOWN not to be able to do the
 *  work. The server sends only real gaps — an unprobed model produces
 *  nothing, because unknown is not a lack — so anything here is worth a line
 *  of the admin's attention. */
export interface RoleIssue {
  role: string
  model: string
  missing: string[]
  note: string
}
