package app.talaria.mobile.net

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The comms wire contract: `MemberChannel` and `ChannelMessageWire` from
 * `api/crates/talaria-channels/src/lib.rs`, both `rename_all = "camelCase"`.
 *
 * The named-wrapper envelope is part of the contract, not decoration — a read
 * returns `{channels}` or `{messages}`, never a bare array
 * (`docs/API-CONVENTIONS.md`). Decoding the wrapper here is what would catch an
 * api that changed its mind.
 */
@Serializable
private data class ChannelsWrapper(val channels: List<Channel> = emptyList())

@Serializable
private data class MessagesWrapper(val messages: List<Message> = emptyList())

class CommsParseTest {
    private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

    private val channelsPayload = """
        {"channels": [
          {"id":"c1","name":"platform","topic":"the api and the app","kind":"channel","role":"member",
           "createdAt":"2026-09-01T10:00:00Z","updatedAt":"2026-10-08T09:00:00Z","unreadCount":3,"peer":null},
          {"id":"c2","name":"dm-7f3a","topic":null,"kind":"dm","role":"member",
           "createdAt":"2026-09-02T10:00:00Z","updatedAt":"2026-10-08T08:00:00Z","unreadCount":1,
           "peer":{"userId":"u-muse","name":"Muse","email":null},
           "members":[{"userId":"u-muse","name":"Muse","email":null}],
           "agents":["Muse"]}
        ]}
        """

    private val messagesPayload = """
        {"messages": [
          {"id":"m1","seq":1,"authorType":"user","author":"Jon","content":"Did the gate clear?",
           "status":"sent","createdAt":"2026-10-08T09:00:00Z","attachments":[],"guard":null,
           "threadRootId":null,"editedAt":null,"chips":[]},
          {"id":"m2","seq":2,"authorType":"agent","author":"Muse",
           "content":"Not yet — the judge flagged one claim.","status":"sent",
           "createdAt":"2026-10-08T09:01:00Z","attachments":[],"guard":null,
           "threadRootId":null,"editedAt":"2026-10-08T09:02:00Z","chips":[],
           "reactions":[{"emoji":"👀","count":2}]},
          {"id":"m3","seq":3,"authorType":"user","author":"Sam","content":"which claim?",
           "status":"sent","createdAt":"2026-10-08T09:03:00Z","attachments":[],"guard":null,
           "threadRootId":"m2","editedAt":null,"chips":[]}
        ]}
        """

    @Test
    fun the_channel_list_decodes_with_its_camelcase_fields() {
        val channels = json.decodeFromString<ChannelsWrapper>(channelsPayload).channels
        assertEquals(2, channels.size)
        assertEquals(3, channels[0].unreadCount)
        assertEquals("2026-10-08T09:00:00Z", channels[0].updatedAt)
        assertEquals("the api and the app", channels[0].topic)
    }

    /** A channel is `#name`; a DM is named by whoever is in it. */
    @Test
    fun labels_follow_what_people_call_the_room() {
        val channels = json.decodeFromString<ChannelsWrapper>(channelsPayload).channels
        assertEquals("#platform", channels[0].label)
        assertEquals("Muse", channels[1].label)
        assertTrue(channels[1].isDm)
        assertEquals(listOf("Muse"), channels[1].agents)
    }

    /** A DM with no peer and no members still has to read as something. */
    @Test
    fun a_dm_with_nothing_to_name_it_falls_back_rather_than_going_blank() {
        val bare = json.decodeFromString<Channel>("""{"id":"c9","name":"","kind":"dm","unreadCount":0}""")
        assertEquals("Direct message", bare.label)
        val named = json.decodeFromString<Channel>("""{"id":"c9","name":"dm-x","kind":"dm","unreadCount":0}""")
        assertEquals("dm-x", named.label)
    }

    /** A peer with no display name is identified by email, then by id — never
     *  by nothing. */
    @Test
    fun a_peer_is_shown_by_the_best_handle_it_has() {
        assertEquals(
            "Muse",
            json.decodeFromString<ChannelPeer>("""{"userId":"u1","name":"Muse","email":"m@x.com"}""").shown,
        )
        assertEquals(
            "m@x.com",
            json.decodeFromString<ChannelPeer>("""{"userId":"u1","name":null,"email":"m@x.com"}""").shown,
        )
        assertEquals("u1", json.decodeFromString<ChannelPeer>("""{"userId":"u1"}""").shown)
    }

    @Test
    fun messages_decode_including_who_is_an_agent() {
        val messages = json.decodeFromString<MessagesWrapper>(messagesPayload).messages
        assertEquals(3, messages.size)
        assertTrue(messages[1].fromAgent, "authorType=agent is how a colleague is known to be one")
        assertTrue(!messages[0].fromAgent)
        assertEquals("2026-10-08T09:02:00Z", messages[1].editedAt)
    }

    @Test
    fun a_thread_reply_is_identifiable_and_the_others_are_not() {
        val messages = json.decodeFromString<MessagesWrapper>(messagesPayload).messages
        assertTrue(messages[2].isThreadReply)
        assertEquals("m2", messages[2].threadRootId)
        assertTrue(messages.count { !it.isThreadReply } == 2)
    }

    /**
     * `guard`, `attachments`, `chips` and `reactions` are all on the wire and
     * none are modelled. That has to be a decode that SUCCEEDS and drops them,
     * not one that fails — a field modelled but unrendered is a field nobody
     * notices breaking, and `guard` in particular carries a verbatim excerpt of
     * flagged content that a reading surface has no business holding.
     */
    @Test
    fun unmodelled_fields_are_dropped_rather_than_fatal() {
        val messages = json.decodeFromString<MessagesWrapper>(messagesPayload).messages
        assertEquals("Did the gate clear?", messages[0].content)
        assertEquals(3, messages.size)
    }

    /** The insert response omits `guard` and `editedAt` entirely rather than
     *  nulling them — the api says so explicitly, and one model has to read
     *  both shapes. */
    @Test
    fun the_insert_shape_decodes_as_well_as_the_page_shape() {
        val inserted = """
            {"message":{"id":"m9","seq":9,"authorType":"user","author":"Jon","content":"ship it",
             "status":"sent","createdAt":"2026-10-08T10:00:00Z","attachments":[],
             "threadRootId":null,"chips":[]}}
            """
        @Serializable
        data class Wrapper(val message: Message)
        val message = json.decodeFromString<Wrapper>(inserted).message
        assertEquals("ship it", message.content)
        assertNull(message.editedAt, "absent and null must read the same to a client")
    }

    @Test
    fun an_empty_instance_decodes_to_empty_lists() {
        assertTrue(json.decodeFromString<ChannelsWrapper>("""{"channels":[]}""").channels.isEmpty())
        assertTrue(json.decodeFromString<MessagesWrapper>("""{"messages":[]}""").messages.isEmpty())
        // And a wrapper whose key is missing entirely must not throw.
        assertTrue(json.decodeFromString<ChannelsWrapper>("""{}""").channels.isEmpty())
    }
}
