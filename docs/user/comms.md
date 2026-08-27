# Comms

Comms is every conversation in one place: **channels** for ambient team talk, **relays** for
gatherings that conclude and archive, **DMs** for teammates, and **agent chats** — one fresh
thread per topic. Agents are members like people are: @mention one in a channel and it
replies right in the feed; open it from the sidebar for a private thread.

Open: **Work → Comms** in the sidebar. Admins can hide Comms from individual people
(Admin → People).

## The four kinds

| Kind | Glyph | What it's for | Lifecycle |
| :--- | :--- | :--- | :--- |
| **Channel** | `#` | Persistent, ambient talk | Stays |
| **Relay** | `⇄` | People + agents gathered around one purpose | **Conclude**: a summary is posted, indexed, and the relay archives |
| **Teammate DM** | | One-to-one with a person | Stays |
| **Agent chat** | `◍` | Working sessions with one agent — a **new thread per topic**, so context stays bounded | Idle threads distill into a summary document and archive |

## To…

| Do this | How |
| :--- | :--- |
| Create a channel or relay | The `+` on the section heading in the sidebar → type a name |
| Bring an agent into a channel | Add it with the **agents** pill in the header (or channel settings), then **@mention** it in a message |
| Talk to an agent privately | Click the agent in the sidebar's **Agents** section — a new thread opens. Right-click the agent for **New thread** |
| Address a message to an agent | Type `@` and its name. `@Dex:opus` also picks a model tier for that reply |
| Reply in a thread | Hover a message → **Reply in thread**. Threads never nest |
| React | Hover → **Add reaction** — or type the emoji's `:shortcode:` |
| Attach something | The `+` in the composer: knowledge doc, file, upload, photos, folder. Paste and drag-drop work too |
| Conclude a relay | Header → **Conclude** — the summary lands as a message and the relay archives |
| Draft tickets from a conversation | Header clipboard icon → pick planner, board, template → **Draft tickets** → review each proposal → **Create all** |
| Leave a channel | Channel settings → **Leave** |
| Rename a thread | Right-click the thread in the sidebar → **Rename** |

While an agent replies you watch it type; **Stop (Esc)** cancels mid-stream. Sending again
while it works queues your message — it never interrupts.

## Messages

Edit or delete your own messages (the channel owner can delete anyone's); hover a message
for the toolbar, right-click for **Copy text / Copy link**. Threads show a **N replies**
rollup on the root message. In agent chats there are no reactions or threads — the only
message action is copy.

## Handing an agent a credential

In an agent chat, the key icon lets you hand over a secret **once**: name it
("Stripe test key"), paste the value — it goes straight to the vault, never into the message
or the transcript — optionally scope it to one host, and **Hand it over**. The agent gets a
handle it can spend once within the hour.

## Notifications

| Event | Notifies |
| :--- | :--- |
| A DM | Outright — "`Jon` sent you a message" |
| An @mention in a channel or relay | "…mentioned you in #general" |
| Ordinary channel chatter | Never — having the channel open means you've read it |
| An agent reaching out to you first | Inbox notification: "`Dex` reached out" |

Tune the classes (in-app / email / both) in **Settings → Notifications**.

## Who can do what

Anyone in a channel can post, add people and agents, and conclude a relay. Only the channel
**owner** can rename it, remove other members, delete the channel, or delete others'
messages. Creating channels and starting relays are permissions your admin can switch off —
without them the `+` buttons don't appear.

## Words Comms uses

| Term | Meaning |
| :--- | :--- |
| **Relay** | A gathering that concludes: summary posted and indexed, then archived |
| **Conclude** | That summarize-and-archive action |
| **Distill** | The automatic version for idle agent chats — the thread archives and a "Distilled: …" document lands in Files |
| **Thread** | Replies hanging off one root message; an agent @mentioned there replies in the thread, with the thread as its context |
| **Tier mention** | `@Name:tier` — routes that one reply to a model tier |
| **Confab guard** | A flag on a reply the model produced without something to ground it: "Unverified · confab guard flagged this reply" |
