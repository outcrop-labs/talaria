#!/usr/bin/env node
// Seed a workspace with a day's worth of work, then open and follow a daily
// brief over it.
//
// WHAT THIS IS FOR. The brief is a read model over four sources — approvals,
// tickets, DMs and notifications — so on an empty install it correctly renders
// "nothing is waiting on you", which is the one state that tells you nothing
// about whether the surface works. This writes a realistic morning, then walks
// the day forward in three passes so the append-only behaviour is visible:
// things appear, things change, things resolve, and NOTHING is rewritten.
//
// IT IS ADDITIVE AND IT IS IDEMPOTENT. Every insert is keyed on a marker
// (`SEED_TAG`) and re-running replaces only what an earlier run of this script
// wrote. It never touches a row it did not create, because the machine you run
// this on is somebody's dev workspace and a seed script that truncates is a
// seed script that eventually truncates the wrong database.
//
// Usage:
//   node scripts/seed-daily-brief.mjs                 # seed + open + 2 sweeps
//   node scripts/seed-daily-brief.mjs --user you@x.io  # pick the owner
//   node scripts/seed-daily-brief.mjs --clean          # remove seeded rows only
//
// NO GATEWAY IS FINE. Opening a brief calls the owner's assistant for the lede,
// and once more per sweep for the delta note. On a machine with no gateway
// running, those calls fail, `fallbackLede` writes a counted sentence instead,
// and every other part of the fixture is unaffected — which is the behaviour
// the surface is supposed to have, so seeing it here is not a broken run.
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const ROOT = join(HERE, '..')
const SEED_TAG = 'talaria-seed:daily-brief'

// `postgres` and `vite` are the UI package's dependencies, and this script
// lives outside it — a bare `import 'postgres'` resolves from `scripts/` and
// finds nothing. Resolving through `ui/package.json` uses the same copy the
// server does rather than asking for a second one at the root, which is how a
// tooling script ends up pinning its own driver version.
const fromUi = createRequire(join(ROOT, 'ui/package.json'))
const importFromUi = async (name) => import(pathToFileURL(fromUi.resolve(name)).href)

const { default: postgres } = await importFromUi('postgres')

// ── Environment ──────────────────────────────────────────────────────────────

function databaseUrl() {
  if (process.env.DATABASE_URL) return process.env.DATABASE_URL
  // `ui/.env` is what `talaria setup` writes and what the dev server reads.
  // Parsed rather than sourced so this script works from any shell.
  for (const candidate of [join(ROOT, 'ui/.env'), join(ROOT, '.env')]) {
    try {
      const line = readFileSync(candidate, 'utf8')
        .split('\n')
        .find((l) => l.startsWith('DATABASE_URL='))
      if (line) return line.slice('DATABASE_URL='.length).trim()
    } catch {
      // Next candidate.
    }
  }
  throw new Error('DATABASE_URL is not set and no ui/.env was found')
}

const args = process.argv.slice(2)
const flag = (name) => args.includes(`--${name}`)
const value = (name) => {
  const i = args.indexOf(`--${name}`)
  return i === -1 ? null : (args[i + 1] ?? null)
}

const sql = postgres(databaseUrl(), { max: 4, idle_timeout: 5, onnotice: () => {} })

// ── The cast ─────────────────────────────────────────────────────────────────
//
// Seeded teammates are real user rows, because a DM needs two members and an
// unread count needs somebody who is not you. Their `sub` carries the tag so
// `--clean` can find them again without guessing from the email domain.

const TEAMMATES = [
  { handle: 'priya', name: 'Priya Raman', email: 'priya@seed.talaria.test' },
  { handle: 'dana', name: 'Dana Okafor', email: 'dana@seed.talaria.test' },
  { handle: 'mitchell', name: 'Mitchell Vance', email: 'mitchell@seed.talaria.test' },
]

/** Tickets, in the states the brief's task source actually looks for.
 *
 *  `bucket` names WHY each one is here, and the set is chosen to cover every
 *  branch of `taskItems`: a review waiting on a human, work that is blocked, a
 *  run that failed, and untriaged inbox items. A seed that only wrote "todo"
 *  tickets would render an empty brief and look like a bug in the brief. */
