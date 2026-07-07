import { useEffect, useState } from 'react'
import { Globe } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { useUsers } from '@/lib/users'
import { useAgents } from '@/lib/agents'
import { fetchEditors, type EditPolicy, type KbEditor, type Visibility } from '@/lib/kb'

// One sharing dialog for both docs and folders — read visibility, edit policy,
// and (when restricted) the explicit list of human + agent editors. Sharing is
// owner-only; non-owners see it read-only.
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
  onSave,
}: {
  open: boolean
  onClose: () => void
  kind: 'docs' | 'spaces'
  id: string
  label: string
  visibility: Visibility
  editPolicy: EditPolicy
  publicSlug: string | null
  canManage: boolean
  onSave: (patch: { visibility: Visibility; editPolicy: EditPolicy; editors: KbEditor[] }) => Promise<void>
}) {
  const { data: users = [] } = useUsers()
  const { data: fleet } = useAgents()
  const [vis, setVis] = useState<Visibility>(visibility)
  const [policy, setPolicy] = useState<EditPolicy>(editPolicy)
  const [editors, setEditors] = useState<KbEditor[]>([])
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open) return
    setVis(visibility)
    setPolicy(editPolicy)
    void fetchEditors(kind, id).then(setEditors)
  }, [open, kind, id, visibility, editPolicy])

  const userIds = editors.filter((e) => e.principalType === 'user').map((e) => e.principalId)
  const agentIds = editors.filter((e) => e.principalType === 'agent').map((e) => e.principalId)
  const setUserIds = (ids: string[]) =>
    setEditors([...ids.map((principalId) => ({ principalType: 'user' as const, principalId })), ...editors.filter((e) => e.principalType === 'agent')])
  const setAgentIds = (ids: string[]) =>
    setEditors([...editors.filter((e) => e.principalType === 'user'), ...ids.map((principalId) => ({ principalType: 'agent' as const, principalId }))])

  const save = async () => {
    setSaving(true)
    try {
      await onSave({ visibility: vis, editPolicy: policy, editors })
      onClose()
    } finally {
      setSaving(false)
    }
  }

  const publicUrl = publicSlug ? `${typeof window !== 'undefined' ? window.location.origin : ''}/${kind === 'spaces' ? 'kb/space' : 'kb'}/${publicSlug}` : null

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
              {saving ? 'Saving…' : 'Save'}
            </Button>
          )}
        </div>
      }
    >
      <div className="space-y-4">
        <div>
          <div className="mb-1 text-xs font-medium text-fg">Who can see it</div>
          <Select value={vis} disabled={!canManage} onChange={(e) => setVis(e.target.value as Visibility)} className="w-full">
            <option value="private">Private — only me</option>
            <option value="org">Organization — everyone in the workspace</option>
            <option value="public">Public — anyone with the link</option>
          </Select>
          {vis === 'public' && publicUrl && (
            <div className="mt-2 flex items-center gap-2 text-xs text-muted">
              <Globe size={12} /> <code className="truncate text-fg">{publicUrl}</code>
            </div>
          )}
        </div>

        <div>
          <div className="mb-1 text-xs font-medium text-fg">Who can edit it</div>
          <Select value={policy} disabled={!canManage} onChange={(e) => setPolicy(e.target.value as EditPolicy)} className="w-full">
            <option value="owner">Only me</option>
            <option value="org">Anyone who can see it</option>
            <option value="restricted">Specific people &amp; agents</option>
          </Select>
          <p className="mt-1 text-[11px] text-muted">Agents can only edit when you list them here, even under “anyone.”</p>
        </div>

        {policy === 'restricted' && (
          <div className="space-y-2">
            <Combobox
              options={users.map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id }))}
              selected={userIds}
              onChange={setUserIds}
              multiple
              size="sm"
              placeholder="People who can edit"
              className="w-full"
            />
            <Combobox
              options={(fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))}
              selected={agentIds}
              onChange={setAgentIds}
              multiple
              size="sm"
              placeholder="Agents who can edit"
              className="w-full"
            />
          </div>
        )}
        {!canManage && <p className="text-[11px] text-muted">Only the owner can change sharing.</p>}
      </div>
    </Modal>
  )
}
