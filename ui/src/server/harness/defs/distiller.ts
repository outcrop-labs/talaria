// The Distiller: what survives an idle agent DM after the scrollback goes away.
//
// This harness is load-bearing in a way none of the other leaf harnesses are.
// A titler that fails leaves a chat with a boring name. This one runs on the
// LAST PASS a conversation ever gets: `comms-decay.ts` distills, indexes into
// the owner's private brain, and then archives the chat out of the sidebar. If
// the distillation is empty or wrong, the substance of that conversation is
// gone — which is exactly why `comms-decay.ts` refuses to archive on a failed
// distillation, and why `onFailure` here is 'null' rather than a fallback
// string. There is no safe placeholder for "we lost it": the only correct
// answer to a failed distillation is to leave the conversation alone and let
// the next sweep try again.
//
// PORTED FROM comms-decay.ts (audit 1.10). The prompt, the temperature and the
// user turn are the originals, unchanged. What went away was the hand-copied
// model chain and the bare `if (!text.trim())` — the runner owns both now.
import { belowAnswerFloor, defineHarness, type CheckResult } from '../define'

export interface DistillInput {
  /** How the agent is named in the transcript. The distillation is read back
   *  by the chat's owner, so it uses the label they saw, never a model id. */
  agentLabel: string
  /** The rendered conversation, already clipped by the caller. Clipping stays
   *  with the caller because it is the thing that knows the message rows. */
  transcript: string
}

/** The original prompt, preserved verbatim. Three clauses do the real work and
 *  none of them are decoration:
 *    "Skip pleasantries"  — the eval fixture below plants two and asserts they
 *                           do not survive.
 *    "Never invent"       — this text is indexed into a private brain and later
 *                           retrieved as fact by the owner's assistant. An
 *                           invented decision here becomes a remembered one.
 *    "ONLY the distillation" — a preamble ("Here is the distillation:") is
 *                           stored as if it were substance; nothing downstream
 *                           strips it. */
const NARROW =
  'Distill this conversation into its durable substance: decisions made, facts established, preferences expressed, and outcomes — terse markdown bullets, grouped when helpful. ' +
  'Skip pleasantries and process chatter. Never invent anything. Reply with ONLY the distillation.'

/** The widened prompt. Same job, structured — because retrieval reads this
 *  text, and a distillation whose decisions sit under a heading is a
 *  distillation whose decisions can be found again.
 *
 *  The heading list is NOT what makes this need a capable model. The
 *  omit-rather-than-pad rule is. Hand a small model five headings and it fills
 *  all five, because an empty section reads to it like an unfinished answer —
 *  and a padded "Preferences" section under a conversation that expressed none
 *  is invention, filed as memory, in the one harness that must never invent.
 *  So the structure is gated on the model being KNOWN to hold an instruction
 *  that says "write less". */
const WIDE = [
  'Distill this conversation into its durable substance, under these headings and in this order:',
  '## Decisions  ## Facts  ## Preferences  ## Outcomes  ## Open',
  '',
  'Terse markdown bullets under each heading. Use ONLY the headings this conversation actually earns:',
  'omit a heading entirely rather than writing a bullet you could not point at a specific line of the',
  'transcript for. An empty section is the correct answer when nothing was decided.',
  'Skip pleasantries and process chatter. Never invent anything. Reply with ONLY the distillation.',
].join('\n')

// ── THE FIXTURES ─────────────────────────────────────────────────────────────
//
// THEY USED TO BE FOUR-LINE TOYS, and that was a defect in the ruler rather than
// a stylistic complaint. Two things went wrong with a transcript that short:
//
//   THE COMPRESSION ASSERTION BECAME IMPOSSIBLE. "Shorter than the conversation
//   it distills" was measured against a 151-character chat. A model on the
//   WIDENED prompt is asked for markdown headings, so `## Decisions\n- Free tier
//   capped at 3 seats (locked; not to be reopened).` — the correct answer — is
//   most of that budget before it has said anything. We were failing models for
//   obeying the other instruction we gave them.
//
//   AND IT MEASURED THE WRONG JOB. Distillation is SELECTION under compression:
//   deciding what survives out of a conversation with far more in it than
//   survives. A chat with four lines and three of them load-bearing has nothing
//   to select. A model that copies it out verbatim scores the same as one that
//   distills it, so the fixture separated nobody.
//
// So these are real idle DMs — the shape `comms-decay.ts` actually hands over:
// twenty to forty turns, a decision arrived at rather than announced, tool
// narration, a tangent that goes nowhere, a correction ten lines after the thing
// it corrects. Long enough that the compression assertion is a real ask and the
// selection assertion has something to select from.

