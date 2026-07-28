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
// reachable; admin-only views (the whole Manage section) are gated by role.
export const GATEABLE_VIEWS: { to: string; label: string }[] = [
  { to: '/comms', label: 'Comms' },
  { to: '/plan', label: 'Plan' },
  { to: '/boards', label: 'Boards' },
  { to: '/research', label: 'Research' },
  { to: '/knowledge', label: 'Knowledge' },
  { to: '/artifacts', label: 'Artifacts' },
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
    title: 'Manage',
    adminOnly: true,
    items: [
      { to: '/agents', label: 'Agents', icon: '◍' },
      { to: '/models', label: 'Models', icon: '▤' },
      { to: '/templates', label: 'Templates', icon: '▣' },
      { to: '/observability', label: 'Observability', icon: '◉' },
    ],
  },
]

/** Routes members can never reach — every item of an admin-only section plus
 *  views that live OFF the sidebar (Admin sits under the user menu now) but
 *  must still be route-gated. */
export const ADMIN_VIEWS: string[] = [
  ...NAV.flatMap((s) => s.items.filter((i) => s.adminOnly || i.adminOnly).map((i) => i.to)),
  '/admin',
]