const TICKETS = [
  {
    key: 'ledger',
    title: 'Ledger migration is blocked on the vendor sandbox',
    description:
      'The migration job cannot reach the vendor sandbox — every call returns 403 since their key rotation on Friday. The agent has stopped rather than half-migrating the ledger.',
    status: 'blocked',
    priority: 'high',
    errorMessage: 'vendor sandbox returned 403 for all requests (key rotated 2026-08-14)',
    dueInDays: 0,
  },
  {
    key: 'webhook',
    title: 'Vendor webhook signature check',
    description:
      'Agent work is finished: HMAC verification on the inbound webhook, with the replay window and a test for a tampered body. Waiting on a human to sign it off.',
    status: 'quality_review',
    priority: 'high',
    outcome: 'Added HMAC-SHA256 verification, a 5-minute replay window, and 4 tests. All green.',
  },
  {
    key: 'backfill',
    title: 'Backfill the audit log for July',
    description: 'The July rows are missing an actor on 1,240 entries. Backfill from the request log before the retention window closes.',
    status: 'failed',
    priority: 'high',
    errorMessage: 'the backfill ran out of memory at row 840,000 — needs batching',
    dueInDays: -3,
  },
  {
    key: 'onboarding',
    title: 'Onboarding check-in flow',
    description: 'Someone needs to decide whether this is a this-sprint thing or next. Filed from the standup notes.',
    status: 'inbox',
    priority: 'medium',
  },
  {
    key: 'landing',
    title: 'Landing page follow-up',
    description: 'Copy is approved; the page still needs the pricing table and the new screenshots.',
    status: 'inbox',
    priority: 'high',
    dueInDays: 1,
  },
]

const DMS = [
  {
    who: 'priya',
    messages: [
      'morning! did the rollback window ever get decided? I need to know before I cut the release branch',
      "if it's still open I'll assume 30 minutes and we can widen it later",
    ],
  },
  {
    who: 'mitchell',
    messages: ['are we still pushing the Mercury launch to Wednesday? Alejandro needs an answer today to book the slot'],
  },
]

// Notifications carry REAL hrefs, or the brief lines built from them are
// unfollowable — the row is the only source the brief has. Two of the four
// below therefore point at things this script just seeded (the platform
// channel and the landing ticket), which is also what production does:
// `notifyUserMentions` links `/comms/channel/<id>` and `notifyTaskUsers`
// links the ticket, and the brief's cross-source dedupe then folds those
// notifications into the channel/task lines instead of double-listing them.
// The other two land on real top-level surfaces (`/research`, `/fleet`).
const notificationSet = (refs) => [
  {
    kind: 'mention',
    title: 'Priya mentioned you in #platform',
    body: '@you — the rollback window question is still open, can you weigh in before standup?',
    href: `/comms/channel/${refs.platformChannelId}`,
  },
  {
    kind: 'task-assigned',
    title: 'You were assigned "Landing page follow-up"',
    body: 'Dana assigned this to you and flagged it as needed for the Thursday review.',
    href: `/boards/${refs.boardId}/${refs.taskIds.get('landing')}`,
  },
  {
    kind: 'research',
    title: 'Research finished: competitive pricing sweep',
    body: 'Six sources read, three pricing changes since May. Report is ready and unread.',
    href: '/research',
  },
  {
    kind: 'agent-problem',
    title: 'atlas could not reach the vendor sandbox',
    body: 'Three consecutive failures on the same credential. The agent has stopped rather than retrying into a rate limit.',
    href: '/fleet',
  },
]

// ── Helpers ──────────────────────────────────────────────────────────────────

const log = (...parts) => console.log('[seed]', ...parts)

async function ownerUser() {
  const wanted = value('user')
  const rows = wanted
    ? await sql`select id, email, name, role from users where email = ${wanted}`
    : await sql`select id, email, name, role from users where sub not like ${`${SEED_TAG}%`} order by created_at asc limit 1`
  if (!rows[0]) throw new Error(wanted ? `no user with email ${wanted}` : 'no users in this database — sign in once first')
  return rows[0]
}

