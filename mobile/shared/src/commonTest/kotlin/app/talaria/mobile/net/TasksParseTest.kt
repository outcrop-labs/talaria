package app.talaria.mobile.net

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The ticket wire contract: `Task` from
 * `api/crates/talaria-tasks-types/src/lib.rs` and `TaskComment`/`TaskFull` from
 * `talaria-tasks`, all `rename_all = "camelCase"`.
 *
 * `GET /api/tasks/{id}` answers a whole `TaskFull` — task, comments, activity,
 * watchers, reviews, judgeReviews, blockedBy, blocks, usage — and this client
 * models three of those nine. The rest must decode-and-drop rather than throw,
 * which is what most of these cases check.
 */
@Serializable
private data class FullWrapper(
    val task: Task,
    val comments: List<TaskComment> = emptyList(),
    val watchers: List<String> = emptyList(),
)

@Serializable
private data class TaskWrapper(val task: Task)

@Serializable
private data class CommentWrapper(val comment: TaskComment)

class TasksParseTest {
    private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

    /** A real `TaskFull`, including the six keys this client does not model. */
    private val fullPayload = """
        {
          "task": {
            "id":"t1","boardId":"b1","ticketRef":"TASK-12","title":"Fix the login loop",
            "description":"Members on a stale session bounce.","status":"quality_review",
            "priority":"high","effort":"M","assignees":["Muse"],"createdBy":"u1",
            "dueDate":null,"startDate":null,"color":null,"tags":["auth"],
            "attachments":[],"timeSpentSeconds":5400,"estimatedHours":2.5,"parentId":null,
            "commentCount":1,"outcome":"Session refresh added; suite green.","resolution":null,
            "errorMessage":null,"createdAt":"2026-10-13T08:00:00Z","updatedAt":"2026-10-14T09:00:00Z",
            "completedAt":null,"approvedAt":null,"archivedAt":null
          },
          "comments":[
            {"id":"tc1","author":"Muse","content":"Ran the suite.","parentId":null,
             "createdAt":"2026-10-14T09:05:00Z"}
          ],
          "activity":[{"id":"a1","kind":"status.changed"}],
          "watchers":["u1","u2"],
          "reviews":[{"id":"r1","status":"rejected"}],
          "judgeReviews":[{"id":"j1","verdict":"pass"}],
          "blockedBy":[],"blocks":[],
          "usage":{"tokens":12345,"usd":0.42}
        }
        """

    @Test
    fun a_whole_task_full_decodes_and_drops_what_is_not_modelled() {
        val full = json.decodeFromString<FullWrapper>(fullPayload)
        assertEquals("t1", full.task.id)
        assertEquals(1, full.comments.size)
        assertEquals(listOf("u1", "u2"), full.watchers)
        // activity, reviews, judgeReviews, blockedBy, blocks and usage were all
        // present and are all absent from the model. Decoding must survive it.
    }

    @Test
    fun the_camelcase_fields_actually_land() {
        val task = json.decodeFromString<FullWrapper>(fullPayload).task
        assertEquals("b1", task.boardId)
        assertEquals("TASK-12", task.ticketRef)
        assertEquals(1, task.commentCount)
        assertEquals("2026-10-14T09:00:00Z", task.updatedAt)
        assertEquals(listOf("Muse"), task.assignees)
        assertEquals(listOf("auth"), task.tags)
    }

    /**
     * `approvedAt` is the whole review gate in one nullable field. Null means
     * the gate is open; a timestamp means a human signed it off. Reading it
     * wrong either hides the buttons on unreviewed work or offers them twice on
     * reviewed work.
     */
    @Test
    fun approved_at_is_the_gate() {
        val open = json.decodeFromString<FullWrapper>(fullPayload).task
        assertNull(open.approvedAt)
        assertFalse(open.isApproved)

        val signedOff = json.decodeFromString<FullWrapper>(
            fullPayload.replace(""""approvedAt":null""", """"approvedAt":"2026-10-14T10:00:00Z""""),
        ).task
        assertTrue(signedOff.isApproved)
        assertEquals("2026-10-14T10:00:00Z", signedOff.approvedAt)
    }

    /** Same label rule as the queue's `WorkItem`, so one ticket reads the same
     *  in both places. */
    @Test
    fun the_label_matches_the_queues_rule() {
        val withRef = json.decodeFromString<FullWrapper>(fullPayload).task
        assertEquals("TASK-12 · Fix the login loop", withRef.label)

        val noRef = json.decodeFromString<FullWrapper>(
            fullPayload.replace("\"ticketRef\":\"TASK-12\"", "\"ticketRef\":null"),
        ).task
        assertEquals("Fix the login loop", noRef.label)

        val emptyRef = json.decodeFromString<FullWrapper>(
            fullPayload.replace("\"ticketRef\":\"TASK-12\"", "\"ticketRef\":\"\""),
        ).task
        assertEquals("Fix the login loop", emptyRef.label, "an empty ref is not a ref")
    }

    /** Both review branches answer `{task}`, and the ticket in it is the
     *  server's new truth — which is why the screen re-reads from it. */
    @Test
    fun the_review_response_decodes() {
        val approved = """{"task":{"id":"t1","status":"quality_review","approvedAt":"2026-10-14T10:00:00Z"}}"""
        val task = json.decodeFromString<TaskWrapper>(approved).task
        assertTrue(task.isApproved)
        // Approval does NOT move it: the status the api returns is unchanged.
        assertEquals("quality_review", task.status)

        val rejected = """{"task":{"id":"t1","status":"in_progress","approvedAt":null}}"""
        val sentBack = json.decodeFromString<TaskWrapper>(rejected).task
        assertFalse(sentBack.isApproved)
        assertEquals("in_progress", sentBack.status)
    }

    @Test
    fun the_comment_response_decodes() {
        val body = """{"comment":{"id":"tc9","author":"Jon","content":"which claim?","parentId":null,
                       "createdAt":"2026-10-14T10:01:00Z"}}"""
        val comment = json.decodeFromString<CommentWrapper>(body).comment
        assertEquals("which claim?", comment.content)
        assertEquals("Jon", comment.author)
    }

    /** A ticket with almost nothing set still has to render. */
    @Test
    fun a_sparse_task_decodes_to_sane_defaults() {
        val task = json.decodeFromString<Task>("""{"id":"t9"}""")
        assertEquals("", task.title)
        assertEquals("", task.status)
        assertTrue(task.assignees.isEmpty())
        assertTrue(task.tags.isEmpty())
        assertEquals(0, task.commentCount)
        assertFalse(task.isApproved)
        assertEquals("", task.label)
    }

    /** A field the api adds tomorrow must not break an app shipped today. */
    @Test
    fun unknown_fields_on_a_task_are_tolerated() {
        val task = json.decodeFromString<Task>("""{"id":"t9","somethingNew":{"a":1},"title":"x"}""")
        assertEquals("x", task.title)
    }
}
