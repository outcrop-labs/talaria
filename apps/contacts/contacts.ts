// Shared bits for the Contacts surfaces: the app slug, the fetch helper bound
// to this app's own server (see server.ts), the document types, and the two
// common reads. The components (ContactsWork/Manage/Settings, ContactModal
// .svelte) import from here so each file stays purely a component.
import { appApi, useAppQuery } from '@talaria/sdk'

export const APP = 'contacts'
export const api = appApi(APP)

export interface Contact {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}
export interface ContactDoc {
  id: string
  data: Contact
  createdAt: string
  updatedAt: string
}

/** `q` is a getter so the query re-keys as the search box changes. */
export const useContacts = (q: () => string) =>
  useAppQuery<{ contacts: ContactDoc[] }>(APP, () => {
    const s = q()
    return `contacts${s ? `?q=${encodeURIComponent(s)}` : ''}`
  })
export const useStages = () => useAppQuery<{ stages: string[] }>(APP, 'config')