async function clean() {
  // ORDER MATTERS ONLY BECAUSE OF THE CASCADES WE DO NOT HAVE. Tasks and
  // channels are removed before the users that authored them, so nothing is
  // left pointing at a deleted row.
  const boards = await sql`select id from boards where name like ${'%' + SEED_TAG + '%'}`
  for (const b of boards) await sql`delete from tasks where board_id = ${b.id}`
  await sql`delete from boards where name like ${'%' + SEED_TAG + '%'}`

  const seeded = await sql`select id from users where sub like ${`${SEED_TAG}%`}`
  for (const u of seeded) {
    const channels = await sql`select channel_id from channel_members where user_id = ${u.id}`
    for (const c of channels) {
      await sql`delete from channel_messages where channel_id = ${c.channel_id}`
      await sql`delete from channel_members where channel_id = ${c.channel_id}`
      await sql`delete from channels where id = ${c.channel_id}`
    }
  }
  await sql`delete from users where sub like ${`${SEED_TAG}%`}`
  await sql`delete from notifications where body like ${'%' + SEED_TAG + '%'} or title like ${'%' + SEED_TAG + '%'}`
  log('removed everything this script had written')
}

async function seedTeammates() {
  const out = new Map()
  for (const mate of TEAMMATES) {
    const rows = await sql`
      insert into users (sub, email, name, role)
      values (${`${SEED_TAG}:${mate.handle}`}, ${mate.email}, ${mate.name}, 'member')
      on conflict (sub) do update set name = excluded.name, email = excluded.email
      returning id, name, email
    `
    out.set(mate.handle, rows[0])
  }
  log(`${out.size} teammates`)
  return out
}

async function seedBoard(owner, mates) {
  const name = `Platform · ${SEED_TAG}`
  const existing = await sql`select id from boards where name = ${name}`
  const board =
    existing[0] ??
    (
      await sql`
        insert into boards (name, owner_id, ticket_prefix) values (${name}, ${owner.id}, 'PLT') returning id
      `
    )[0]

  await sql`insert into board_members (board_id, user_id, role) values (${board.id}, ${owner.id}, 'owner') on conflict do nothing`
  for (const mate of mates.values()) {
    await sql`insert into board_members (board_id, user_id, role) values (${board.id}, ${mate.id}, 'editor') on conflict do nothing`
  }

  // No `board_statuses` rows on purpose. The task source has a documented
  // fallback for a board that has not customised its columns — 'inbox' and
  // 'quality_review' are read as triage and review — and seeding the default
  // path is what makes this fixture representative of a fresh board.
  await sql`delete from tasks where board_id = ${board.id}`
  let no = 0
  const ids = new Map()
  for (const t of TICKETS) {
    const due = t.dueInDays === undefined ? null : new Date(Date.now() + t.dueInDays * 86_400_000)
    const rows = await sql`
      insert into tasks (board_id, title, description, status, priority, ticket_no, due_date,
                         error_message, outcome, created_by, created_at, updated_at)
      values (${board.id}, ${t.title}, ${t.description}, ${t.status}, ${t.priority}, ${++no}, ${due},
              ${t.errorMessage ?? null}, ${t.outcome ?? null}, ${owner.email ?? 'user'},
              now() - interval '2 days', now() - interval '3 hours')
      returning id
    `
    ids.set(t.key, rows[0].id)
  }
  await sql`update boards set ticket_seq = ${no} where id = ${board.id}`
  log(`board "${name}" with ${TICKETS.length} tickets`)
  return { board, ids }
}

async function seedDms(owner, mates) {
  for (const dm of DMS) {
    const mate = mates.get(dm.who)
    if (!mate) continue
    const dmKey = [owner.id, mate.id].sort().join(':')
    const existing = await sql`select id from channels where dm_key = ${dmKey}`
    const channel =
      existing[0] ??
      (
        await sql`
          insert into channels (name, kind, dm_key, created_by) values ('', 'dm', ${dmKey}, ${owner.id}) returning id
        `
      )[0]

    await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${owner.id}, 'owner') on conflict do nothing`
    await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${mate.id}, 'owner') on conflict do nothing`
    await sql`delete from channel_messages where channel_id = ${channel.id}`

    let seq = 0
    for (const content of dm.messages) {
      // `author` must be the teammate's email: the unread query excludes
      // messages whose author matches the READER, and matches on that string.
      await sql`
        insert into channel_messages (channel_id, seq, author_type, author, content, status, created_at)
        values (${channel.id}, ${++seq}, 'user', ${mate.email}, ${content}, 'complete', now() - interval '90 minutes')
      `
    }
    await sql`update channels set msg_seq = ${seq}, updated_at = now() - interval '90 minutes' where id = ${channel.id}`
    // The owner has NOT read them — which is the entire point of the fixture.
    await sql`update channel_members set last_read_seq = 0 where channel_id = ${channel.id} and user_id = ${owner.id}`
  }
  log(`${DMS.length} unread DMs`)
}

