import Foundation

/// Что экран графа пишет словами. Только строки — ни одного вида, поэтому их видят тесты.
enum GraphWording {

    /// Полное имя узла — в карточке; на холсте оно сокращено (`GraphPicture.short`).
    static func title(of node: GraphAnswer.Node) -> String {
        switch node.kind {
        case .conversation:
            if !node.label.isEmpty { return node.label }
            return "Без названия · " + (node.conversation?.sessionId ?? "")
        case .reply:
            return "\(node.label) · \(speaker(node.reply?.speaker))"
        case .file:
            return node.file.map { shortened($0.path) } ?? node.label
        case .project:
            return node.label
        case .topic:
            return node.label
        }
    }

    /// Строка над графом: что на нём, словами.
    static func explanation(_ view: GraphAnswer.View) -> String {
        let focus = view.node(view.focus)
        let text = focus?.kind == .file
            ? "Файл и беседы, которые его меняли или где вы его называли. Правее — другие файлы этих бесед."
            : "Беседа и файлы, которые она меняла или вы называли. Правее — другие беседы тех же файлов."
        // Вид выше холста — за краем дальний шаг, и об этом надо сказать вместе с тем, чем его достать.
        let drawn = view.nodes.filter { $0.kind != .reply }.count
        return drawn > tallerThanCanvas ? text + " Стрелки и два пальца двигают, 0 — весь граф." : text
    }

    /// Со скольких узлов вид выше холста.
    static let tallerThanCanvas = 14

    /// Что сказать, когда шаг ничего не нашёл: пустой второй шаг называется, а не прячется.
    static func emptiness(_ view: GraphAnswer.View) -> String? {
        guard let focus = view.node(view.focus) else { return nil }
        let near: GraphAnswer.Kind = focus.kind == .conversation ? .file : .conversation
        let far: GraphAnswer.Kind = focus.kind == .conversation ? .conversation : .file
        let firstStep = view.nodes.contains { $0.kind == near && $0.depth == 1 }
        if !firstStep {
            let passing = view.notShown.inPassing
            let tail = passing > 0 ? " Прочитанных мимоходом — \(passing), они свёрнуты." : ""
            return focus.kind == .conversation
                ? "Эта беседа не правила файлов и не называла их." + tail
                : "Ни одна беседа этого контекста не правила этот файл и не называла его." + tail
        }
        guard view.depth == 2, !view.nodes.contains(where: { $0.kind == far && $0.depth == 2 }) else { return nil }
        if view.counts[far].found > view.counts[far].shown { return nil }
        return focus.kind == .conversation
            ? "У этих файлов других бесед нет."
            : "У этих бесед других файлов нет."
    }

    /// Карточка выбранного узла: вид, заголовок, строки под ним и действие.
    struct Card: Equatable {
        let type: String
        let title: String
        let detail: String
        let key: String?
        let action: String?
    }

    static func card(of node: GraphAnswer.Node, in view: GraphAnswer.View, now: Date) -> Card {
        let focusHint = node.focusable && node.id != view.focus ? " · клик — фокус" : ""
        switch node.kind {
        case .conversation:
            let facts = node.conversation
            var detail = ["сессия \(facts?.sessionId ?? "—")"]
            if let facts { detail.append("\(facts.messageCount) \(messagesWord(facts.messageCount))") }
            if let last = facts?.lastMessageAtUnixMs {
                detail.append("последнее " + CoreAnswer.relative(unixMs: last, now: now))
            }
            return Card(type: "Беседа", title: title(of: node), detail: detail.joined(separator: " · ") + focusHint,
                        key: "⏎", action: "Скопировать claude --resume")
        case .file:
            let state = node.file?.state
            let type: String
            switch state {
            case .missing?: type = "Файл · нет на диске"
            case .outside?: type = "Файл · вне контекста"
            default: type = "Файл"
            }
            var detail: [String] = []
            switch state {
            case .missing?: detail.append("есть в графе, нет на диске")
            case .outside?: detail.append("в контексте \(view.context) этого пути нет — перехода нет")
            default: break
            }
            detail += touches(into: node.id, in: view, now: now)
            let opens = state == .present
            return Card(type: type, title: title(of: node), detail: detail.joined(separator: " · ") + focusHint,
                        key: opens ? "⏎" : nil, action: opens ? "Открыть" : nil)
        case .reply:
            return Card(type: "Сообщение", title: title(of: node), detail: "", key: nil, action: nil)
        case .project:
            return Card(type: "Проект", title: title(of: node),
                        detail: node.project.map { shortened($0.root) + " · корень контекста \(view.context)" } ?? "",
                        key: nil, action: nil)
        case .topic:
            return Card(type: "Тема", title: title(of: node), detail: "метка модели", key: nil, action: nil)
        }
    }

