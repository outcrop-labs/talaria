package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.put
import io.ktor.client.request.setBody
import io.ktor.http.ContentType
import io.ktor.http.contentType
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * One ticket: reading it, saying something on it, moving it, and clearing the
 * review gate.
 *
 * Shapes from `api/crates/talaria-tasks-types/src/lib.rs` (`Task`) and
 * `talaria-tasks` (`TaskComment`, `TaskFull`), all `rename_all = "camelCase"`.
 * `GET /api/tasks/{id}` answers a `TaskFull` — the ticket plus comments,
 * activity, watchers, reviews, dependencies and usage — and only the first
 * three are modelled, because the rest are desk surfaces.
 *
 * APPROVAL IS A MARK, NOT A MOVE, and getting that wrong would make the app
 * lie. The api is explicit: approving sets `approvedAt` and leaves the column
 * alone, because a signed-off ticket usually still has a merge, a deploy or a
 * release in front of it — forcing it to `done` would claim the work had
 * shipped when it had not. A person moves it to done when it really is. The
 * review queues read the mark, so an approved ticket stops asking to be
 * reviewed. **Rejection** is the move: it sends the ticket back to the board's
 * first working column.
 *
 * This is also the repo's own rule showing up on a phone — never force a `done`
 * transition; agents create and triage, a human signs off.
 */
@Serializable
data class Task(
    val id: String,
    @SerialName("boardId") val boardId: String = "",
    @SerialName("ticketRef") val ticketRef: String? = null,
    val title: String = "",
    val description: String? = null,
    val status: String = "",
    val priority: String = "",
    val assignees: List<String> = emptyList(),
    val tags: List<String> = emptyList(),
    @SerialName("commentCount") val commentCount: Int = 0,
    val outcome: String? = null,
    val resolution: String? = null,
    @SerialName("errorMessage") val errorMessage: String? = null,
    @SerialName("updatedAt") val updatedAt: String? = null,
    /** When a person signed this off. Null means the gate is still open. */
    @SerialName("approvedAt") val approvedAt: String? = null,
) {
    /** `TASK-12 · Fix the login loop`, or the bare title — the api's own rule
     *  (`WorkItem::label`), kept identical so one ticket reads the same in the
     *  queue and on its own screen. */
    val label: String get() = ticketRef?.takeIf { it.isNotEmpty() }?.let { "$it · $title" } ?: title

    /** Signed off already. The gate must not offer to clear itself twice. */
    val isApproved: Boolean get() = approvedAt != null
}

@Serializable
data class TaskComment(
    val id: String,
    val author: String = "",
    val content: String = "",
    @SerialName("parentId") val parentId: String? = null,
    @SerialName("createdAt") val createdAt: String? = null,
)

@Serializable
private data class TaskFullEnvelope(
    val task: Task,
    val comments: List<TaskComment> = emptyList(),
    val watchers: List<String> = emptyList(),
)

@Serializable
private data class TaskEnvelope(val task: Task)

@Serializable
private data class CommentEnvelope(val comment: TaskComment)

@Serializable
private data class ReviewBody(
    val status: String,
    val notes: String? = null,
)

@Serializable
private data class CommentBody(
    val content: String,
    @SerialName("parentId") val parentId: String? = null,
)

@Serializable
private data class StatusPatch(val status: String)

/** A ticket as a phone needs it: the ticket, what has been said on it, and who
 *  is watching. */
data class TaskDetail(
    val task: Task,
    val comments: List<TaskComment>,
    val watchers: List<String>,
)

/** The gate's two answers. There is no third — a gate that can be left ajar is
 *  a queue that never empties. */
enum class ReviewVerdict(val wire: String) {
    Approve("approved"),
    Reject("rejected"),
}

fun taskPath(id: String) = "/api/tasks/$id"

fun taskReviewPath(id: String) = "/api/tasks/$id/review"

fun taskCommentsPath(id: String) = "/api/tasks/$id/comments"

suspend fun readTask(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
): CommsResult<TaskDetail> =
    guardedCall(
        instance,
        call = { client.get("${instance.origin}${taskPath(taskId)}") { credential.applyTo(this) } },
        decode = {
            val full = it.body<TaskFullEnvelope>()
            TaskDetail(full.task, full.comments, full.watchers)
        },
    )

/**
 * Clear or refuse the review gate.
 *
 * `notes` is optional to the api and offered here because a rejection without
 * a reason sends an agent back to work with nothing to go on.
 */
suspend fun reviewTask(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
    verdict: ReviewVerdict,
    notes: String? = null,
): CommsResult<Task> =
    guardedCall(
        instance,
        call = {
            client.post("${instance.origin}${taskReviewPath(taskId)}") {
                credential.applyTo(this)
                contentType(ContentType.Application.Json)
                setBody(ReviewBody(status = verdict.wire, notes = notes?.takeIf { it.isNotBlank() }))
            }
        },
        decode = { it.body<TaskEnvelope>().task },
    )

suspend fun commentOnTask(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
    content: String,
): CommsResult<TaskComment> =
    guardedCall(
        instance,
        call = {
            client.post("${instance.origin}${taskCommentsPath(taskId)}") {
                credential.applyTo(this)
                contentType(ContentType.Application.Json)
                setBody(CommentBody(content))
            }
        },
        decode = { it.body<CommentEnvelope>().comment },
    )

/**
 * Move a ticket.
 *
 * Deliberately NOT a way to reach `done`: the screen offers the board's working
 * columns and leaves terminal transitions to a desk, which is the same reason
 * approval is a mark. A phone that could quietly complete work would undo the
 * guarantee the review gate exists to make.
 */
suspend fun setTaskStatus(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
    status: String,
): CommsResult<Task> =
    guardedCall(
        instance,
        call = {
            client.put("${instance.origin}${taskPath(taskId)}") {
                credential.applyTo(this)
                contentType(ContentType.Application.Json)
                setBody(StatusPatch(status))
            }
        },
        decode = { it.body<TaskEnvelope>().task },
    )
