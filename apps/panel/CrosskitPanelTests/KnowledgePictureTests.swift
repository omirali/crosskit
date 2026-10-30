import XCTest

/// Что карта знаний рисует и пишет: цвет типа, размер по связям, поиск с соседями, карточки.
final class KnowledgePictureTests: XCTestCase {

    private func map() throws -> KnowledgeAnswer.Map {
        try KnowledgeAnswer.map(from: Data(KnowledgeAnswerTests.map.utf8))
    }

    private let palette = ["c0", "c1", "c2", "c3"]

    /// Цвет — место типа в словаре конфигурации, а не в этой карте: один тип — один цвет.
    func testATypeIsColouredByItsPlaceInTheVocabulary() throws {
        let picture = KnowledgePicture.of(try map(), palette: palette)
        XCTAssertEqual(picture.nodes.map(\.color), ["c0", "c1", "c2"])
        XCTAssertEqual(KnowledgePicture.color(of: "concept", in: ["x", "concept"], palette: palette), "c1")
        XCTAssertEqual(KnowledgePicture.color(of: "unknown", in: ["concept"], palette: palette), "c3")
        XCTAssertEqual(KnowledgePicture.color(of: "t5", in: ["t0", "t1", "t2", "t3", "t4", "t5"], palette: palette), "c1")
    }

    func testANodeGrowsWithItsFactsAndStopsAtACeiling() {
        XCTAssertEqual(KnowledgePicture.size(degree: 0), 12)
        XCTAssertLessThan(KnowledgePicture.size(degree: 1), KnowledgePicture.size(degree: 4))
        XCTAssertEqual(KnowledgePicture.size(degree: 400), 46)
    }

    func testEveryFactIsALineBetweenTwoEntitiesByID() throws {
        let picture = KnowledgePicture.of(try map(), palette: palette)
        XCTAssertEqual(picture.edges.map(\.source), ["entity:контрагент", "entity:пакет документ"])
        XCTAssertEqual(picture.edges.map(\.label), ["identified-by", "belongs-to"])
        let ids = Set(picture.nodes.map(\.id))
        XCTAssertTrue(picture.edges.allSatisfy { ids.contains($0.source) && ids.contains($0.target) })
    }

    /// Поиск находит по любому написанию и подсвечивает соседей; одна буква не ищется.
    func testASearchFindsAnySpellingAndLightsItsNeighbours() throws {
        let map = try map()
        let found = KnowledgePicture.found("контрагенты", in: map)
        XCTAssertEqual(found.hits, ["entity:контрагент"])
        XCTAssertEqual(found.near, ["entity:бин", "entity:пакет документ"])
        XCTAssertEqual(KnowledgePicture.found("к", in: map), .nothing)
    }

    /// По основе, в имени, написаниях и описании: «контракты» — основа «контракт» описания
    /// пакета; «договор» — описания контрагента. Совпавшие по имени — первыми.
    func testASearchFindsAnyFormOfAWordInNamesAndDescriptions() throws {
        let map = try map()
        XCTAssertEqual(KnowledgePicture.found("контракты", in: map).hits, ["entity:пакет документ"])
        XCTAssertEqual(KnowledgePicture.found("договоров", in: map).hits, ["entity:контрагент"])
        XCTAssertEqual(KnowledgePicture.found("документ", in: map).hits, ["entity:пакет документ"])
        XCTAssertEqual(KnowledgePicture.found("пакеты документов", in: map).hits, ["entity:пакет документ"])
    }

