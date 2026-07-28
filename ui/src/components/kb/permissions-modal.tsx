import { useEffect, useMemo, useState } from 'react'
import { Globe, Lock, Building2, Bot, X, Check, Copy } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Avatar } from '@/components/ui/avatar'
import { Combobox } from '@/components/ui/combobox'
import { SkeletonRows } from '@/components/ui/skeleton'
import { useUsers } from '@/lib/users'
import { useAgents } from '@/lib/agents'
import { fetchEditors, type EditPolicy, type GrantRole, type KbEditor, type Visibility } from '@/lib/kb'
import { cn } from '@/lib/cn'

// Google-Drive-style sharing: a list of people + agents each with a Viewer/
// Editor role, then a "general access" tier (restricted / org / public).
// Sharing is owner-only; non-owners see it read-only.

const ROLE_OPTIONS = [
  { value: 'viewer', label: 'Viewer', sub: 'Can read it' },
  { value: 'editor', label: 'Editor', sub: 'Can read & edit' },
]
const ACCESS_OPTIONS = [
  { value: 'private', label: 'Restricted', sub: 'Only people you add below' },
  { value: 'org', label: 'Organization', sub: 'Everyone in the workspace' },
  { value: 'public', label: 'Anyone with the link', sub: 'Public on the internet' },
]