/** The #platform room the mention notification is ABOUT. Without it the
 *  notification names a channel that does not exist and its brief line points
 *  at `/comms` — a fixture row with no referent, which on the surface reads
 *  as "stuck dummy data". Real, unread, from Priya: the comms source and the
 *  mention notification then say the same thing, and the dedupe folds them
 *  into one followable line. */
async function seedPlatformChannel(owner, mates) {
  const name = 'platform'
  const existing = await sql`select id from channels where name = ${name} and kind = 'channel' and archived_at is null`
  const channel =
    existing[0] ??
    (await sql`
      insert into channels (name, kind, created_by) values (${name}, 'channel', ${owner.id}) returning id
    `)[0]

  await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${owner.id}, 'owner') on conflict do nothing`
  for (const handle of ['priya', 'dana']) {
    const mate = mates.get(handle)
    if (mate) await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${mate.id}, 'editor') on conflict do nothing`
  }
  await sql`delete from channel_messages where channel_id = ${channel.id}`
  const first = (owner.name ?? owner.email ?? 'you').split(/\s+/)[0]
  await sql`
    insert into channel_messages (channel_id, seq, author_type, author, content, status, created_at)
    values (${channel.id}, 1, 'user', ${mates.get('priya')?.email ?? 'priya@seed.talaria.test'},
            ${`@${first} — the rollback window question is still open, can you weigh in before standup?`}, 'complete', now() - interval '75 minutes')
  `
  await sql`update channels set msg_seq = 1, updated_at = now() - interval '75 minutes' where id = ${channel.id}`
  await sql`update channel_members set last_read_seq = 0 where channel_id = ${channel.id} and user_id = ${owner.id}`
  log(`#${name} with Priya’s mention`)
  return channel.id
}

async function seedNotifications(owner, refs) {
  await sql`delete from notifications where user_id = ${owner.id} and body like ${'%' + SEED_TAG + '%'}`
  for (const n of notificationSet(refs)) {
    await sql`
      insert into notifications (user_id, kind, title, body, href, created_at)
      values (${owner.id}, ${n.kind}, ${n.title}, ${`${n.body}\n\n<!-- ${SEED_TAG} -->`}, ${n.href}, now() - interval '4 hours')
    `
  }
  log(`${notificationSet(refs).length} unread notifications`)
}

// ── Walking the day forward ──────────────────────────────────────────────────
//
// The three passes below are the demonstration. Between them the WORLD changes
// — a review gets signed off, a blocked ticket gets unblocked, a new DM lands —
// and the brief is asked to notice. What must be true afterwards is that the
// document has grown and nothing in it has been edited, which the summary at
// the end asserts rather than merely prints.

