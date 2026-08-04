// Contacts — the reference Talaria app. Everything here comes from
// '@talaria/sdk' (+ react): Mercury UI kit, session hooks, and fetch helpers
// bound to this app's own server (see server.ts). Three surfaces:
//   work      /x/contacts          the CRM itself
//   manage    /x/contacts/manage   data overview + admin danger zone
//   settings  Settings → Contacts  pipeline stages
import { useState } from 'react'
import {
  defineApp,
  appApi,
  useAppQuery,
  useAppInvalidate,
  useMe,
  Button,
  Input,
  Textarea,
  Select,
  Modal,
  Chip,
  DangerLink,
  EmptyState,
  Panel,
  SectionHeader,
  SkeletonRows,
  confirm,
  alert,
} from '@talaria/sdk'

const APP = 'contacts'
const api = appApi(APP)

interface Contact {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}
interface ContactDoc {
  id: string
  data: Contact
  createdAt: string
  updatedAt: string
}

const useContacts = (q: string) => useAppQuery<{ contacts: ContactDoc[] }>(APP, `contacts${q ? `?q=${encodeURIComponent(q)}` : ''}`)
const useStages = () => useAppQuery<{ stages: string[] }>(APP, 'config')

// ── Work surface ───────────────────────────────────────────────────────────
function ContactsWork() {
  const [q, setQ] = useState('')
  const [stageFilter, setStageFilter] = useState<string | null>(null)
  const [editing, setEditing] = useState<ContactDoc | 'new' | null>(null)
  const { data, isLoading } = useContacts(q)
  const { data: cfg } = useStages()
  const invalidate = useAppInvalidate(APP)
  const stages = cfg?.stages ?? []

  const contacts = (data?.contacts ?? []).filter((c) => !stageFilter || c.data.stage === stageFilter)

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto w-full max-w-3xl">
        <div className="mb-6 flex items-center gap-3">
          <h1 className="flex-1 font-sans text-lg font-semibold text-fg">Contacts</h1>
          <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search…" size="sm" className="w-48" />
          <Button size="sm" onClick={() => setEditing('new')}>
            New contact
          </Button>
        </div>

        {stages.length > 0 && (
          <div className="mb-4 flex flex-wrap gap-1.5">
            {[null, ...stages].map((s) => (
              <Chip key={s ?? 'all'} onSelect={() => setStageFilter(s)} selected={stageFilter === s} className="capitalize">
                {s ?? 'all'}
              </Chip>
            ))}
          </div>
        )}

        {isLoading ? (
          <SkeletonRows rows={5} />
        ) : contacts.length === 0 ? (
          <EmptyState icon="☏" title={q || stageFilter ? 'No matches' : 'No contacts yet'} hint={q || stageFilter ? undefined : 'Add your first contact to start the pipeline'} />
        ) : (
          <ul className="divide-y divide-line overflow-hidden rounded-lg border border-line bg-panel">
            {contacts.map((c) => (
              <li
                key={c.id}
                onClick={() => setEditing(c)}
                className="flex cursor-pointer items-center gap-3 px-4 py-3 transition-colors duration-120 hover:bg-hover"
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium text-fg">{c.data.name}</div>
                  <div className="truncate text-xs text-muted">
                    {[c.data.company, c.data.email].filter(Boolean).join(' · ') || '—'}
                  </div>
                </div>
                {c.data.stage && <Chip className="capitalize">{c.data.stage}</Chip>}
              </li>
            ))}
          </ul>
        )}
      </div>

      {editing && (
        <ContactModal
          doc={editing === 'new' ? null : editing}
          stages={stages}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void invalidate()
          }}
        />
      )}
    </div>
  )
}

