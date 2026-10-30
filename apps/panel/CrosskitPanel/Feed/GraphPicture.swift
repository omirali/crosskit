import Foundation

/// Что страница графа рисует: вид ядра, переведённый в элементы Cytoscape.
///
/// Перевод — здесь, а не в странице, и без WebKit: страница только раскладывает и рисует то,
/// что ей дали, а что нарисовать — решено в коде, который видят тесты.
///
/// Три решения этапа UI-4b живут в этом переводе:
///
/// * **реплики узлами не рисуются.** Цепочка «беседа → реплика → файл» становится одной линией
///   «беседа → файл» с подписью глагола («правила ×12 · вы назвали»). Номер реплики и момент —
///   в карточке файла, с пометкой, что реплика найдена по времени (`by_time` в ответе ядра);
/// * **линии идут слева направо по шагам**: от узла ближе к фокусу к дальнему, проект и тема —
///   слева от своего узла. Так dagre ставит проект, беседу, её файлы и другие беседы колонками;
/// * **подпись узла короткая**, полное имя — в карточке.
struct GraphPicture: Encodable, Equatable {

    struct Node: Encodable, Equatable {
        let id: String
        /// `conversation`, `file`, `project`, `topic`.
        let kind: String
        let label: String
        /// У файла: `present`, `missing`, `outside`; у остальных — `nil`.
        let state: String?
        let focus: Bool
    }

    struct Edge: Encodable, Equatable {
        let id: String
        let source: String
        let target: String
        /// `touched`, `in-project`, `about`.
        let kind: String
        let label: String
        let named: Bool
        /// Беседа правила или создала файл; `false` — только читала. Рисуется сплошной или точками.
        let changed: Bool
    }

    let nodes: [Node]
    let edges: [Edge]
    /// Подписи линий — только у выбранного узла. Сорок пять подписей подряд — стена текста, а не
    /// граф (снимок UI-4b, беседа на 2 016 реплик); полная подпись всегда есть в карточке.
    let quietLabels: Bool

    /// Больше стольких линий касания — подписи только у выбранного.
    static let loudestLabels = 12

    /// Самая длинная подпись узла на холсте; длиннее — с многоточием, целиком — в карточке.
    static let labelLimit = 28

    static func of(_ view: GraphAnswer.View) -> GraphPicture {
        let drawn = view.nodes.filter { $0.kind != .reply }
        let nodes = drawn.map { node in
            Node(id: node.id, kind: node.kind.rawValue, label: short(GraphPicture.label(of: node)),
                 state: node.file?.state.rawValue, focus: node.id == view.focus)
        }
        let present = Set(drawn.map(\.id))
        let depth = Dictionary(uniqueKeysWithValues: view.nodes.map { ($0.id, $0.depth) })

        var touched: [(source: String, target: String, touches: [GraphAnswer.Touch])] = []
        var attached: [Edge] = []
        for edge in view.edges {
            switch edge.kind {
            case "touched":
                guard let touch = edge.touch else { continue }
                // Линия от реплики — это линия от её беседы.
                let conversation = view.node(edge.from)?.reply?.conversation ?? edge.from
                guard present.contains(conversation), present.contains(edge.to) else { continue }
                // От ближнего к фокусу — к дальнему; при равенстве — от беседы к файлу.
                let fileFirst = (depth[edge.to] ?? 0) < (depth[conversation] ?? 0)
                let (source, target) = fileFirst ? (edge.to, conversation) : (conversation, edge.to)
                if let at = touched.firstIndex(where: { $0.source == source && $0.target == target }) {
                    touched[at].touches.append(touch)
                } else {
                    touched.append((source, target, [touch]))
                }
            case "in-project", "about":
                guard present.contains(edge.from), present.contains(edge.to) else { continue }
                // Проект и тема — слева от узла, к которому относятся.
                attached.append(Edge(id: "\(edge.kind):\(edge.to)>\(edge.from)", source: edge.to,
                                     target: edge.from, kind: edge.kind, label: "", named: false, changed: true))
            default:
                continue
            }
        }
        // Проект файла, который совпадает с проектом его беседы, своей линии не получает: она
        // повторила бы уже сказанное и перечеркнула граф (первый снимок UI-4b).
        let projectOf = Dictionary(attached.filter { $0.kind == "in-project" }.map { ($0.target, $0.source) },
                                   uniquingKeysWith: { first, _ in first })
        let conversations = Set(view.nodes.filter { $0.kind == .conversation }.map(\.id))
        attached.removeAll { edge in
            guard edge.kind == "in-project", !conversations.contains(edge.target) else { return false }
            return touched.contains { pair in
                let other = pair.source == edge.target ? pair.target : (pair.target == edge.target ? pair.source : nil)
                return other.map { projectOf[$0] == edge.source } ?? false
            }
        }
        let touchEdges = touched.map { pair in
            Edge(id: "touched:\(pair.source)>\(pair.target)", source: pair.source, target: pair.target,
                 kind: "touched", label: touchLabel(pair.touches), named: pair.touches.contains(where: \.named),
                 changed: pair.touches.contains { $0.verbs.contains { $0 != "read" } })
        }
        return GraphPicture(nodes: nodes, edges: touchEdges + attached,
                            quietLabels: touchEdges.count > loudestLabels)
    }

