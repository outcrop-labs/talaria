package app.talaria.mobile

import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.InMemoryAccountStore
import app.talaria.mobile.account.Instance
import app.talaria.mobile.account.Me
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.account.SignInResult
import app.talaria.mobile.instance.Beacon
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.net.HomeQueues
import app.talaria.mobile.net.HomeResult
import app.talaria.mobile.net.HomeSummary
import app.talaria.mobile.net.OrgGlance
import app.talaria.mobile.net.Ports
import app.talaria.mobile.net.Channel
import app.talaria.mobile.net.ChannelPeer
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.Message
import app.talaria.mobile.net.QueueBucket
import app.talaria.mobile.net.ReviewVerdict
import app.talaria.mobile.net.Task
import app.talaria.mobile.net.TaskComment
import app.talaria.mobile.net.TaskDetail
import app.talaria.mobile.net.UserEvent
import app.talaria.mobile.net.WorkItem
import kotlinx.coroutines.flow.MutableSharedFlow

/**
 * The headless UI driver.
 *
 * `runComposeUiTest` composes the REAL tree — the same composables the phone
 * runs — against Skia's software renderer. No display, no emulator, no device,
 * no screenshots to eyeball: it finds nodes, clicks them, types into them and
 * asserts, inside the ordinary `:shared:jvmTest` gate that runs in a devbox in
 * seconds. That is the whole reason the UI lives in `commonMain` rather than
 * behind a platform boundary.
 *
 * THE PORTS ARE FAKED, AND THAT IS THE POINT. An earlier version of these tests
 * drove a real Ktor client through a MockEngine, and it was flaky — `waitUntil`
 * races a coroutine on another dispatcher, and a virtual test clock can burn
 * its whole timeout before that coroutine is ever scheduled. So the UI tests
 * answer UI questions against functions that return immediately, and the wire
 * is answered where it lives: `BeaconTest` at the response level and
 * `LoopbackApiTest` over a real socket. No test waits on another's scheduler.
 *
 * What these tests are FOR is behaviour — does a bad URL show a reason and
 * leave the list alone, does an expired session drop the account, does each
 * account's own credential ride its own request. Appearance is the spec's job
 * (docs/design/mercury-spec.md), not a test's.
 */
const val TEST_UUID = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"

fun testInstance(
    origin: String = "https://talaria.example.com",
    label: String = "Outcrop Labs",
) = Instance(id = TEST_UUID, origin = origin, label = label)

/**
 * Overridable ports with sane defaults, recording what they were asked.
 *
 * The defaults describe a healthy instance with one person's work waiting on
 * it, so a test states only the thing it is about.
 */
class FakePorts {
    var onProbe: suspend (String) -> ProbeResult = { origin ->
        ProbeResult.Found(origin, Beacon(instance = TEST_UUID, companyName = "Outcrop Labs"))
    }

    var onSignIn: suspend (Instance, String, String) -> SignInResult = { instance, username, password ->
        if (password == CORRECT_PASSWORD) {
            SignInResult.Ok(Account(instance.id, username), Credential.Session("sid-$username"))
        } else {
            SignInResult.Refused("that username and password did not match")
        }
    }

    var onMe: suspend (Instance, Credential) -> MeResult = { _, _ -> MeResult.Ok(Me(name = "Jon Iler")) }

    var onHome: suspend (Instance, Credential) -> HomeResult = { _, _ -> HomeResult.Ok(sampleHome()) }

    /** Origins the launcher asked about, in order. */
    val probed = mutableListOf<String>()

    /** Credentials `/api/home` was fetched with — the assertion surface for
     *  "did the right account's session ride the request?" */
    val homeCredentials = mutableListOf<Credential>()

    var onChannels: suspend (Instance, Credential) -> CommsResult<List<Channel>> = { _, _ ->
        CommsResult.Ok(sampleChannels())
    }

    var onMessages: suspend (Instance, Credential, String) -> CommsResult<List<Message>> = { _, _, _ ->
        CommsResult.Ok(sampleMessages())
    }