function ContactModal({ doc, stages, onClose, onSaved }: { doc: ContactDoc | null; stages: string[]; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState<Contact>(doc?.data ?? { name: '', stage: stages[0] })
  const [busy, setBusy] = useState(false)
  const set = (k: keyof Contact) => (e: { target: { value: string } }) => setForm((f) => ({ ...f, [k]: e.target.value }))

  const save = async () => {
    if (!form.name?.trim()) return
    setBusy(true)
    try {
      if (doc) await api.patch(`contacts/${doc.id}`, form)
      else await api.post('contacts', form)
      onSaved()
    } catch (e) {
      void alert({ title: 'Could not save', message: (e as Error).message })
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!doc) return
    if (!(await confirm({ title: `Delete ${doc.data.name}?`, message: 'This cannot be undone.', danger: true }))) return
    setBusy(true)
    try {
      await api.del(`contacts/${doc.id}`)
      onSaved()
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open onClose={onClose} title={doc ? doc.data.name : 'New contact'}>
      <div className="space-y-3">
        <Input value={form.name ?? ''} onChange={set('name')} placeholder="Name" autoFocus />
        <div className="flex gap-2">
          <Input value={form.company ?? ''} onChange={set('company')} placeholder="Company" className="flex-1" />
          <Input value={form.email ?? ''} onChange={set('email')} placeholder="Email" className="flex-1" />
        </div>
        <Select value={form.stage ?? ''} onChange={set('stage')}>
          {stages.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </Select>
        <Textarea value={form.notes ?? ''} onChange={set('notes')} placeholder="Notes" rows={4} />
        <div className="flex items-center justify-between pt-1">
          {doc ? (
            // Destructive trigger: quiet mono link → safety orange, paired with
            // the confirm() step — never an orange fill (spec §8).
            <DangerLink disabled={busy} onClick={() => void remove()}>
              Delete
            </DangerLink>
          ) : (
            <span />
          )}
          <div className="flex gap-2">
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button disabled={busy || !form.name?.trim()} onClick={() => void save()}>
              {doc ? 'Save' : 'Create'}
            </Button>
          </div>
        </div>
      </div>
    </Modal>
  )
}

// ── Manage surface ─────────────────────────────────────────────────────────
function ContactsManage() {
  const { data: me } = useMe()
  const { data: stats, refetch } = useAppQuery<{ total: number; byStage: Record<string, number> }>(APP, 'stats')
  const invalidate = useAppInvalidate(APP)

  const wipe = async () => {
    if (!(await confirm({ title: 'Wipe ALL contacts data?', message: 'Every contact and setting this app stored is deleted. This cannot be undone.', confirmLabel: 'Wipe everything', danger: true }))) return
    try {
      await api.post('wipe')
      await invalidate()
      await refetch()
    } catch (e) {
      void alert({ title: 'Could not wipe', message: (e as Error).message })
    }
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto w-full max-w-2xl space-y-6">
        <h1 className="font-sans text-lg font-semibold text-fg">Contacts data</h1>
        {/* §8 stat block: big sans numeral + 10px mono uppercase label. */}
        <Panel>
          <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Contacts</div>
          <div className="mt-1.5 font-sans text-2xl font-semibold text-fg">{stats?.total ?? 0}</div>
          <div className="mt-3 flex flex-wrap gap-2">
            {Object.entries(stats?.byStage ?? {}).map(([stage, n]) => (
              <Chip key={stage} className="capitalize">
                {stage}: {n}
              </Chip>
            ))}
          </div>
        </Panel>
        {me?.role === 'admin' && (
          // §8 danger zone: orange hairline panel, orange mono header, right
          // meta, muted body — destructive button stays an orange outline.
          <Panel className="border-danger/40">
            <div className="mb-3 flex items-baseline justify-between gap-2">
              <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Danger zone</span>
              <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
            </div>
            <p className="mb-3 text-xs text-muted">Remove everything this app has stored.</p>
            <Button variant="danger" size="sm" onClick={() => void wipe()}>
              Wipe all data
            </Button>
          </Panel>
        )}
      </div>
    </div>
  )
}

// ── Settings surface ───────────────────────────────────────────────────────
function ContactsSettings() {
  const { data: cfg, refetch } = useStages()
  const [draft, setDraft] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const value = draft ?? (cfg?.stages ?? []).join(', ')

  const save = async () => {
    setBusy(true)
    try {
      await api.put('config', { stages: value.split(',').map((s) => s.trim().toLowerCase()).filter(Boolean) })
      setDraft(null)
      await refetch()
    } catch (e) {
      void alert({ title: 'Could not save stages', message: (e as Error).message })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel as="section">
      <SectionHeader title="Pipeline stages" className="mb-1" />
      <p className="mb-3 text-xs text-muted">Comma-separated, in order. Shared by everyone using Contacts.</p>
      <div className="flex gap-2">
        <Input value={value} onChange={(e) => setDraft(e.target.value)} className="flex-1" />
        <Button disabled={busy || draft === null} onClick={() => void save()}>
          Save
        </Button>
      </div>
    </Panel>
  )
}

export default defineApp({
  work: ContactsWork,
  manage: ContactsManage,
  settings: ContactsSettings,
})