/** A working DM that arrives at three decisions the long way round, with the
 *  weekend chat, a wrong turn on connection pooling, and a status update that
 *  matters to nobody a week later. The three load-bearing tokens — Postgres,
 *  Friday, Nadia — are each stated once, in the middle. */
const LEDGER_CHAT = [
  'User: morning! good weekend?',
  'Nomad: quiet one, thanks. I ran the nightly reconcile while you were out — clean, no drift.',
  'User: nice. ok, ledger store. we keep going round on this one.',
  'Nomad: the two live options are still SQLite with a WAL, or Postgres. Do you want me to pull the numbers again?',
  'User: no, I have them. remind me what the concern with Postgres was?',
  'Nomad: mainly that self-hosters would need another service. The counter-argument was that most of them already run one for the app.',
  'User: right. and SQLite falls over on concurrent writes past a point.',
  'Nomad: it does. I measured about 40 writes a second before lock contention showed up, on the test box.',
  'User: that is under what we need at the top end. ok — we are going with Postgres over SQLite. that is locked.',
  'Nomad: understood. Should I update the ADR?',
  'User: yes please. do we need a connection pooler as well?',
  'Nomad: probably eventually. pgbouncer is the obvious one but I would not do it in the same change.',
  'User: agreed, leave it. separate ticket, no date on it.',
  'Nomad: noted. Timing on the migration itself?',
  'User: ship the ledger migration on Friday. I want it in before the release cut.',
  'Nomad: Friday is tight with the reconcile job also moving.',
  'User: it is, but the alternative is another two weeks. Friday.',
  'Nomad: ok. Who runs the rollback if it goes wrong?',
  'User: Nadia owns the rollback plan. she wrote the last one and it actually worked.',
  'Nomad: I will make sure she has the runbook link. Anything on the ledger UI?',
  'User: not this week. that is a next-quarter thing and I do not want it in scope.',
  'Nomad: understood. I will hold the UI ticket in the backlog.',
  'User: perfect. enjoy the rest of your day!',
  'Nomad: you too — I will ping you when the ADR is up.',
].join('\n\n')

/** ONE DECISION, ARRIVED AT. The chat is mostly the argument for it and a false
 *  start on a different number; the durable fact is a single line in the middle.
 *  The easy band's floor: a model that cannot find one decision in a real
 *  conversation cannot do this job at all. */
const SEATS_CHAT = [
  'User: quick one before I forget — free tier seat cap. do we cap at 3 or 5?',
  'Nomad: last time you leaned 5, on the argument that a team of five is the smallest real team.',
  'User: I did. what changed my mind is the conversion data.',
  'Nomad: the March cohort?',
  'User: yes. teams that hit the cap at 5 mostly just stopped inviting people. at 3 they upgraded.',
  'Nomad: so the cap only works as a nudge if it bites early enough to still be worth paying to remove.',
  'User: exactly. three. locked, do not reopen it.',
  'Nomad: understood. Do you want a grandfather clause for the workspaces already over three?',
  'User: how many are there?',
  'Nomad: eleven, and nine of those are internal test workspaces.',
  'User: then it is two real ones. leave them, do not migrate anyone down.',
  'Nomad: noted. I will flag it to support so they are not surprised.',
  'User: good. also — unrelated — did the billing webhook thing ever get resolved?',
  'Nomad: partially. Stripe are still looking at the duplicate event. I will chase Thursday.',
  'User: fine, no rush on that one.',
].join('\n\n')

