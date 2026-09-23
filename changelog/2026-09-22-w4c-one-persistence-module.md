- **W4c — one persistence module.** `ui/src/lib/persist.ts`
  (`readStored`/`writeStored`/`readFlag`/`writeFlag` + `readText`/`writeText`)
  mirrors `lib/view-memory.ts`'s parse/validate contract and absorbs a refusing
  store on every access; the 12 consumers keep their exact keys, fallback values
  and stored shapes (`'1'`/`'0'`, bare text, `JSON.stringify`). **Three files had
  NO guard at all** (`lib/theme.ts`, `components/setup/UnreadableSecretsBanner.svelte`,
  `routes/app/Artifacts.svelte` — a private-mode or blocked store threw on
  read *and* write) and `lib/sticky-agent.svelte.ts` guarded only its read; all
  four are absorbed now, which is the improvement this wave was for.
