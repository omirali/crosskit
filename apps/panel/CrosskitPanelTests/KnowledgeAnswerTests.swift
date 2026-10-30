import XCTest

/// Ответ ядра на `knowledge map --json` — в том виде, в каком его печатает
/// `src/presentation/machine_knowledge.rs`. Сущности и связи выдуманы.
final class KnowledgeAnswerTests: XCTestCase {

    static let map = """
    {"schema":2,"context":"demo","limit":800,"truncated":false,
     "counts":{"entities":{"shown":3,"found":3},"facts":{"shown":2,"found":2}},
     "entity_types":["concept","attribute","document"],"relation_types":["identified-by","belongs-to"],
     "roots":["/Users/alice/Projects/erp","/Users/alice/.claude/projects/erp"],
     "entities":[
      {"id":"entity:контрагент","name":"Контрагент","type":"concept","description":"Сторона договора.",
       "spellings":["Контрагент","Контрагенты"],"sources":2,"degree":2,
       "terms":["контрагент","сторон","договор"]},
      {"id":"entity:бин","name":"БИН","type":"attribute","description":"","spellings":["БИН"],"sources":1,"degree":1,
       "terms":["бин"]},
      {"id":"entity:пакет документ","name":"Пакет документов","type":"document","description":"Сканы контракта.",
       "spellings":["Пакет документов"],"sources":1,"degree":1,
       "terms":["пакет","документ","скан","контракт"]}],
     "facts":[
      {"id":"fact:a","from":"entity:контрагент","to":"entity:бин","predicate":"identified-by",
       "description":"","weight":9,"quote":"Контрагент уникален по БИН",
       "source":{"kind":"file","path":"/Users/alice/Projects/erp/45-registry/registry.md","part":1}},
      {"id":"fact:b","from":"entity:пакет документ","to":"entity:контрагент","predicate":"belongs-to",
       "description":"Пакет принадлежит одному контрагенту.","weight":7,
       "quote":"каждый пакет принадлежит одному контрагенту",
       "source":{"kind":"conversation","path":"/Users/alice/.claude/projects/erp/s.jsonl","part":4,
                 "session_id":"5b0c77de","conversation":"Разбор реестра","reply":3}}]}
    """

    func testAMapIsReadWithItsEntitiesFactsAndSources() throws {
        let map = try KnowledgeAnswer.map(from: Data(Self.map.utf8))

        XCTAssertEqual(map.context, "demo")
        XCTAssertEqual(map.counts.entities.found, 3)
        XCTAssertEqual(map.entityTypes, ["concept", "attribute", "document"])
        XCTAssertEqual(map.entity("entity:контрагент")?.spellings, ["Контрагент", "Контрагенты"])
        let chat = try XCTUnwrap(map.fact("fact:b"))
        XCTAssertEqual(chat.source.kind, .conversation)
        XCTAssertEqual(chat.source.sessionId, "5b0c77de")
        XCTAssertEqual(chat.source.reply, 3)
        XCTAssertEqual(map.fact("fact:a")?.source.sessionId, nil)
    }

    /// Ответ другого номера не читается вовсе, а отказ — это категория, не страница.
    func testAnotherSchemaOrARefusalIsNotAMap() {
        let other = Self.map.replacingOccurrences(of: "\"schema\":2", with: "\"schema\":3")
        XCTAssertThrowsError(try KnowledgeAnswer.map(from: Data(other.utf8)))
        let refused = #"{"schema":2,"error":{"category":"not-indexed","context":"demo"}}"#
        XCTAssertThrowsError(try KnowledgeAnswer.map(from: Data(refused.utf8))) { error in
            XCTAssertEqual(error as? FeedFailure, .core(.notIndexed))
        }
    }
}
