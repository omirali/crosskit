import XCTest

/// Разбор ответа ядра. Фикстуры — синтетический вывод `crosskit search --json` той же формы,
/// что печатает `src/presentation/machine_answer.rs`; имена и пути выдуманы.
final class CoreAnswerTests: XCTestCase {

    private static let home = "/Users/alice"

    private static let page = #"""
    {"schema":2,"context":"work","query":"retry webhook","looked_for":"retry webhook","period":null,
     "results":[
      {"kind":"file","cursor":"document:3f000000:file:a","path":"/Users/alice/Projects/payments-api/src/retry.ts",
       "state":"present","fragment":"retry the webhook, then Retry once more","matched":["retry","webhook"],
       "matches":2,"topic":"платежи","conversation":null},
      {"kind":"conversation","cursor":"document:3e000000:file:b",
       "path":"/Users/alice/.claude/projects/payments-api/5b0c77de.jsonl","state":"present",
       "fragment":"ретраи webhook и cron","matched":["webhook"],"matches":1,"topic":null,
       "conversation":{"session_id":"5b0c77de-41f","name":"Ретраи webhook","message_count":34,
                       "last_message_at_unix_ms":1788419370000}},
      {"kind":"file","cursor":"document:3d000000:file:c","path":"/Users/alice/notes/old-retry.md",
       "state":"missing","fragment":"retry","matched":["retry"],"matches":1,"topic":null,"conversation":null}
     ],
     "next":"document:3d000000:file:c",
     "not_shown":{"by_policy":2,"missing":1,"lost_by_graph":0},
     "index":{"documents":120,"written_by_this_build":true,"folding_stopped_at":null}}
    """#

    private func page(scope: FeedScope = .everything) throws -> FeedPage {
        let answer = try CoreAnswer.page(from: Data(Self.page.utf8))
        return CoreAnswer.feedPage(answer, scope: scope, pageNumber: 1, home: Self.home,
                                   now: Date(timeIntervalSince1970: 1_788_500_000))
    }

    func testRowsAreGroupedByKindInTheCoresOrder() throws {
        let page = try page()
        XCTAssertEqual(page.sections.map(\.kind), [.conversation, .file])
        XCTAssertEqual(page.sections[1].rows.map(\.identity),
                       ["document:3f000000:file:a", "document:3d000000:file:c"])
        XCTAssertEqual(page.next, FeedCursor(opaque: "document:3d000000:file:c"))
        XCTAssertEqual(page.counters?.hiddenByPolicy, 2)
        XCTAssertEqual(page.counters?.missingCollapsed, 1)
    }

    func testAConversationCarriesItsSessionAndName() throws {
        guard case .conversation(let row) = try page().sections[0].rows[0] else {
            return XCTFail("первая группа — беседы")
        }
        XCTAssertEqual(row.sessionID, "5b0c77de-41f")
        XCTAssertEqual(row.title, "Ретраи webhook")
        XCTAssertEqual(row.messageCount, 34)
        XCTAssertNotNil(row.when)
    }

    func testAFileIsSplitIntoDirectoryAndNameUnderHome() throws {
        guard case .file(let row) = try page().sections[1].rows[0] else { return XCTFail("файл") }
        XCTAssertEqual(row.directory, "~/Projects/payments-api/src/")
        XCTAssertEqual(row.name, "retry.ts")
        XCTAssertEqual(row.path, "/Users/alice/Projects/payments-api/src/retry.ts")
        XCTAssertEqual(row.presence, .present)
        XCTAssertNil(row.when, "времени файла в ответе ядра нет, и панель его не придумывает")
    }

    func testTheFilterChoosesAGroupOnThePanelsSide() throws {
        XCTAssertEqual(try page(scope: .conversations).sections.map(\.kind), [.conversation])
        XCTAssertEqual(try page(scope: .files).sections.map(\.kind), [.file])
    }

    /// Совпавшее слово подсвечивается целым словом и в любом регистре, но не внутри другого.
    func testMatchedWordsBecomeWholeWordRanges() {
        let text = "Retry the retry, not retrying"
        let marked = CoreAnswer.ranges(of: ["retry"], in: text).map { String(text[$0]) }
        XCTAssertEqual(marked, ["Retry", "retry"])
    }

    func testARefusalIsAFailureOfItsCategory() {
        let refused = Data(#"{"schema":2,"error":{"category":"busy","context":"work"}}"#.utf8)
        XCTAssertThrowsError(try CoreAnswer.page(from: refused)) { error in
            XCTAssertEqual(error as? FeedFailure, .core(.busy))
        }
    }

    /// Незнакомая категория — «не получилось», а не догадка по написанию.
    func testAnUnknownCategoryIsReadAsFailed() {
        let refused = Data(#"{"schema":2,"error":{"category":"brand-new","context":null}}"#.utf8)
        XCTAssertThrowsError(try CoreAnswer.page(from: refused)) { error in
            XCTAssertEqual(error as? FeedFailure, .core(.failed))
        }
    }

    func testAnotherSchemaIsNotReadAtAll() {
        let other = Data(Self.page.replacingOccurrences(of: #""schema":2"#, with: #""schema":3"#).utf8)
        XCTAssertThrowsError(try CoreAnswer.page(from: other)) { error in
            XCTAssertEqual(error as? FeedFailure, .coreUnreadable)
        }
        XCTAssertThrowsError(try CoreAnswer.page(from: Data("crosskit failed".utf8))) { error in
            XCTAssertEqual(error as? FeedFailure, .coreUnreadable)
        }
    }

    func testTheContextsAreReadFromTheirOwnAnswer() throws {
        let listed = Data(#"{"schema":2,"contexts":["work","personal"]}"#.utf8)
        XCTAssertEqual(try CoreAnswer.contexts(from: listed), ["work", "personal"])
    }

    /// Две страницы ядра — как одна: строки подряд, курсор второй, непоказанное суммой.
    func testAPageFollowedByTheNextIsReadAsOne() throws {
        let first = try CoreAnswer.page(from: Data(Self.page.utf8))
        let joined = first.followed(by: first)
        XCTAssertEqual(joined.results.count, 6)
        XCTAssertEqual(joined.next, first.next)
        XCTAssertEqual(joined.notShown.byPolicy, 4)
        XCTAssertEqual(joined.rows(wanted: .conversations), 2)
        XCTAssertEqual(joined.rows(wanted: .files), 4)
    }
}
