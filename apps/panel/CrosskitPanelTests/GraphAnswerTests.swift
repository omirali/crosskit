import XCTest

/// Разбор ответа `crosskit graph --json`. Фикстура — синтетический вывод той же формы, что
/// печатает `src/presentation/machine_graph.rs`; имена и пути выдуманы.
final class GraphAnswerTests: XCTestCase {

    static let view = #"""
    {"schema":2,"context":"work","focus":"conversation:5b0c77de","depth":2,"limit":100,
     "nodes":[
      {"id":"conversation:5b0c77de","kind":"conversation","label":"Договоры в payments-api","depth":0,
       "focusable":true,"provenance":{"record":"Conversation","key":"conversation:rev1"},
       "conversation":{"session_id":"5b0c77de","message_count":2016,"replies_not_shown":2015,
                       "last_message_at_unix_ms":1788419370000},
       "reply":null,"file":null,"outside":null,"project":null},
      {"id":"reply:5b0c77de:18","kind":"reply","label":"#18","depth":1,"focusable":false,
       "provenance":{"record":"Message","key":"conversation:rev1:m18"},"conversation":null,
       "reply":{"conversation":"conversation:5b0c77de","ordinal":18,"speaker":"model","at_unix_ms":1788419000000},
       "file":null,"outside":null,"project":null},
      {"id":"file:f1","kind":"file","label":"contract.rs","depth":1,"focusable":true,
       "provenance":{"record":"File","key":"f1"},"conversation":null,"reply":null,
       "file":{"path":"/Users/alice/Projects/payments-api/src/contract.rs","state":"missing"},
       "outside":null,"project":null},
      {"id":"outside:/other/registry.rs","kind":"file","label":"registry.rs","depth":1,"focusable":false,
       "provenance":{"record":"FileTouch","key":"/other/registry.rs"},"conversation":null,"reply":null,
       "file":{"path":"/other/registry.rs","state":"outside"},"project":null},
      {"id":"project:/Users/alice/Projects/payments-api","kind":"project","label":"payments-api","depth":1,
       "focusable":false,"provenance":{"record":"configuration","key":"include /Users/alice/Projects/payments-api"},
       "conversation":null,"reply":null,"file":null,"outside":null,
       "project":{"root":"/Users/alice/Projects/payments-api"}}
     ],
     "edges":[
      {"from":"conversation:5b0c77de","to":"reply:5b0c77de:18","kind":"has-reply","touch":null},
      {"from":"reply:5b0c77de:18","to":"file:f1","kind":"touched","touch":{"verbs":["read","edited"],"named":true,"times":3,"by_time":true}},
      {"from":"conversation:5b0c77de","to":"outside:/other/registry.rs","kind":"touched","touch":{"verbs":["edited"],"named":false,"times":1,"by_time":false}},
      {"from":"file:f1","to":"project:/Users/alice/Projects/payments-api","kind":"in-project","touch":null}
     ],
     "truncated":true,
     "counts":{"conversation":{"shown":1,"found":3},"reply":{"shown":1,"found":9},"file":{"shown":2,"found":34},"project":{"shown":1,"found":1},"topic":{"shown":0,"found":0}},
     "not_shown":{"in_passing":7,"by_policy":1},
     "provenance_summary":[{"record":"Conversation","nodes":1}]}
    """#

    func testAViewCarriesItsNodesEdgesAndCounts() throws {
        let view = try GraphAnswer.view(from: Data(Self.view.utf8))
        XCTAssertEqual(view.focus, "conversation:5b0c77de")
        XCTAssertEqual(view.nodes.map(\.kind), [.conversation, .reply, .file, .file, .project])
        XCTAssertEqual(view.node("file:f1")?.file?.state, .missing)
        XCTAssertEqual(view.node("conversation:5b0c77de")?.conversation?.repliesNotShown, 2015)
        XCTAssertEqual(view.edges[1].touch,
                       GraphAnswer.Touch(verbs: ["read", "edited"], named: true, times: 3, byTime: true))
        XCTAssertTrue(view.truncated)
        XCTAssertEqual(view.counts[.file], GraphAnswer.Count(shown: 2, found: 34))
        XCTAssertEqual(view.counts.shown, 5)
        XCTAssertEqual(view.counts.found, 47)
        XCTAssertEqual(view.notShown.inPassing, 7)
        XCTAssertFalse(view.node("outside:/other/registry.rs")?.focusable ?? true)
        XCTAssertEqual(view.node("outside:/other/registry.rs")?.file?.state, .outside)
    }

    func testAnAnswerOfAnotherSchemaIsNotRead() {
        let other = Self.view.replacingOccurrences(of: #""schema":2"#, with: #""schema":3"#)
        XCTAssertThrowsError(try GraphAnswer.view(from: Data(other.utf8))) { error in
            XCTAssertEqual(error as? FeedFailure, .coreUnreadable)
        }
    }

    func testAFocusTheContextDoesNotHoldIsARefusalNotAnEmptyGraph() {
        let refused = #"{"schema":2,"error":{"category":"not-found","context":"work"}}"#
        XCTAssertThrowsError(try GraphAnswer.view(from: Data(refused.utf8))) { error in
            XCTAssertEqual(error as? FeedFailure, .core(.notFound))
        }
    }

    /// ⇥ на строке спрашивает ядро тем, что строка уже несёт: сессией беседы или путём файла.
    func testASearchRowGivesItsFocus() {
        let nothing = Fragment(text: "", matches: [])
        let conversation = FeedRow.conversation(ConversationRow(
            id: "c", cursor: FeedCursor(opaque: "c"), tool: .claudeCode, title: "t", fragment: nothing,
            project: nil, when: nil, messageCount: 1, topic: nil, sessionID: "5b0c77de"))
        let file = FeedRow.file(FileRow(
            id: "f", cursor: FeedCursor(opaque: "f"), path: "/Users/alice/a.rs", directory: "~/", name: "a.rs",
            fragment: nothing, matched: [], presence: .present, project: nil, when: nil, topic: nil))
        XCTAssertEqual(GraphAnswer.focus(of: conversation), "conversation:5b0c77de")
        XCTAssertEqual(GraphAnswer.focus(of: file), "path:/Users/alice/a.rs")
    }
}