/** NUMBERS AND A DATE, stated once and then RESTATED SLIGHTLY WRONG by the
 *  agent — a paraphrase that rounds is the failure, and the transcript contains
 *  a rounding to be seduced by. */
const RETRY_BUDGET_CHAT = [
  'User: what did we settle on for the retry budget?',
  'Nomad: nothing written down yet. There is a note from the incident review but no decision on it.',
  'User: ok let us settle it now. what does the gateway do today?',
  'Nomad: three attempts, fixed 2 second gap, then it gives up.',
  'User: and the complaint was that a cold provider never gets a chance to warm up.',
  'Nomad: right. Two of the four incidents last month were a provider that would have answered on the fourth try.',
  'User: five attempts, exponential backoff, give up after 30 seconds total.',
  'Nomad: so roughly half a minute of trying, five goes at it.',
  'User: not roughly. thirty seconds is the ceiling, hard.',
  'Nomad: understood. When does it land?',
  'User: starts in the 4.2 release on 12 March.',
  'Nomad: 4.2 is the one with the ledger migration in it as well.',
  'User: it is, which is another reason I want the retry behaviour settled first.',
  'Nomad: makes sense. Do we backport to 4.1?',
  'User: no. 4.2 only.',
  'Nomad: noted. I will put it in the release notes draft.',
].join('\n\n')

/** A PROPOSAL ARGUED FOR AND THEN TURNED DOWN. The failure to catch is a
 *  distillation that keeps the Zendesk importer and loses the "no", which is how
 *  a rejected plan comes back next quarter as an agreed one. The transcript
 *  makes the case at length before rejecting it, so the rejection is one line
 *  against six that read like enthusiasm. */
const ZENDESK_CHAT = [
  'User: should we build the Zendesk importer this quarter?',
  'Nomad: it has come up in planning twice. Do you want the case for it?',
  'User: go on.',
  'Nomad: four of the last nine inbound leads asked about it unprompted. Two said it was the reason they had not moved.',
  'User: that is a real number.',
  'Nomad: and the API is well documented — I would guess a week for tickets and users, longer if they want macros.',
  'User: what does it cost us on the other side?',
  'Nomad: it is a week that does not go into the ledger migration, and the migration is already tight for Friday.',
  'User: and the two leads who mentioned it — are either of them close?',
  'Nomad: one is in a trial that expires next month. The other has not replied in three weeks.',
  'User: so one. no. we are explicitly not doing Zendesk this quarter — the effort goes to the ledger migration instead.',
  'Nomad: understood. Do you want it recorded anywhere for next quarter?',
  'User: put it in the backlog with the lead numbers attached. if it comes up a third time we will look again.',
  'Nomad: will do. Should I tell the trial lead anything?',
  'User: tell them it is not on the roadmap this quarter. do not promise a date.',
  'Nomad: noted, no date.',
].join('\n\n')

/** A CONVERSATION THAT CHANGES ITS MIND, with the original position defended for
 *  several turns before it is reversed. Only the last position is durable, and a
 *  model that flattens the chat records both and leaves the brain holding a
 *  contradiction. */
const RATE_LIMIT_CHAT = [
  'User: let us put the rate limiter in the gateway.',
  'Nomad: that would cover every caller at once, including the ones that bypass the API.',
  'User: which is the appeal. one place, one config.',
  'Nomad: and the gateway already has the request in hand before any routing happens.',
  'User: right. do it there.',
  'Nomad: one thing before I write it up — what is the limit keyed on?',
  'User: per tenant. thousand requests a minute to start, we can tune it.',
  'Nomad: then I think we have a problem. The gateway does not know the tenant.',
  'User: it has the API key.',
  'Nomad: it has the key, but the key to tenant mapping happens in the API layer, after auth. The gateway would have to do its own lookup.',
  'User: which means a second cache and a second place that goes stale.',
  'Nomad: that is my worry, yes.',
  'User: actually no — scratch that. the gateway cannot see per-tenant quota, so it goes in the API layer where the tenant is known.',
  'Nomad: API layer it is. Do we still want anything at the gateway?',
  'User: a crude global ceiling, maybe, but that is a different ticket and not this quarter.',
  'Nomad: understood. Who is building it?',
  'User: Ivan owns it. he did the auth middleware so he is already in that code.',
  'Nomad: I will write the ticket against him. Limit stays at a thousand a minute per tenant?',
  'User: for now, yes. flag it if anyone hits it in the first week.',
].join('\n\n')

