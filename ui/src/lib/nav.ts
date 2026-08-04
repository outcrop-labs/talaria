// The application menu — three mental modes. WORK is where everyone gets
// things done (chat, channels, boards, inbox); MANAGE is the control plane for
// the people running the platform (fleet, models, compute, cost, audit);
// SYSTEM holds Settings (everyone) and Admin (role-locked) — moved out of the
// user menu into the sidebar per the Mercury design (spec §5).

import type { LucideIcon } from 'lucide-react'
import {
  Activity,
  BookOpen,
  Bot,
  CalendarRange,
  Cpu,
  FileBox,
  FolderSearch,
  Hexagon,
  Inbox,
  LayoutGrid,
  LayoutTemplate,
  MessageCircle,
  PlugZap,
  Settings,
  Settings2,
  Shield,
} from 'lucide-react'

export interface NavItem {
  to: string
  label: string
  /** Lucide icon component for core items; enabled apps inject string glyphs
   *  (their manifest `icon`) — renderers must handle both. */
  icon: LucideIcon | string
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
      { to: '/', label: 'Inbox', icon: Inbox },
      { to: '/comms', label: 'Comms', icon: MessageCircle },
      { to: '/plan', label: 'Plan', icon: CalendarRange },
      { to: '/boards', label: 'Boards', icon: LayoutGrid },
      { to: '/research', label: 'Research', icon: FolderSearch },
      { to: '/knowledge', label: 'Knowledge', icon: BookOpen },
      { to: '/artifacts', label: 'Artifacts', icon: FileBox },
    ],
  },
  {
    // Not a blanket admin section anymore: members see whichever Manage views
    // they've been granted (deniedViews computes the default-denied set).
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: Bot },
      { to: '/models', label: 'Models', icon: Cpu },
      { to: '/mcp', label: 'MCP', icon: PlugZap },
      { to: '/templates', label: 'Templates', icon: LayoutTemplate },
      { to: '/studio', label: 'Agent Studio', icon: Settings2 },
      { to: '/observability', label: 'Observability', icon: Activity },
      { to: '/apps', label: 'Apps', icon: Hexagon },
    ],
  },
  {
    // Settings is always reachable (never gateable); Admin stays role-locked
    // via adminOnly (it feeds ADMIN_VIEWS below, same as before the move).
    title: 'System',
    items: [
      { to: '/settings', label: 'Settings', icon: Settings },
      { to: '/admin', label: 'Admin', icon: Shield, adminOnly: true },
    ],
  },
]

/** Routes members can never reach regardless of grants. Manage views moved to
 *  the grantable set; Admin (now in the sidebar SYSTEM section) stays
 *  role-locked via its adminOnly flag. */
export const ADMIN_VIEWS: string[] = [
  ...new Set([
    ...NAV.flatMap((s) => s.items.filter((i) => s.adminOnly || i.adminOnly).map((i) => i.to)),
    '/admin',
  ]),
]