export function PermissionsModal({
  open,
  onClose,
  kind,
  id,
  label,
  visibility,
  editPolicy,
  publicSlug,
  canManage,
  inheritable = false,
  inherited = false,
  folderName,
  onSave,
}: {
  open: boolean
  onClose: () => void
  kind: 'docs' | 'spaces' | 'artifacts'
  id: string
  label: string
  visibility: Visibility
  editPolicy: EditPolicy
  publicSlug: string | null
  canManage: boolean
  /** Docs can inherit their folder's access; folders can't. */
  inheritable?: boolean
  inherited?: boolean
  folderName?: string
  onSave: (patch: { visibility: Visibility; editPolicy: EditPolicy; editors: KbEditor[]; permsInherited?: boolean }) => Promise<void>
}) {
  const { data: users = [], isLoading: usersLoading } = useUsers()
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  // Until both principal lists resolve, grants would render as raw ids and the
  // add picker would be empty — hold the list's shape instead.
  const principalsLoading = usersLoading || agentsLoading
  const [vis, setVis] = useState<Visibility>(visibility)
  const [grants, setGrants] = useState<KbEditor[]>([])
  const [inh, setInh] = useState(inherited)
  // General audience role for org/public: editors → edit_policy 'org'.
  const [orgRole, setOrgRole] = useState<GrantRole>(editPolicy === 'org' ? 'editor' : 'viewer')
  const [saving, setSaving] = useState(false)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    if (!open) return
    setVis(visibility)
    setOrgRole(editPolicy === 'org' ? 'editor' : 'viewer')
    setInh(inherited)
    setCopied(false)
    void fetchEditors(kind, id).then(setGrants)
  }, [open, kind, id, visibility, editPolicy, inherited])

  const showInheritBanner = inheritable && inh

  const nameOf = (g: KbEditor) => {
    if (g.principalType === 'user') {
      const u = users.find((x) => x.id === g.principalId)
      return u?.name ?? u?.email ?? g.principalId
    }
    return (fleet?.agents ?? []).find((a) => a.id === g.principalId)?.label ?? g.principalId
  }

  // People/agents not already granted — the "add" picker.
  const addOptions = useMemo(() => {
    const has = new Set(grants.map((g) => `${g.principalType}:${g.principalId}`))
    return [
      ...users.filter((u) => !has.has(`user:${u.id}`)).map((u) => ({ value: `user:${u.id}`, label: u.name ?? u.email ?? u.id, sub: u.email ?? 'Person', icon: <Avatar name={u.name ?? u.email} className="h-5 w-5 text-[9px]" /> })),
      ...(fleet?.agents ?? []).filter((a) => !has.has(`agent:${a.id}`)).map((a) => ({ value: `agent:${a.id}`, label: a.label, sub: `Agent · ${a.role}`, icon: <span className="grid h-5 w-5 place-items-center rounded-full bg-card2 text-muted"><Bot size={12} /></span> })),
    ]
  }, [users, fleet, grants])

  const addGrant = (val: string) => {
    const [type, pid] = [val.slice(0, val.indexOf(':')), val.slice(val.indexOf(':') + 1)]
    if (type !== 'user' && type !== 'agent') return
    setGrants((g) => [...g, { principalType: type, principalId: pid, role: 'viewer' }])
  }
  const setRole = (i: number, role: GrantRole) => setGrants((g) => g.map((x, j) => (j === i ? { ...x, role } : x)))
  const remove = (i: number) => setGrants((g) => g.filter((_, j) => j !== i))

  const save = async () => {
    setSaving(true)
    try {
      if (inheritable && inh) {
        // Still inheriting — just make sure it's marked inherited (reset case).
        await onSave({ visibility: vis, editPolicy: 'owner', editors: [], permsInherited: true })
      } else {
        const editPolicyNext: EditPolicy = vis !== 'private' && orgRole === 'editor' ? 'org' : 'owner'
        await onSave({ visibility: vis, editPolicy: editPolicyNext, editors: grants, ...(inheritable ? { permsInherited: false } : {}) })
      }
      onClose()
    } finally {
      setSaving(false)
    }
  }

  const publicBase = kind === 'artifacts' ? 'a' : kind === 'spaces' ? 'kb/space' : 'kb'
  const publicUrl = publicSlug ? `${typeof window !== 'undefined' ? window.location.origin : ''}/${publicBase}/${publicSlug}` : null
  const AccessIcon = vis === 'public' ? Globe : vis === 'org' ? Building2 : Lock

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Share “${label}”`}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={onClose}>
            {canManage ? 'Cancel' : 'Close'}
          </Button>
          {canManage && (
            <Button size="sm" onClick={() => void save()} disabled={saving}>
              {saving ? 'Saving' : 'Save'}
            </Button>
          )}
        </div>
      }
    >
      <div className="space-y-5">
        {showInheritBanner ? (
          <div className="rounded-xl border border-line-subtle bg-card/50 p-4 text-sm">
            <div className="mb-1 font-medium text-fg">Access follows the folder{folderName ? ` “${folderName}”` : ''}</div>
            <p className="mb-3 text-xs text-muted">
              This document shares whoever the folder is shared with. Customize it to give this doc its own,
              more or less restrictive, access.
            </p>
            {canManage && (
              <Button variant="outline" size="sm" onClick={() => setInh(false)}>
                Customize for this doc
              </Button>
            )}
          </div>
        ) : (
          <>
        {/* People & agents */}
        <div>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">People &amp; agents</div>
          {canManage && (
            <Combobox
              options={addOptions}
              selected={[]}
              onChange={(v) => v[0] && addGrant(v[0])}
              placeholder="Add a person or agent"
              size="sm"
              className="mb-2"
            />
          )}
          {principalsLoading ? (
            <SkeletonRows rows={3} avatar className="px-1 py-2" />
          ) : (
          <div className="space-y-1">
            {/* Owner row (implicit editor) */}
            <div className="flex items-center gap-2.5 rounded-lg px-1 py-1.5">
              <Avatar name={label} className="h-7 w-7 shrink-0 text-[10px]" />
              <span className="min-w-0 flex-1 truncate text-sm text-fg">Owner</span>
              <span className="shrink-0 text-xs text-muted">Owner</span>
            </div>
            {grants.map((g, i) => (
              <div key={`${g.principalType}:${g.principalId}`} className="flex items-center gap-2.5 rounded-lg px-1 py-1">
                {g.principalType === 'user' ? (
                  <Avatar name={nameOf(g)} className="h-7 w-7 shrink-0 text-[10px]" />
                ) : (
                  <span className="grid h-7 w-7 shrink-0 place-items-center rounded-full bg-card2 text-muted"><Bot size={14} /></span>
                )}
                <span className="min-w-0 flex-1 truncate text-sm text-fg">{nameOf(g)}</span>
                {canManage ? (
                  <>
                    <Combobox
                      options={ROLE_OPTIONS}
                      selected={[g.role]}
                      onChange={(v) => v[0] && setRole(i, v[0] as GrantRole)}
                      searchable={false}
                      size="sm"
                      className="w-28 shrink-0"
                    />
                    <button type="button" onClick={() => remove(i)} className="shrink-0 rounded p-1 text-muted hover:text-[color:var(--theme-danger)]" title="Remove">
                      <X size={14} />
                    </button>
                  </>
                ) : (
                  <span className="shrink-0 text-xs capitalize text-muted">{g.role}</span>
                )}
              </div>
            ))}
            {grants.length === 0 && <div className="px-1 py-1 text-xs text-muted">No one else has been added.</div>}
          </div>
          )}
        </div>

        {/* General access */}
        <div className="border-t border-line-subtle pt-4">
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">General access</div>
          <div className="flex items-center gap-2.5">
            <span className={cn('grid h-9 w-9 shrink-0 place-items-center rounded-full', vis === 'private' ? 'bg-card2 text-muted' : 'bg-accent/15 text-accent')}>
              <AccessIcon size={16} />
            </span>
            <Combobox
              options={ACCESS_OPTIONS}
              selected={[vis]}
              onChange={(v) => v[0] && setVis(v[0] as Visibility)}
              searchable={false}
              disabled={!canManage}
              size="sm"
              className="min-w-0 flex-1"
            />
            {vis !== 'private' && (
              <Combobox
                options={ROLE_OPTIONS}
                selected={[orgRole]}
                onChange={(v) => v[0] && setOrgRole(v[0] as GrantRole)}
                searchable={false}
                disabled={!canManage}
                size="sm"
                className="w-28 shrink-0"
              />
            )}
          </div>

          {vis === 'public' && publicUrl && (
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard?.writeText(publicUrl)
                setCopied(true)
              }}
              className="mt-3 flex w-full items-center gap-2 rounded-lg border border-line-subtle px-3 py-2 text-left text-xs text-muted hover:border-[var(--theme-accent-border)]"
            >
              <Globe size={13} className="shrink-0" />
              <code className="min-w-0 flex-1 truncate text-fg">{publicUrl}</code>
              {copied ? <Check size={13} className="shrink-0 text-accent" /> : <Copy size={13} className="shrink-0" />}
            </button>
          )}
          <p className="mt-2 text-[11px] text-muted">Agents only edit when given the Editor role here, never by default.</p>
          {inheritable && canManage && (
            <button type="button" onClick={() => setInh(true)} className="mt-2 text-[11px] text-accent hover:underline">
              Reset to folder defaults
            </button>
          )}
        </div>
          </>
        )}
        {!canManage && <p className="text-[11px] text-muted">Only the owner can change sharing.</p>}
      </div>
    </Modal>
  )
}