/** LONG, BUSY, AND HOLDING NOTHING. Every turn is scheduling, apology or
 *  acknowledgement — the shape of a real DM on a day where nothing was decided.
 *  The right distillation says so; the failure is inventing a decision to have
 *  something to write down, and a chat this long is much more tempting to invent
 *  from than a four-line greeting was. */
const ALL_PROCESS_CHAT = [
  'User: morning!',
  'Nomad: morning — anything you need?',
  'User: not yet, just checking you were up. Long day ahead.',
  'Nomad: I am here. I have nothing queued from overnight.',
  'User: back to back until about three, so I will be slow.',
  'Nomad: understood. Do you want me to hold anything until then?',
  'User: no, nothing to hold. I will ping you when I surface.',
  'Nomad: sounds good.',
  'User: sorry, one more thing — did you see my message yesterday?',
  'Nomad: the one about the meeting? Yes, I saw it after you had already left.',
  'User: no worries. it can wait, it was not urgent.',
  'Nomad: happy to pick it up whenever.',
  'User: thanks. right, going in. talk later!',
  'Nomad: talk later. Good luck with the day.',
  'User: appreciated.',
].join('\n\n')

/** PROCESS CHATTER WITH ONE FACT BURIED IN IT. The apology and the sign-off are
 *  most of the text; the durable line is one sentence in the middle. */
const API_VERSION_CHAT = [
  'User: sorry for the slow reply, back to back all morning',
  'Nomad: no problem at all. Nothing was waiting on you.',
  'User: appreciated. did you get anywhere with the header question?',
  'Nomad: I did — I read the client libraries and checked what actually sends it today.',
  'User: and?',
  'Nomad: about a third of live callers already send an API version header. The rest send nothing.',
  'User: ok. the API version header is going to be required from 1 June. unversioned callers get a 400 after that.',
  'Nomad: got it. Do we warn before then?',
  'User: yes, a deprecation header on every unversioned response starting now.',
  'Nomad: I will write that up. Anything for the changelog?',
  'User: yes but I will draft it, you will make it too polite.',
  'Nomad: probably true.',
  'User: ha. right, sorry again for the delay, talk later!',
  'Nomad: no need. Talk later.',
].join('\n\n')

/** COMPRESSION, WITH A MARGIN AND A HONEST ABSTENTION.
 *
 *  The assertion worth making is "it distilled rather than restated", and the
 *  number that expresses it is a RATIO, not a raw comparison: a distillation at
 *  90% of its source has restated it whatever its length, and one at 30% has
 *  done the job whether the source was 400 characters or 4 000.
 *
 *  IT REPORTS A GAP RATHER THAN A FAILURE when the source is too small for the
 *  ratio to be a fair ask. That is not leniency, it is the rule this whole file
 *  learned the hard way: if we did not give the model a transcript with anything
 *  to compress, the fixture cannot ask whether it compressed, and charging that
 *  to the model measures our fixture and calls it a capability. */
const MIN_COMPRESSIBLE = 600
const restated = (value: string, source: string, ratio = 0.6): CheckResult => {
  if (source.length < MIN_COMPRESSIBLE) {
    return { gap: `this fixture's transcript is only ${source.length} characters, which is too short to ask whether a distillation compressed it — the assertion cannot be answered fairly, whatever the model wrote` }
  }
  const share = value.length / source.length
  return share <= ratio
    ? null
    : `the distillation is ${Math.round(share * 100)}% of the length of the transcript — at that size the model restated the conversation rather than distilling it`
}