    /// «правила ×12 · вы назвали». Подлежащее — беседа: она правила, создала, читала.
    static func touchLabel(_ touches: [GraphAnswer.Touch]) -> String {
        var verbs: [String] = []
        for verb in touches.flatMap(\.verbs) where !verbs.contains(verb) { verbs.append(verb) }
        var label = verbs.map(verbWord).joined(separator: ", ")
        let times = touches.reduce(0) { $0 + $1.times }
        if times > 1 { label += " ×\(times)" }
        if touches.contains(where: \.named) { label += " · вы назвали" }
        return label
    }

    static func verbWord(_ verb: String) -> String {
        switch verb {
        case "read": return "читала"
        case "wrote": return "создала"
        case "edited": return "правила"
        default: return verb
        }
    }

    /// Подпись узла целиком — то, что потом сокращается для холста.
    static func label(of node: GraphAnswer.Node) -> String {
        switch node.kind {
        case .conversation:
            return node.label.isEmpty ? "Без названия" : node.label
        default:
            return node.label
        }
    }

    static func short(_ text: String, limit: Int = labelLimit) -> String {
        guard text.count > limit else { return text }
        return String(text.prefix(limit - 1)) + "…"
    }
}

/// Тема страницы: цвета токенов в текущем оформлении, строками CSS. Страница своих цветов не
/// знает — иначе тёмная тема окна и белый холст расходились бы, как в первом снимке владельца.
struct GraphTheme: Encodable, Equatable {
    let fg: String
    let fg2: String
    let fg3: String
    let conversation: String
    let file: String
    let project: String
    let topic: String
    let warn: String
    let edge: String
    let accent: String
    let solid: String
}

/// Вызов страницы. Тело — постоянная строка; данные идут **значением** аргумента
/// `callAsyncJavaScript` и разбираются в странице `JSON.parse`, а не вклеиваются в код: строка
/// из чужого файла не может стать кодом страницы.
struct GraphPageCall: Equatable {

    /// Какая страница отвечает: граф окрестности или карта знаний. У каждой свой объект в
    /// `window`, и вызов одной не может попасть в другую.
    enum Page: String {
        case graph = "crosskitGraph"
        case knowledge = "crosskitKnowledge"
    }

    enum Function: String {
        case render
        case select
        case highlight
        case zoom
        case panBy
        case zoomBy
        case step
        case center
    }

    var page: Page = .graph
    let function: Function
    let payload: String

    /// Что исполняет страница. От данных не зависит.
    var body: String { "window.\(page.rawValue).\(function.rawValue)(payload)" }
    var arguments: [String: String] { ["payload": payload] }

    static func render(_ picture: GraphPicture, theme: GraphTheme, generation: Int, selected: String?) -> GraphPageCall {
        struct Render: Encodable {
            let picture: GraphPicture
            let theme: GraphTheme
            let generation: Int
            let selected: String?
        }
        return GraphPageCall(function: .render, payload: encode(Render(picture: picture, theme: theme,
                                                                         generation: generation, selected: selected)))
    }

    static func select(_ id: String?) -> GraphPageCall {
        struct Select: Encodable { let id: String? }
        return GraphPageCall(function: .select, payload: encode(Select(id: id)))
    }

    static func encode<T: Encodable>(_ value: T) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        // Кодирование этих типов не падает: строки, числа и логические значения.
        return (try? encoder.encode(value)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }
}