    var onPost: suspend (Instance, Credential, String, String) -> CommsResult<Message> = { _, _, _, content ->
        CommsResult.Ok(
            Message(id = "posted-${'$'}{posted.size}", seq = 99, authorType = "user", author = "Jon", content = content),
        )
    }

    var onMarkRead: suspend (Instance, Credential, String) -> CommsResult<Unit> = { _, _, _ -> CommsResult.Ok(Unit) }

    /** Channels opened, messages sent, and rooms marked read, in order. */
    val messagesRead = mutableListOf<String>()
    val posted = mutableListOf<Pair<String, String>>()
    val markedRead = mutableListOf<String>()

    /** The ticket surface. The default ticket sits AT the review gate with no
     *  approvedAt, because that is the state the whole app exists to resolve. */
    var ticket: Task = sampleTicket()
    var ticketComments: List<TaskComment> = listOf(
        TaskComment(id = "tc1", author = "Muse", content = "Ran the suite; one claim needed a citation."),
    )

    var onTask: suspend (Instance, Credential, String) -> CommsResult<TaskDetail> = { _, _, _ ->
        CommsResult.Ok(TaskDetail(ticket, ticketComments, watchers = emptyList()))
    }

    var onReview: suspend (Instance, Credential, String, ReviewVerdict, String?) -> CommsResult<Task> =
        { _, _, _, verdict, _ ->
            // The api's semantics, faithfully: approving MARKS and does not
            // move; rejecting moves and does not mark. A fake that moved on
            // approve would let a wrong screen pass.
            ticket =
                when (verdict) {
                    ReviewVerdict.Approve -> ticket.copy(approvedAt = "2026-10-14T10:00:00Z")
                    ReviewVerdict.Reject -> ticket.copy(status = "in_progress")
                }
            CommsResult.Ok(ticket)
        }

    var onComment: suspend (Instance, Credential, String, String) -> CommsResult<TaskComment> =
        { _, _, _, content ->
            val added = TaskComment(id = "tc-${'$'}{ticketComments.size + 1}", author = "Jon", content = content)
            ticketComments = ticketComments + added
            CommsResult.Ok(added)
        }

    /** What the gate was asked, in order: verdict and the notes that rode with it. */
    val reviewed = mutableListOf<Pair<ReviewVerdict, String?>>()
    val ticketsRead = mutableListOf<String>()
    val commented = mutableListOf<Pair<String, String>>()

    /** The firehose a test drives by hand. `emit` on this is how a test says
     *  "the server just announced something changed". */
    val eventBus = MutableSharedFlow<UserEvent>(extraBufferCapacity = 16)

    fun ports(): Ports =
        Ports(
            probe = { origin ->
                probed += origin
                onProbe(origin)
            },
            signIn = { instance, username, password -> onSignIn(instance, username, password) },
            me = { instance, credential -> onMe(instance, credential) },
            home = { instance, credential ->
                homeCredentials += credential
                onHome(instance, credential)
            },
            channels = { instance, credential -> onChannels(instance, credential) },
            messages = { instance, credential, channelId ->
                messagesRead += channelId
                onMessages(instance, credential, channelId)
            },
            post = { instance, credential, channelId, content ->
                posted += channelId to content
                onPost(instance, credential, channelId, content)
            },
            markRead = { instance, credential, channelId ->
                markedRead += channelId
                onMarkRead(instance, credential, channelId)
            },
            events = { _, _ -> eventBus },
            task = { instance, credential, taskId ->
                ticketsRead += taskId
                onTask(instance, credential, taskId)
            },
            review = { instance, credential, taskId, verdict, notes ->
                reviewed += verdict to notes
                onReview(instance, credential, taskId, verdict, notes)
            },
            comment = { instance, credential, taskId, content ->
                commented += taskId to content
                onComment(instance, credential, taskId, content)
            },
        )

    companion object {
        const val CORRECT_PASSWORD = "correct-horse"
    }
}