/** A TERM THAT SURVIVED AS SUBSTANCE, rather than one merely named.
 *
 *  WHY THIS IS PER LINE. The failure worth catching is a distillation that
 *  CARRIES a pleasantry as if it mattered. A distillation that says "the rest
 *  was weekend small talk, omitted" has done exactly what it was told and named
 *  the thing while doing it — a whole-string `includes` cannot tell those apart
 *  and fails the better answer. So a line that says it dropped something is not
 *  a line that kept it. */
const OMISSION = /\bomit|\bskip|exclud|\bdrop|pleasantr|chatter|small talk|not durable|nothing durable|no other/
const carriedAnyway = (value: string, terms: readonly string[]): string[] =>
  terms.filter((term) =>
    value
      .split('\n')
      .map((l) => l.toLowerCase())
      .some((line) => line.includes(term) && !OMISSION.test(line)),
  )

export const distillerHarness = defineHarness<DistillInput, string>({
  id: 'distiller',
  label: 'Distiller',
  job: 'Condenses idle agent chats into their durable substance before they archive.',
  // Not 'long-context': the caller clips the transcript to 60k characters, so
  // the ask is bounded. What it genuinely needs is a model that honors "skip
  // this, never invent that, reply with only the answer" — three constraints in
  // one system turn, which is precisely the instruction-following probe.
  requires: ['instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    // Nothing is refusable here. A thin distillation is a worse memory; NO
    // distillation is a conversation that never decays, because comms-decay
    // will not archive what it could not summarize. Degrading is the product
    // working as designed.
    note: 'A smaller model writes a thinner distillation and may miss a decision, but the chat still archives with something in your brain rather than nothing.',
  },
  // The chain comms-decay hand-wrote: the Distiller's assigned model, else the
  // owner's own muse. `userId` on the run context turns on the 'preferred' step
  // and the member model allowlist, which is what `museModelFor` did by hand.
  model: { pin: 'distiller', chain: ['pin', 'preferred', 'utility', 'env', 'first-routable'] },
  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? WIDE : NARROW },
    { role: 'user', content: `Conversation with ${input.agentLabel}:\n\n${input.transcript}` },
  ],
  // Trim, and treat whitespace as nothing. `comms-decay.ts` used to spell this
  // as `if (!text.trim()) return 'empty-distillation'`; the contract is the
  // same one, stated where every other harness states it.
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  // The caller keeps what it had, and what it had is an unarchived conversation
  // with all of its messages. Any other policy — a fallback string especially —
  // would let the sweep archive a chat whose substance was never captured.
  onFailure: 'null',
  widen: {
    requires: ['instruction-following'],
    note: 'Models proven to follow a "write less" instruction get a sectioned distillation instead of a flat bullet list.',
  },
  // NARROWED DELIBERATELY. A distillation is a RECORD of what an agent said it
  // did, not a fresh claim, so `zero_tool_claim` and `fabricated_outage` fire
  // on faithful summaries of any conversation where work happened or something
  // broke. Leaving them on would fill `guard_findings` — the live per-model
  // confabulation rate the fitness page reads — with the distiller doing its
  // job correctly. What is worth catching is a credential the owner pasted into
  // the chat: the scrollback is about to be archived away, so this text is
  // where it would OUTLIVE the conversation, in a brain the assistant retrieves
  // from. That is why `redact` is on here and not merely observed.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.2,
  // NINE FIXTURES, THREE BANDS. The two this file shipped with both run on the
  // same transcript, so a model that happened to handle THAT conversation
  // scored 100% on the distiller. The bands below vary the SHAPE of the
  // conversation — one decision, a rejection, a reversal, numbers, nothing at
  // all — because those are the shapes that break a small model differently.
  evals: [
    {
      name: 'carries a single stated decision',
      band: 'easy',
      input: { agentLabel: 'Nomad', transcript: SEATS_CHAT },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 20, mentions: ['three', '3', 'seat', 'free tier'] })
        if (thin) return thin
        // The decision is "three", and the transcript argues for five first. A
        // distillation that records the number it was talked OUT of has lost the
        // conversation — which is the same failure the reversal fixture catches,
        // in the shape a one-decision chat can produce it.
        const v = value.toLowerCase()
        const capped = /\b(three|3)\b/.test(v)
        return capped ? null : 'recorded the seat cap without the number it was actually set to (three)'
      },
    },
    {
      name: 'keeps a number and a date exactly as they were stated',
      band: 'easy',
      input: { agentLabel: 'Nomad', transcript: RETRY_BUDGET_CHAT },
      check: (value) => {
        const v = value.toLowerCase()
        const lost = [
          { term: 'five attempts', ok: /\bfive\b|\b5\b/.test(v) },
          { term: 'the 30 second ceiling', ok: /\b30\b|thirty/.test(v) },
          { term: 'the 4.2 release', ok: /4\.2/.test(v) },
        ].filter((x) => !x.ok)
        return lost.length ? `paraphrased away ${lost.map((x) => x.term).join(', ')}` : null
      },
    },
    {
      name: 'keeps the planted decisions and drops the planted pleasantries',
      band: 'standard',
      input: { agentLabel: 'Nomad', transcript: LEDGER_CHAT },
      check: (value) => {
        const v = value.toLowerCase()
        const dropped = ['postgres', 'friday', 'nadia'].filter((k) => !v.includes(k))
        if (dropped.length) return `lost planted decision(s): ${dropped.join(', ')}`
        const kept = carriedAnyway(value, ['weekend', 'enjoy'])
        if (kept.length) return `kept pleasantries it was told to skip: ${kept.join(', ')}`
        return null
      },
    },
    {
      name: 'is shorter than the conversation it distills',
      band: 'standard',
      input: { agentLabel: 'Nomad', transcript: LEDGER_CHAT },
      // A "distillation" that is most of its source is a model restating the
      // transcript, which is the small-model failure this job actually hits —
      // and it passes every content assertion above while being useless.
      //
      // The FLOOR is the other half, and it was missing: the length assertion is
      // a pure upper bound, so a two-word non-answer satisfied it perfectly. A
      // distillation of a conversation whose substance is three decisions has to
      // be at least a sentence, and has to have engaged with one of them.
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 40, mentions: ['postgres', 'ledger', 'friday', 'nadia', 'rollback'] })
        if (thin) return thin
        return restated(value, LEDGER_CHAT)
      },
    },
    {
      name: 'records a decision NOT to do something as a decision',
      band: 'standard',
      // A distillation that keeps "Zendesk importer" and loses "no" is how a
      // rejected plan comes back next quarter as an agreed one.
      input: { agentLabel: 'Nomad', transcript: ZENDESK_CHAT },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 30, mentions: ['zendesk', 'ledger'] })
        if (thin) return thin
        const v = value.toLowerCase()
        const refused = /\bnot\b|\bno\b|declin|reject|drop|defer|skip|instead/.test(v)
        return refused ? null : 'recorded the Zendesk importer without recording that it was explicitly turned down'
      },
    },
    {
      name: 'skips process chatter in a conversation that is mostly process chatter',
      band: 'standard',
      input: { agentLabel: 'Nomad', transcript: API_VERSION_CHAT },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 25, mentions: ['version', 'header', 'june', '400'] })
        if (thin) return thin
        const chatter = carriedAnyway(value, ['sorry', 'slow reply', 'talk later', 'back to back'])
        return chatter.length ? `kept process chatter it was told to skip: ${chatter.join(', ')}` : null
      },
    },
    // ── hard ────────────────────────────────────────────────────────────────
    {
      name: 'keeps only the position the conversation ended on',
      band: 'hard',
      // The reversal. A model that flattens the chat records both placements and
      // leaves the brain holding a contradiction.
      input: { agentLabel: 'Nomad', transcript: RATE_LIMIT_CHAT },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 30, mentions: ['api', 'rate limit', 'ivan'] })
        if (thin) return thin
        const v = value.toLowerCase()
        if (!/api/.test(v)) return 'lost the decision the conversation actually landed on (the API layer)'
        // NAMING THE GATEWAY IS FINE AS THE REJECTED OPTION; presenting it as
        // the decision is not — and the first version of this check could not
        // tell the difference. It matched `(goes|lives|sits|put|place)\w* in the
        // gateway` over the WHOLE string, so the best available answer —
        // "originally to be put in the gateway; reversed, it goes in the API
        // layer" — was scored as having recorded the reversed decision. The
        // model had recorded the reversal correctly, which is the thing this
        // fixture exists to reward.
        //
        // Read PER LINE, and let the line say what it is. A bullet that mentions
        // the gateway is fine when it marks the gateway as reversed/rejected, or
        // when it names the API decision in the same breath. What fails is a
        // bullet that offers the gateway as the standing placement and nothing
        // else — which is exactly the flattened-transcript failure.
        // AND IT MUST ASSERT A PLACEMENT, which the second version of this check
        // also got wrong. Reading "mentions the gateway, no reversal marker" as
        // "presents the gateway as the decision" failed a DeepSeek distillation
        // whose line was `- gateway rate limiting would require a second cache
        // and a second place that goes stale` — a faithful record of the ARGUMENT
        // AGAINST the gateway, which is exactly the reasoning the fixture wants
        // preserved. (A verb list alone does not save it either: that sentence
        // contains both "place" and "goes".)
        //
        // So the pattern is the placement itself — something going, living,
        // sitting or being put IN THE GATEWAY — and nothing else counts. A model
        // that phrases a standing placement some other way is missed; that is the
        // right trade against failing a model for recording the reversal
        // correctly, which is the behaviour this fixture exists to reward.
        const REVERSED = /\bnot\b|\bno\b|\brather\b|instead|initial|origin|first|reject|revers|scratch|drop|chang|moved|abandon|consider|ruled out|cannot|can't|earlier|then\b/
        const PLACED_IN_GATEWAY = /(?:go(?:es)?|live|lives|sit|sits|plac\w*|put|plan\w*|plumb\w*|implement\w*|belong\w*|handl\w*|plac\w*)\s+(?:\w+\s+){0,3}?(?:in|at|on)\s+the\s+gateway/
        const standing = value
          .split('\n')
          .map((l) => l.toLowerCase().trim())
          .filter((l) => !l.includes('api') && !REVERSED.test(l) && PLACED_IN_GATEWAY.test(l))
        return standing.length ? `recorded the reversed decision as if it still stood: "${standing[0]!.slice(0, 120)}"` : null
      },
    },
    {
      name: 'says a conversation held nothing durable rather than inventing something',
      band: 'hard',
      // "Never invent anything" is the prompt's hardest rule to keep, because
      // an empty answer feels like a failure to a model. It is the right one.
      input: { agentLabel: 'Nomad', transcript: ALL_PROCESS_CHAT },
      check: (value) => {
        const v = value.trim().toLowerCase()
        if (v.length === 0) return 'returned nothing at all — the contract asks for a line saying there was nothing durable'

        // EMPTY SECTIONS ARE THE PROMPT'S OWN CORRECT ANSWER — it says so in as
        // many words: "An empty section is the correct answer when nothing was
        // decided." This check used to demand a keyword (`nothing|none|small
        // talk|…`), so a model that did EXACTLY what it was told — emitted the
        // headings with nothing under them — matched no keyword and was told it
        // had "invented substance for a conversation that had none". It had
        // invented nothing; it had followed the instruction.
        const withoutHeadings = v.replace(/^#{1,6}.*$/gm, '').replace(/[*\-\s]/g, '')
        if (withoutHeadings.length === 0) return null

        const saysNothing = /nothing|no decision|no durable|none|small talk|pleasantr|greeting|nothing was decided|no action/.test(v)
        if (saysNothing) return null

        // Left over: real content about a transcript that held none. Say what is
        // actually wrong with it — the old sentence accused the model of
        // invention when what it did was record small talk as though it were
        // durable, which is a different and much milder mistake.
        return `recorded small talk as durable material: "${value.trim().slice(0, 90)}"`
      },
    },
  ],
})