    /// Русское слово находит латинскую основу по созвучию — имена таблиц пишутся по-английски.
    /// Синоним — нет: «договор» и «контракт» созвучием не связаны.
    func testARussianWordFindsALatinTermBySound() throws {
        let base = try map()
        let table = KnowledgeAnswer.Entity(id: "entity:supplier_contracts", name: "supplier_contracts", type: "dataset",
                                           description: "", spellings: ["supplier_contracts"], sources: 1, degree: 1,
                                           terms: ["supplier", "contract"])
        let map = KnowledgeAnswer.Map(context: base.context, limit: 400, truncated: false, counts: base.counts,
                                      entityTypes: base.entityTypes, relationTypes: base.relationTypes,
                                      roots: [], entities: [table], facts: [])
        let found = KnowledgePicture.found("контракты", in: map)
        XCTAssertEqual(found.hits, ["entity:supplier_contracts"])
        XCTAssertTrue(found.bySound)
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "контракты", found: found),
                       "Найдено по созвучию: supplier_contracts · 0 соседей.")
        XCTAssertEqual(KnowledgePicture.found("договор", in: map).hits, [])
        XCTAssertFalse(KnowledgePicture.soundsLike("bin", "bin"), "short stems are not compared")
        XCTAssertFalse(KnowledgePicture.soundsLike("kontrakt", "point"))
    }

    /// Ничего не нашлось — ближайшие имена, а не пустая строка.
    func testNothingFoundNamesTheClosestNames() throws {
        let map = try map()
        let found = KnowledgePicture.found("контрогент", in: map)
        XCTAssertEqual(found.hits, [])
        XCTAssertEqual(found.closest.first, "Контрагент")
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "контрогент", found: found),
                       "«контрогент» — на карте нет; ближе всего: Контрагент, Пакет документов, БИН.")
        XCTAssertEqual(KnowledgePicture.edits("кот", "кит"), 1)
    }

    /// Имена самых связанных подписаны всегда, остальные — на приближении.
    func testTheMostConnectedNamesArePinned() throws {
        let base = try map()
        let many = (0..<25).map { index in
            KnowledgeAnswer.Entity(id: "e\(index)", name: "Сущность \(index)", type: "concept", description: "",
                                   spellings: ["Сущность \(index)"], sources: 1, degree: index, terms: [])
        }
        let map = KnowledgeAnswer.Map(context: base.context, limit: 400, truncated: false, counts: base.counts,
                                      entityTypes: base.entityTypes, relationTypes: base.relationTypes,
                                      roots: [], entities: many, facts: [])
        let pinned = KnowledgePicture.of(map, palette: palette).nodes.filter(\.pinned).map(\.id)
        XCTAssertEqual(pinned.count, KnowledgePicture.pinnedNames)
        XCTAssertTrue(pinned.contains("e24") && !pinned.contains("e4"))
    }

    /// Скрытый тип уходит с карты вместе со своими связями; папка оставляет связи из её
    /// источников и сущности, которых они касаются.
    func testAFilterKeepsOnlyWhatItAdmits() throws {
        let map = try map()
        let noAttributes = KnowledgePicture.filtered(map, by: .init(hiddenTypes: ["attribute"], folder: nil))
        XCTAssertEqual(noAttributes.facts.map(\.id), ["fact:b"])
        XCTAssertNil(noAttributes.entity("entity:бин"))

        let task = KnowledgePicture.filtered(map, by: .init(hiddenTypes: [], folder: "/Users/alice/Projects/erp/45-registry"))
        XCTAssertEqual(task.facts.map(\.id), ["fact:a"])
        XCTAssertEqual(Set(task.entities.map(\.id)), ["entity:контрагент", "entity:бин"])
        XCTAssertEqual(KnowledgePicture.filtered(map, by: .everything), map)
    }

    func testTheFoldersToChooseAreRootsAndTheirFirstLevel() throws {
        let folders = KnowledgePicture.folders(of: try map()).map(\.folder)
        XCTAssertEqual(Set(folders), ["/Users/alice/Projects/erp", "/Users/alice/Projects/erp/45-registry",
                                      "/Users/alice/.claude/projects/erp"])
    }

    func testAFactCardHasItsQuoteAndWhereItWasRead() throws {
        let map = try map()
        let file = KnowledgeWording.card(of: try XCTUnwrap(map.fact("fact:a")), in: map)
        XCTAssertEqual(file.title, "Контрагент —identified-by→ БИН")
        XCTAssertEqual(file.lines.first, "«Контрагент уникален по БИН»")
        XCTAssertTrue(file.lines.last?.hasSuffix("45-registry/registry.md · часть 1") ?? false, file.lines.description)
        XCTAssertEqual(file.action, "Открыть файл")

        let chat = KnowledgeWording.card(of: try XCTUnwrap(map.fact("fact:b")), in: map)
        XCTAssertEqual(chat.lines.last, "беседа «Разбор реестра» · реплика 3")
        XCTAssertEqual(chat.action, "Скопировать claude --resume")
    }

    func testAnEntityCardSaysItsOtherSpellingsAndHowConnectedItIs() throws {
        let card = KnowledgeWording.card(of: try XCTUnwrap(try map().entity("entity:контрагент")))
        XCTAssertEqual(card.type, "concept")
        XCTAssertEqual(card.lines, ["Сторона договора.", "также: Контрагенты", "2 связи · 2 источника"])
    }

    func testTheLineAboveTheMapSaysWhatWasFound() throws {
        let map = try map()
        XCTAssertEqual(KnowledgeWording.counts(map), "3 сущности · 2 связи")
        let found = KnowledgePicture.found("бин", in: map)
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "бин", found: found),
                       "Найдено: БИН · 1 сосед.")
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "zz", found: .nothing),
                       "«zz» — на карте нет.")
        let several = KnowledgePicture.found("кон", in: map)
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "кон", found: several),
                       "Найдено 2: Контрагент, Пакет документов · 1 сосед. ↑↓ — по найденным.")
    }

    /// За пределом карта держит самые связанные; чего на ней нет, то названо, и сказано где искать.
    func testACutMapSaysWhereToLookForWhatItDoesNotHold() throws {
        let cut = KnowledgeAnswerTests.map
            .replacingOccurrences(of: "\"truncated\":false", with: "\"truncated\":true")
            .replacingOccurrences(of: "\"entities\":{\"shown\":3,\"found\":3}",
                                  with: "\"entities\":{\"shown\":3,\"found\":900}")
        let map = try KnowledgeAnswer.map(from: Data(cut.utf8))
        XCTAssertEqual(KnowledgeWording.counts(map),
                       "3 сущности · 2 связи · показано 3 из 900 — самые связанные")
        XCTAssertEqual(KnowledgeWording.explanation(map, query: "договор", found: .nothing),
                       "«договор» — на карте нет. Карта держит 3 самых связанных; остальные — `crosskit knowledge show договор`.")
    }

    /// Вызов карты идёт её объекту страницы, данные — значением аргумента.
    func testACallReachesTheKnowledgePageAndCarriesDataAsAnArgument() {
        let call = GraphPageCall.highlight(KnowledgePicture.Found(hits: ["entity:a"], near: [], closest: ["b"]))
        XCTAssertEqual(call.body, "window.crosskitKnowledge.highlight(payload)")
        XCTAssertEqual(call.arguments["payload"], #"{"hits":["entity:a"],"near":[]}"#)
        XCTAssertEqual(GraphPageCall.select("x").body, "window.crosskitGraph.select(payload)")
    }
}