async function advanceWorld(step, owner, mates, ids) {
  if (step === 1) {
    // THE STATE THE OLD SOURCE COULD NOT SEE: Mitchell's DM gets READ and not
    // answered. Under the unread-based source this made the line vanish and the
    // brief announced it done; the line must now stay open, saying so.
    const mitchell = mates.get('mitchell')
    if (mitchell) {
      const key = [owner.id, mitchell.id].sort().join(':')
      const ch = await sql`select id, msg_seq from channels where dm_key = ${key}`
      if (ch[0]) {
        await sql`
          update channel_members set last_read_seq = ${ch[0].msg_seq}
          where channel_id = ${ch[0].id} and user_id = ${owner.id}
        `
      }
    }
    // Priya's thread gets ANSWERED, by the owner's own hand.
    const priya = mates.get('priya')
    if (priya) {
      const key = [owner.id, priya.id].sort().join(':')
      const ch = await sql`select id, msg_seq from channels where dm_key = ${key}`
      if (ch[0]) {
        const seq = (ch[0].msg_seq ?? 0) + 1
        await sql`
          insert into channel_messages (channel_id, seq, author_type, author, content, status)
          values (${ch[0].id}, ${seq}, 'user', ${owner.email ?? owner.name ?? 'user'},
                  'rollback window is 30 minutes — go ahead and cut the branch', 'complete')
        `
        await sql`update channels set msg_seq = ${seq}, updated_at = now() where id = ${ch[0].id}`
      }
    }
    // The review is signed off, and a new question arrives.
    await sql`update tasks set status = 'done', updated_at = now() where id = ${ids.get('webhook')}`
    const dana = mates.get('dana')
    const dmKey = [owner.id, dana.id].sort().join(':')
    const rows = await sql`select id, msg_seq from channels where dm_key = ${dmKey}`
    const channel =
      rows[0] ??
      (await sql`insert into channels (name, kind, dm_key, created_by) values ('', 'dm', ${dmKey}, ${owner.id}) returning id, msg_seq`)[0]
    await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${owner.id}, 'owner') on conflict do nothing`
    await sql`insert into channel_members (channel_id, user_id, role) values (${channel.id}, ${dana.id}, 'owner') on conflict do nothing`
    const seq = (channel.msg_seq ?? 0) + 1
    await sql`
      insert into channel_messages (channel_id, seq, author_type, author, content, status)
      values (${channel.id}, ${seq}, 'user', ${dana.email}, 'can I start creator outreach today, or do you want to look at the shortlist first?', 'complete')
    `
    await sql`update channels set msg_seq = ${seq}, updated_at = now() where id = ${channel.id}`
    await sql`update channel_members set last_read_seq = 0 where channel_id = ${channel.id} and user_id = ${owner.id}`
    return 'read Mitchell without answering, replied to Priya, signed off the webhook review, and Dana asked a new question'
  }

  // DELEGATION: the owner hands Dana's thread to their assistant. The next
  // sweep drafts a reply AND sends it, because a grant exists — where every
  // other waiting thread only gets a draft parked for approval.
  const dana = mates.get('dana')
  if (dana) {
    const key = [owner.id, dana.id].sort().join(':')
    const ch = await sql`select id from channels where dm_key = ${key}`
    if (ch[0]) {
      await sql`
        insert into assistant_reply_grants (user_id, channel_id) values (${owner.id}, ${ch[0].id})
        on conflict do nothing
      `
    }
  }

  // The blocked ticket moves, and one notification is dealt with.
  await sql`
    update tasks set status = 'quality_review', error_message = null,
                     outcome = 'Vendor issued a new sandbox key; the migration completed on the retry.',
                     updated_at = now()
    where id = ${ids.get('ledger')}
  `
  await sql`update notifications set read_at = now() where user_id = ${owner.id} and kind = 'agent-problem'`
  return 'handed Dana’s thread to the assistant, unblocked the ledger migration, and cleared the agent alert'
}

// ── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  if (flag('clean')) {
    await clean()
    return
  }

  const owner = await ownerUser()
  log(`owner: ${owner.name ?? owner.email} (${owner.id})`)

  const assistants = await sql`select model, display_name from agent_defs where owner_user_id = ${owner.id} and enabled limit 1`
  if (!assistants[0]) {
    // Stated plainly rather than worked around. A brief is written by the
    // owner's own assistant and by nothing else, so without one this script can
    // still seed the WORLD but cannot produce a document.
    log('WARNING: this user has no enabled personal assistant, so no brief can be written.')
    log('         The sources below will still be seeded and the surface will show the setup card.')
  }

  const mates = await seedTeammates()
  const { board, ids } = await seedBoard(owner, mates)
  await seedDms(owner, mates)
  const platformChannelId = await seedPlatformChannel(owner, mates)
  await seedNotifications(owner, { boardId: board.id, taskIds: ids, platformChannelId })

  if (!assistants[0]) {
    log('done — seeded the sources only.')
    return
  }

  // THE BRIEF RUNS THROUGH THE REAL SERVER MODULES, and that is the whole
  // value of doing it here rather than writing `daily_brief_entries` rows by
  // hand: this exercises `openBrief` and `sweepBrief` exactly as the scheduler
  // does, so a fixture that renders is evidence the scheduled path works
  // — where hand-written rows would only be evidence that the renderer does.
  //
  // Loaded through Vite's SSR loader because those modules are TypeScript with
  // `@/` path aliases and extensionless relative imports; plain Node resolves
  // none of that, and the alternative was a second copy of the logic in JS.
  // This is the same `ssrLoadModule` call the dev server's own /api middleware
  // makes (see ui/vite.config.ts), so the resolution is the project's, not this
  // script's idea of it.
  const { createServer } = await importFromUi('vite')
  const vite = await createServer({
    root: join(ROOT, 'ui'),
    // No middleware mode server, no HMR, no watcher: this process wants a
    // module resolver and nothing else, and a file watcher left running is why
    // a one-shot script hangs at the end instead of exiting.
    server: { middlewareMode: true, hmr: false, watch: null },
    appType: 'custom',
    logLevel: 'warn',
  })

  let brief
  try {
    const mod = await vite.ssrLoadModule('/src/server/daily-brief.ts')
    const { openBrief, sweepBrief, getBrief } = mod
    const session = { id: owner.id, sub: '', email: owner.email, name: owner.name, picture: null, provider: 'local', role: owner.role }

    const opened = await openBrief(session)
    log(opened.opened ? 'opened today’s brief' : 'today’s brief already existed — appending to it')

    for (const step of [1, 2]) {
      const what = await advanceWorld(step, owner, mates, ids)
      const swept = await sweepBrief(session)
      log(`pass ${step}: ${what} → +${swept.appended} entries (${swept.added} new, ${swept.changed} changed, ${swept.resolved} resolved)`)
    }

    brief = await getBrief(session)
  } finally {
    await vite.close()
  }

  if ('absent' in brief) {
    log(`FAILED: the brief reads as absent (${brief.absent}) after being opened — that is a bug, not a fixture problem.`)
    process.exitCode = 1
    return
  }

  const comms = brief.sections.find((s) => s.section === 'comms')
  if (comms) {
    console.log('')
    log('conversations:')
    for (const line of comms.lines) {
      const draft = brief.comms.find((c) => c.sourceKey === line.key)
      const mark = line.resolved ? '\u2713' : '\u25cf'
      const extra = draft?.delegated ? ' [assistant handles this]' : draft?.draft ? ' [draft parked]' : ''
      log(`   ${mark} ${String(line.current.statusLabel ?? '').padEnd(22)} ${line.current.title}${extra}`)
    }
  }

  const lines = brief.sections.reduce((n, s) => n + s.lines.length, 0)
  const moved = brief.sections.flatMap((s) => s.lines).filter((l) => l.history.length > 1).length
  console.log('')
  log(`brief ${brief.date} — ${lines} lines across ${brief.sections.length} sections, ${brief.updates.length} updates, ${brief.lastSeq} entries`)
  log(`${moved} line(s) have a history, ${brief.sections.flatMap((s) => s.lines).filter((l) => l.resolved).length} resolved and still on the page`)
  log(`lede: ${brief.lede.slice(0, 120)}${brief.lede.length > 120 ? '…' : ''}`)
  console.log('')
  log('open http://localhost:5273/ to see it.')
}

main()
  .catch((e) => {
    console.error('[seed] failed:', e)
    process.exitCode = 1
  })
  .finally(async () => {
    await sql.end({ timeout: 5 }).catch(() => {})
    // EXPLICIT EXIT, and it is not laziness. Loading the server modules starts
    // the singletons they own — the application's own postgres pool and the
    // Redis connection `publishUser` publishes over — and neither exposes a
    // teardown this script could call. Both hold the event loop open, so a
    // one-shot script that merely finishes its work never returns to the shell.
    //
    // The work is already durable at this point: every insert has committed and
    // the summary above is read back from the database rather than from
    // anything in memory. The one thing still in flight is the artifact mirror,
    // which `appendEntries` fires detached BY DESIGN — so a brief that exits
    // before its mirror finishes has a stale share link until the next append,
    // which is exactly the tradeoff that path was written to make.
    await new Promise((r) => setTimeout(r, 1_500))
    process.exit(process.exitCode ?? 0)
  })
