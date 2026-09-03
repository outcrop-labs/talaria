# Knowledge

Knowledge is your workspace's shared brain in writing: a tree of markdown documents in
**spaces** (folders), written and edited together in real time. Promote a document to
**official** and it grounds every agent's answers — that's the line between notes and
knowledge here.

Open: **Work → Knowledge** in the sidebar. Admins can hide Knowledge from individual people
(Admin → People).

## To…

| Do this | How |
| :--- | :--- |
| Create a space | **New space** (sidebar) → name it. A space is a folder whose top page is its editable overview |
| Write a doc | **Doc** (tree footer), or the **+** on any row for a nested one |
| Find a doc | **Search knowledge** — title, text, space |
| Edit | Toggle **Edit** (authored docs open in **Read**). Everything saves as you type |
| Reorganize | Drag docs in the tree — before, after, or inside |
| Attach a file | The doc's **Attachments** strip → **Attach a file** |
| Comment | Select text in Read mode → **Comment**; threads can be resolved and reopened |
| See old versions | Kebab → **Version history** — preview any revision, **Restore this version** if needed |
| Make it official | **Promote** — the doc is indexed into the org brain and grounds every agent's answer |
| Share | **Share & permissions** (see below) |

**Agent docs** (tree footer) start from an OKF scaffold — structured knowledge for agents.
Official documents also carry an **OKF** summary: the agent-facing digest the Librarian
maintains, refreshed automatically when the document changes.

## Editing with Muse

Under every document, the Muse bar drafts for you: describe the change ("tighten the intro,
add a checklist") or select a passage and say how it should change. **Draft** (or refine the
proposal), then **Accept** or **Discard**. Selecting text anywhere also offers **Muse** and
**Comment** directly.

## Sharing

One dialog for docs, spaces, and files (in Files):

- **People & agents** — named viewers or editors. Agents only edit when given the Editor
  role, never by default.
- **General access** — **Restricted** (only people you add) · **Organization** (everyone in
  the workspace) · **Anyone with the link** (public on the internet).
- Documents inherit their folder's audience until you **Customize for this doc**.

Public links: a doc shares its rendered page (`/kb/…`); a space shares only its overview
paragraph, never the doc tree. Sharing changes are the owner's.

## Official, brains, and agents

| Concept | Meaning |
| :--- | :--- |
| **Official** | Promoted into the org brain; grounds every agent. Demoting (double confirm) pulls it out immediately |
| **Brain routing** | Which knowledge brain retrieves this content: auto, none, or a named brain |
| **OKF** | The agent-facing summary maintained on official docs |

Agents can search, read, write, and edit knowledge with their own tools — agent-written docs
start as drafts, and only a person can mark one official or change sharing. A doc an agent
writes belongs to the human it was answering (its owner controls sharing); private drafts are
invisible to agents; only official (or explicitly shared) content grounds them.

## Who can do what

Editing docs is on for members by default (**Edit knowledge**); creating spaces and
promoting to official is its own permission (**Curate knowledge**, usually admins). Per-doc
roles (viewer/editor) apply on top. One trap: without edit rights, the Read/Edit toggle
still appears — you'll only learn you can't save when the save fails.
