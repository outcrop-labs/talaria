// THE WAYS IN, as data. The empty stage used to say "Nothing open" and stop
// there, which told somebody standing in front of their whole fleet what was
// absent and nothing about what to do next. These four cards are the answer:
// each one is a session this surface already knows how to start, written as
// the first thing you would have said to the agent anyway.
//
// A CATALOGUE, not markup, for the reason every other arithmetic module here
// is separate: the copy and the kinds are the part that can be wrong in a way
// a person notices — a card that opens the wrong surface, a seed that forgets
// to say what it needs — and a test can reach a list.

/** What starting a card DOES. `work` and `plan` open a session with the agent
 *  (the kind the server resolves for itself on the first turn); `research`
 *  opens the research ask instead, because a report is scoped by a question
 *  the composer asks for rather than by a message. */
export type StarterKind = 'work' | 'plan' | 'research'

export interface StarterCard {
  /** Stable key — the `{#each}` key and what a test names. */
  key: string
  title: string
  hint: string
  kind: StarterKind
  /** The first message the session is started with.
   *
   *  EMPTY ON THE RESEARCH CARD ALONE, and that is the whole difference
   *  between it and the other three: the research composer owns the question
   *  (mode, depth, the ask itself), so a seed here would be a message sent
   *  into a surface that never asked for one. `sendsSeed` is the predicate. */
  seed: string
}

/** Does this card send its seed as a message, or open a composer? */
export const sendsSeed = (card: StarterCard): boolean => card.kind !== 'research'

export const STARTER_CARDS: readonly StarterCard[] = [
  {
    key: 'sprint',
    title: 'Plan a sprint',
    hint: 'Paste what you have and the plan builds in the document beside you.',
    kind: 'plan',
    seed: 'Help me plan the next sprint. I am going to paste what we have (notes, a backlog, last sprint’s leftovers) and I want a short plan out of it: the goal, the work in the order it should happen, and what we are deliberately leaving out.',
  },
  {
    key: 'research',
    title: 'Research a question',
    hint: 'Ask for a cited report. Recon answers fast, Expedition goes deep.',
    kind: 'research',
    seed: '',
  },
  {
    key: 'tickets',
    title: 'Draft tickets from a discussion',
    hint: 'Talk it through, then turn the thread into tickets for review.',
    kind: 'work',
    seed: 'I want to turn a discussion into tickets. I am going to paste the thread below. Read it, ask me about anything it leaves ambiguous, and then we can draft the tickets from this session.',
  },
  {
    key: 'clock',
    title: 'Put the agent on a clock',
    hint: 'Agree the job here, then set the clock in the header.',
    kind: 'work',
    seed: 'I want you running this on a schedule. Here is the job I have in mind. Tell me what you would check on each run and what you would report back, and once we agree I will set the clock in the header of this session.',
  },
]
