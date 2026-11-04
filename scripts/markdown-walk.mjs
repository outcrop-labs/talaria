// The markdown walk check-docs.mjs links against, on its own so
// check-docs.test.mjs can run it on a scratch tree. check-docs.mjs's header
// says what the walk covers and why.
import { readdirSync } from 'node:fs'
import { join } from 'node:path'

// app-data/ and app-builds/ are the app platform's runtime state at the
// checkout root in dev (talaria-users `app_data_dir`, ui app-build/paths.ts):
// each app's database files and its compiled builds. The database's `pg/`
// directory belongs to the container's user, so once any app has run its
// database, the walk could not read it and `bun run check` died on EACCES.
export const EXEMPT_DIRS = new Set(['node_modules', 'fleet', 'logs', 'playground', 'worktrees', 'app-data', 'app-builds'])
export const EXEMPT_FILES = new Set(['CHANGELOG.md']) // append-only record; frozen links are history

/** Every markdown file under `dir`, minus the exemptions above. A directory
 *  the walk cannot read is skipped and named in `unreadable`, not a crash:
 *  docs live in tracked files, which a checkout can always read, so an
 *  unreadable directory is something a running process owns. */
export function walkMarkdown(dir, acc = [], unreadable = []) {
  let entries
  try {
    entries = readdirSync(dir, { withFileTypes: true })
  } catch (e) {
    if (e?.code !== 'EACCES' && e?.code !== 'EPERM') throw e
    unreadable.push(dir)
    return acc
  }
  for (const entry of entries) {
    if (entry.isDirectory()) {
      if (entry.name.startsWith('.') && entry.name !== '.github' && entry.name !== '.claude') continue
      if (EXEMPT_DIRS.has(entry.name)) continue
      walkMarkdown(join(dir, entry.name), acc, unreadable)
    } else if (entry.name.endsWith('.md')) {
      if (EXEMPT_FILES.has(entry.name)) continue
      acc.push(join(dir, entry.name))
    }
  }
  return acc
}
