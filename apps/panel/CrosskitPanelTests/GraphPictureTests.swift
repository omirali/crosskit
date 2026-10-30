import XCTest

/// Перевод вида ядра в элементы страницы и то, что страница о нём скажет словами. Без WebKit:
/// что рисовать, решено здесь, а страница только раскладывает.
final class GraphPictureTests: XCTestCase {

    private func view() throws -> GraphAnswer.View {
        try GraphAnswer.view(from: Data(GraphAnswerTests.view.utf8))
    }

    /// Реплик на холсте нет: цепочка «беседа → реплика → файл» — одна линия от беседы.
    func testRepliesAreNotDrawnAndTheirTouchIsALineFromTheConversation() throws {
        let picture = GraphPicture.of(try view())
        XCTAssertFalse(picture.nodes.contains { $0.kind == "reply" })
        let touched = picture.edges.filter { $0.kind == "touched" }
        XCTAssertEqual(touched.map(\.source), ["conversation:5b0c77de", "conversation:5b0c77de"])
        XCTAssertEqual(Set(touched.map(\.target)), ["file:f1", "outside:/other/registry.rs"])
    }

    func testATouchIsLabelledWithWhatTheConversationDidAndWhetherYouNamedIt() throws {
        let picture = GraphPicture.of(try view())
        let toFile = try XCTUnwrap(picture.edges.first { $0.target == "file:f1" })
        XCTAssertEqual(toFile.label, "читала, правила ×3 · вы назвали")
        XCTAssertTrue(toFile.named)
        let outside = try XCTUnwrap(picture.edges.first { $0.target == "outside:/other/registry.rs" })
        XCTAssertEqual(outside.label, "правила")
        XCTAssertEqual(GraphPicture.touchLabel([GraphAnswer.Touch(verbs: ["wrote"], named: false, times: 1, byTime: false)]),
                       "создала")
    }

    /// Вне контекста — состояние файла, тот же вид узла.
    func testAFileOutsideTheContextIsAFileInThatState() throws {
        let picture = GraphPicture.of(try view())
        let outside = try XCTUnwrap(picture.nodes.first { $0.id == "outside:/other/registry.rs" })
        XCTAssertEqual(outside.kind, "file")
        XCTAssertEqual(outside.state, "outside")
    }

    /// Проект — слева от узла, к которому относится: линия идёт от проекта. У файла того же
    /// проекта, что его беседа, своей линии нет — она перечеркнула бы граф.
    func testAProjectStandsLeftOfWhatLiesInIt() throws {
        let picture = GraphPicture.of(try view())
        let project = try XCTUnwrap(picture.edges.first { $0.kind == "in-project" })
        XCTAssertEqual(project.source, "project:/Users/alice/Projects/payments-api")
        XCTAssertEqual(project.target, "file:f1", "у беседы проекта нет — линия к файлу нужна")

        let base = try view()
        let shared = GraphAnswer.View(
            context: base.context, focus: base.focus, depth: base.depth, limit: base.limit, nodes: base.nodes,
            edges: base.edges + [GraphAnswer.Edge(from: "conversation:5b0c77de",
                                                  to: "project:/Users/alice/Projects/payments-api",
                                                  kind: "in-project", touch: nil)],
            truncated: base.truncated, counts: base.counts, notShown: base.notShown)
        let targets = GraphPicture.of(shared).edges.filter { $0.kind == "in-project" }.map(\.target)
        XCTAssertEqual(targets, ["conversation:5b0c77de"])
    }

    func testALongNameIsShortOnTheCanvasAndWholeInTheCard() {
        let long = String(repeating: "договор ", count: 6)
        XCTAssertEqual(GraphPicture.short(long).count, GraphPicture.labelLimit)
        XCTAssertTrue(GraphPicture.short(long).hasSuffix("…"))
        XCTAssertEqual(GraphPicture.short("payments-api"), "payments-api")
    }

    /// Реплика, найденная по времени, так и названа — в карточке файла, а не подана фактом.
    func testTheCardSaysAReplyWasPlacedByTime() throws {
        let view = try view()
        let file = try XCTUnwrap(view.node("file:f1"))
        let card = GraphWording.card(of: file, in: view, now: Date(timeIntervalSince1970: 1_788_500_000))
        XCTAssertTrue(card.detail.contains("реплика #18"), card.detail)
        XCTAssertTrue(card.detail.contains("определена по времени"), card.detail)
        XCTAssertEqual(card.type, "Файл · нет на диске")
    }

    /// Второй шаг, который ничего не нашёл, называется, а не прячется.
    func testAnEmptySecondStepIsSaid() throws {
        let view = try view()
        XCTAssertEqual(GraphWording.emptiness(view), nil, "найдено больше, чем показано: срез, не пустота")
        let alone = GraphAnswer.View(
            context: view.context, focus: view.focus, depth: 2, limit: 100, nodes: view.nodes,
            edges: view.edges, truncated: false,
            counts: GraphAnswer.Counts(conversation: .init(shown: 1, found: 1), reply: view.counts.reply,
                                       file: view.counts.file, project: view.counts.project,
                                       topic: view.counts.topic),
            notShown: view.notShown)
        XCTAssertEqual(GraphWording.emptiness(alone), "У этих файлов других бесед нет.")
    }

    /// Беседа без касаний — не пустой экран, а сказанное словами.
    func testAConversationThatTouchedNothingSaysSo() throws {
        let view = try view()
        let bare = GraphAnswer.View(
            context: view.context, focus: view.focus, depth: 2, limit: 100,
            nodes: view.nodes.filter { $0.kind == .conversation }, edges: [], truncated: false,
            counts: view.counts, notShown: GraphAnswer.NotShown(inPassing: 2, byPolicy: 0))
        XCTAssertEqual(GraphWording.emptiness(bare),
                       "Эта беседа не правила файлов и не называла их. Прочитанных мимоходом — 2, они свёрнуты.")
    }

    /// Данные уходят странице значением аргумента: тело вызова от них не зависит, и строка из
    /// чужого файла не становится кодом.
    func testDataReachesThePageAsAJSONValueNotAsCode() throws {
        let hostile = "\"); window.location = \"https://example.com\"; (\""
        let call = GraphPageCall.select(hostile)
        XCTAssertEqual(call.body, "window.crosskitGraph.select(payload)")
        XCTAssertFalse(call.body.contains(hostile))
        let payload = try XCTUnwrap(call.arguments["payload"])
        let decoded = try JSONSerialization.jsonObject(with: Data(payload.utf8)) as? [String: String]
        XCTAssertEqual(decoded?["id"], hostile)

        let theme = GraphTheme(fg: "a", fg2: "a", fg3: "a", conversation: "a", file: "a", project: "a",
                               topic: "a", warn: "a", edge: "a", accent: "a", solid: "a")
        let render = GraphPageCall.render(GraphPicture.of(try view()), theme: theme, generation: 7, selected: nil)
        XCTAssertEqual(render.body, "window.crosskitGraph.render(payload)")
        let object = try JSONSerialization.jsonObject(with: Data(try XCTUnwrap(render.arguments["payload"]).utf8))
            as? [String: Any]
        XCTAssertEqual(object?["generation"] as? Int, 7)
    }
}
