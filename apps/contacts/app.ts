// Contacts — the reference Talaria app. Everything here comes from
// '@talaria/sdk' (+ svelte): Mercury UI kit, session hooks, and fetch helpers
// bound to this app's own server (see server.ts). Three surfaces:
//   work      /x/contacts          the CRM itself
//   manage    /x/contacts/manage   data overview + admin danger zone
//   settings  Settings → Contacts  pipeline stages
import { defineApp } from '@talaria/sdk'
import ContactsWork from './ContactsWork.svelte'
import ContactsManage from './ContactsManage.svelte'
import ContactsSettings from './ContactsSettings.svelte'

export default defineApp({
  work: ContactsWork,
  manage: ContactsManage,
  settings: ContactsSettings,
})