/** A store holding one instance, optionally with one signed-in account. */
fun testStore(
    signedIn: Boolean,
    username: String = "jon@outcroplabs.com",
    displayName: String? = "Jon Iler",
): InMemoryAccountStore =
    if (signedIn) {
        InMemoryAccountStore(
            instances = listOf(testInstance()),
            signedIn = mapOf(Account(TEST_UUID, username, displayName) to Credential.Session("sid-abc")),
        )
    } else {
        InMemoryAccountStore(instances = listOf(testInstance()))
    }

private fun item(
    id: String,
    board: String,
    ref: String?,
    title: String,
    status: String,
    queue: String,
) = WorkItem(
    id = id,
    boardId = "b-$board",
    board = board,
    ticketRef = ref,
    title = title,
    status = status,
    updatedAt = "2026-10-08T09:00:00Z",
    queue = queue,
)

/**
 * A home summary shaped like a real morning: two tickets at the review gate,
 * one agent blocked, nothing in triage.
 *
 * `count` is set independently of `items.size` on purpose — the api is explicit
 * that the count is the whole queue and the items are a capped window, and the
 * screen has to say so when they differ.
 */
fun sampleHome(
    review: Int = 2,
    blocked: Int = 1,
    triage: Int = 0,
    unread: Int = 4,
    boards: Int = 3,
): HomeSummary =
    HomeSummary(
        org = OrgGlance(name = "Outcrop Labs"),
        queues =
            HomeQueues(
                review =
                    QueueBucket(
                        count = review,
                        items =
                            listOf(
                                item("t1", "Platform", "TASK-12", "Fix the login loop", "quality_review", "review"),
                                item("t2", "Platform", null, "Rename the gateway flag", "quality_review", "review"),
                            ).take(minOf(review, 2)),
                    ),
                blocked =
                    QueueBucket(
                        count = blocked,
                        items =
                            listOf(
                                item("t3", "Growth", "GRO-4", "Needs a decision on pricing", "blocked", "blocked"),
                            ).take(minOf(blocked, 1)),
                    ),
                triage = QueueBucket(count = triage, items = emptyList()),
            ),
        unread = unread,
        boards = boards,
    )

/** A room list shaped like a real one: a channel with something new in it, a
 *  quiet channel, and a DM with an agent. */
fun sampleChannels(): List<Channel> =
    listOf(
        Channel(id = "c-quiet", name = "general", kind = "channel", unreadCount = 0, topic = "everything else"),
        Channel(id = "c-busy", name = "platform", kind = "channel", unreadCount = 3, topic = "the api and the app"),
        Channel(
            id = "c-dm",
            name = "dm-7f3a",
            kind = "dm",
            unreadCount = 1,
            peer = ChannelPeer(userId = "u-muse", name = "Muse"),
            agents = listOf("Muse"),
        ),
    )

fun sampleMessages(): List<Message> =
    listOf(
        Message(id = "m1", seq = 1, authorType = "user", author = "Jon", content = "Did the gate clear?"),
        Message(
            id = "m2",
            seq = 2,
            authorType = "agent",
            author = "Muse",
            content = "Not yet — the judge flagged one claim.",
        ),
        // A thread reply, which belongs under its root and not in the flow.
        Message(
            id = "m3",
            seq = 3,
            authorType = "user",
            author = "Sam",
            content = "which claim?",
            threadRootId = "m2",
        ),
    )

/** A ticket parked at the review gate — the state the app exists to resolve. */
fun sampleTicket(
    id: String = "t1",
    approvedAt: String? = null,
    status: String = "quality_review",
): Task =
    Task(
        id = id,
        boardId = "b1",
        ticketRef = "TASK-12",
        title = "Fix the login loop",
        description = "Members on a stale session bounce between /login and /home.",
        status = status,
        priority = "high",
        assignees = listOf("Muse"),
        commentCount = 1,
        outcome = "Session refresh added; suite green.",
        updatedAt = "2026-10-14T09:00:00Z",
        approvedAt = approvedAt,
    )