    /// Что с файлом делали беседы вида, и у какой реплики — с пометкой, что реплика найдена по
    /// времени, а не записана: так её и надо читать.
    static func touches(into id: String, in view: GraphAnswer.View, now: Date) -> [String] {
        view.edges.filter { $0.to == id && $0.touch != nil }.compactMap { edge in
            guard let touch = edge.touch else { return nil }
            let reply = view.node(edge.from)?.reply
            let conversation = reply.flatMap { view.node($0.conversation) } ?? view.node(edge.from)
            var line = GraphPicture.touchLabel([touch])
            if let conversation, conversation.id != view.focus {
                line = "«\(GraphPicture.short(title(of: conversation)))»: " + line
            }
            if let reply {
                var at = "реплика #\(reply.ordinal)"
                if let moment = reply.atUnixMs { at += ", " + CoreAnswer.relative(unixMs: moment, now: now) }
                line += touch.byTime ? " (\(at) — определена по времени)" : " (\(at))"
            }
            return line
        }
    }

    /// «обрезано · показано 40 из 74» — или ничего, когда вид держит всё, что нашёл. Реплик на
    /// холсте нет, поэтому и в счёте их нет: они — в карточке.
    static func truncation(_ view: GraphAnswer.View) -> String? {
        let kinds: [GraphAnswer.Kind] = [.conversation, .file, .project, .topic]
        let shown = kinds.reduce(0) { $0 + view.counts[$1].shown }
        let found = kinds.reduce(0) { $0 + view.counts[$1].found }
        guard found > shown else { return nil }
        return "обрезано · показано \(shown) из \(found)"
    }

    /// Счёт по каждому виду узла: найдено и показано. Вид, которого нет вовсе, не называется.
    static func counts(_ view: GraphAnswer.View) -> String {
        let names: [(GraphAnswer.Kind, String)] = [
            (.conversation, "Беседы"), (.file, "Файлы"), (.project, "Проекты"), (.topic, "Темы")]
        var parts = names.compactMap { kind, name -> String? in
            let count = view.counts[kind]
            guard count.found > 0 else { return nil }
            return count.shown == count.found ? "\(name) \(count.found)" : "\(name) \(count.shown) из \(count.found)"
        }
        let outside = view.nodes.filter { $0.file?.state == .outside }.count
        if outside > 0 { parts.append("из файлов вне контекста — \(outside)") }
        return parts.joined(separator: " · ")
    }

    static func speaker(_ value: String?) -> String {
        value == "owner" ? "вы" : "модель"
    }

    static func messagesWord(_ count: Int) -> String {
        plural(count, one: "сообщение", few: "сообщения", many: "сообщений")
    }

    /// Форма слова при числе: 1 сообщение, 3 сообщения, 11 сообщений. Одна на панель.
    static func plural(_ count: Int, one: String, few: String, many: String) -> String {
        let tens = count % 100, ones = count % 10
        if (11...14).contains(tens) { return many }
        switch ones {
        case 1: return one
        case 2...4: return few
        default: return many
        }
    }

    /// Домашний каталог — `~`, как в выдаче.
    static func shortened(_ path: String) -> String {
        let (directory, name) = CoreAnswer.split(path, home: NSHomeDirectory())
        return directory + name
    }
}
