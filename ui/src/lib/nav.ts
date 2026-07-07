// The application menu — two mental modes. WORK is where everyone gets things
// done (chat, channels, boards, inbox); MANAGE is the control plane for the
// people running the platform (fleet, models, compute, cost, audit, admin) and
// is admin-only: average users get their own surfaces (Home, Settings) instead.

export interface NavItem {
  to: string
  label: string
  icon: string
  adminOnly?: boolean
}

// Views an admin can grant/revoke per user. Home and Settings are always
// reachable; admin-only views (the whole Manage section) are gated by role.
export const GATEABLE_VIEWS: { to: string; label: string }[] = [
  { to: '/chat', label: 'Chat' },
  { to: '/channels', label: 'Channels' },
  { to: '/boards', label: 'Boards' },
  { to: '/knowledge', label: 'Knowledge' },
  { to: '/inbox', label: 'Inbox' },
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
      { to: '/', label: 'Home', icon: '◇' },
      { to: '/chat', label: 'Chat', icon: '◈' },
      { to: '/channels', label: 'Channels', icon: '⋕' },
      { to: '/boards', label: 'Boards', icon: '⧉' },
      { to: '/knowledge', label: 'Knowledge', icon: '❖' },
      { to: '/inbox', label: 'Inbox', icon: '⌾' },
    ],
  },
  {
    title: 'Manage',
    adminOnly: true,
    items: [
      { to: '/agents', label: 'Agents', icon: '◍' },
      { to: '/models', label: 'Models', icon: '▤' },
      { to: '/inference', label: 'Compute', icon: '▚' },
      { to: '/cost', label: 'Cost', icon: '⌗' },
      { to: '/activity', label: 'Audit', icon: '⌁' },
      { to: '/alerts', label: 'Alerts', icon: '△' },
    ],
  },
  {
    title: 'System',
    items: [
      { to: '/settings', label: 'Settings', icon: '⚙' },
      { to: '/admin', label: 'Admin', icon: '⛨', adminOnly: true },
    ],
  },
]

/** Routes members can never reach — every item of an admin-only section plus
 *  individually gated items. The nav config is the single source of truth; the
 *  route gate in _app.tsx enforces it beyond just hiding menu entries. */
export const ADMIN_VIEWS: string[] = NAV.flatMap((s) =>
  s.items.filter((i) => s.adminOnly || i.adminOnly).map((i) => i.to),
)
