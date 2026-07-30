// The application menu — two mental modes. WORK is where everyone gets things
// done (chat, channels, boards, inbox); MANAGE is the control plane for the
// people running the platform (fleet, models, compute, cost, audit) and is
// admin-only. Settings and Admin live under the USER MENU, not the sidebar —
// the rail is for work surfaces only.

export interface NavItem {
  to: string
  label: string
  icon: string
  adminOnly?: boolean
}

// Views an admin can grant/revoke per user. Home and Settings are always
// reachable. Work views default ALLOWED (denials stored per user); Manage
// views default DENIED for members (explicit allows stored per user) — same
// checklist in Admin → People, opposite resting state.
export const GATEABLE_VIEWS: { to: string; label: string }[] = [
  { to: '/comms', label: 'Comms' },
  { to: '/plan', label: 'Plan' },
  { to: '/boards', label: 'Boards' },
  { to: '/research', label: 'Research' },
  { to: '/knowledge', label: 'Knowledge' },
  { to: '/artifacts', label: 'Artifacts' },
]

/** Manage-section views a member can be granted (pairs with the fine-grained
 *  permissions: view access opens the door, permissions gate the actions). */
export const MANAGE_VIEWS: { to: string; label: string }[] = [
  { to: '/agents', label: 'Agents' },
  { to: '/models', label: 'Models' },
  { to: '/mcp', label: 'MCP' },
  { to: '/templates', label: 'Templates' },
  { to: '/studio', label: 'Agent Studio' },
  { to: '/observability', label: 'Observability' },
  { to: '/apps', label: 'Apps' },
]

export interface NavSection {
  title: string
  items: NavItem[]
  /** The whole section is role-gated (hidden + route-bounced for members). */
  adminOnly?: boolean
}

export const NAV: NavSection[] = [
  {
    title: 'Work',
    items: [
      { to: '/', label: 'Inbox', icon: '▽' },
      { to: '/comms', label: 'Comms', icon: '◈' },
      { to: '/plan', label: 'Plan', icon: '⊞' },
      { to: '/boards', label: 'Boards', icon: '⧉' },
      { to: '/research', label: 'Research', icon: '◎' },
      { to: '/knowledge', label: 'Knowledge', icon: '❖' },
      { to: '/artifacts', label: 'Artifacts', icon: '◆' },
    ],
  },
  {
    // Not a blanket admin section anymore: members see whichever Manage views
    // they've been granted (deniedViews computes the default-denied set).
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: '◍' },
      { to: '/models', label: 'Models', icon: '▤' },
      { to: '/mcp', label: 'MCP', icon: '⌁' },
      { to: '/templates', label: 'Templates', icon: '▣' },
      { to: '/studio', label: 'Agent Studio', icon: '⚙' },
      { to: '/observability', label: 'Observability', icon: '◉' },
      { to: '/apps', label: 'Apps', icon: '⬡' },
    ],
  },
]

/** Routes members can never reach regardless of grants. Manage views moved to
 *  the grantable set; Admin (under the user menu) stays role-locked. */
export const ADMIN_VIEWS: string[] = [
  ...NAV.flatMap((s) => s.items.filter((i) => s.adminOnly || i.adminOnly).map((i) => i.to)),
  '/admin',
]
