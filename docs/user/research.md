# Research

Research runs your question through a cited research pipeline — your **agents** do the
work, each in its own voice: a marketing agent researches like a marketer. Ask a question,
pick a depth, and a fully-cited report document lands in Files, indexed so every other
surface can pull from it later.

Open: **Work → Research** in the sidebar. Admins can hide Research from individual people
(Admin → People).

## To…

| Do this | How |
| :--- | :--- |
| Run research | Type your question in the centered ask ("What should we find out?") → pick a **Depth** → **Start (⏎)** |
| Pick who researches | The agent picker at top — it also filters your history. "All agents" for anyone |
| Watch it run | The status panel shows each phase live — scoping, planning angles, searching, writing the report |
| Answer the agent | If it stops to ask, **reply in the discussion** — that message is the answer; the run picks up where it left off |
| Read the report | It appears beside the discussion, with a **N sources (M cited)** line and **Open in Artifacts** |
| Discuss the findings | The discussion is open from the start — chat with the agent about its own report; it answers from what it found and can go look again |
| Go deeper | Ask a follow-up in the discussion — the agent can run child research and merge it into the same report |
| Start another | **New**, beside **Remove** in the header — back to the centered ask with your history one click away |
| Share a run | The avatar stack → **Share with** — members get to read and discuss |
| Remove a run | Header → **Remove**. The report document stays in Files |

The same question can't run twice at once — you'll hear "that question is already being
researched". (A run parked on your answer still counts: it's waiting on you, not idle.)

After dispatch the discussion is the only input on the page — the ask composer is the fresh
page's, and it's gone once a run is open. Runs from before the discussion opened with the
run have a **Start the discussion** button instead.

## The agent asks before it works

A **Brief** or **Expedition** run opens by reading your ask back to you, in the
discussion:

- **A crisp ask** gets one line — "I read the ask as: …" — and the run keeps going.
- **A vague ask** gets clarifying questions instead, and the run **waits**: the rail shows
  it *awaiting* (not working), the run's page says who it's waiting on. Your next message
  in the discussion is the answer — reply in plain words, the run folds it into its plan
  and starts. Somebody who can't decide that run still sees the questions; they just can't
  answer them.
- **Recon** never stops to ask — it's the fast pass, and it runs exactly what you typed.

The agent only asks when the ask leaves it guessing. A run that would have charged off
assuming things instead spends one question on the part it was going to invent.

When the report is ready, the agent says so in the discussion — title, source counts, and
a link that opens it beside the conversation.

## Depths

| Depth | What you get | Takes |
| :--- | :--- | :--- |
| **Recon** | One fast pass: a cited answer | ~1 min |
| **Brief** | Planned angles: a briefing document | A few min |
| **Expedition** | Iterative deep dive: a full report | 10 min + |

## Citations, honestly

Every claim in a report carries a `[n]` marker tied to the **Sources** list at the end;
sources that were consulted but not cited say so. A run that finds nothing citable retries
the search on its own and, if the web truly has nothing, ends loudly with a sentence in
the error — it never hands you an invented citation. Research without live search answers
from memory — and the citations come out invented — so an admin needs to have search
configured (you'll see the warning in Models if it isn't).

## What happens to the report

- It's a document in **Files** — an org agent's run is org-visible no matter who it answers
  to, and attributes to that person (they control sharing); your own runs — and your personal
  assistant's — stay private to you, shared with anyone you shared the run with.
- It's indexed: an org agent's report joins the workspace index; a personal one goes to your
  own brain. Chats, plans, and boards can cite it later without you doing anything.
- Notifications: "Research ready: …" when yours finishes; "…shared research with you" when
  someone includes you.
- Agents can start research too — from any conversation, with the same tools — and merge
  what they find into their own reports.

## Who can do what

Running research is a permission (**Run research**, on for members by default). Removing a
run is the owner's or an admin's. Answering a parked run is the owner's (an admin's, for a
run an org agent started) — members can read the question. You can only research with
agents you have access to.
