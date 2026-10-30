import Foundation

/// Что карта знаний пишет словами. Только строки — ни одного вида, поэтому их видят тесты.
enum KnowledgeWording {

    /// «312 сущностей · 540 связей» — и «показано 800 из 1 203», когда карта срезана.
    static func counts(_ map: KnowledgeAnswer.Map) -> String {
        var parts = ["\(map.counts.entities.shown) \(entitiesWord(map.counts.entities.shown))",
                     "\(map.counts.facts.shown) \(factsWord(map.counts.facts.shown))"]
        if map.truncated {
            parts.append("показано \(map.counts.entities.shown) из \(map.counts.entities.found) — самые связанные")
        }
        return parts.joined(separator: " · ")
    }

    /// Строка над картой: что на ней, или что нашёл набранный текст.
    static func explanation(_ map: KnowledgeAnswer.Map, query: String, found: KnowledgePicture.Found) -> String {
        let wanted = query.trimmingCharacters(in: .whitespaces)
        guard wanted.count >= 2 else {
            return "Цвет — тип, размер — число связей. Наберите имя — подсветятся сущность и соседи; клик по связи — цитата и источник."
        }
        guard !found.hits.isEmpty else {
            var line = "«\(wanted)» — на карте нет"
            if !found.closest.isEmpty { line += "; ближе всего: " + found.closest.joined(separator: ", ") }
            if map.truncated {
                line += ". Карта держит \(map.counts.entities.shown) самых связанных; остальные — `crosskit knowledge show \(wanted)`"
            }
            return line + "."
        }
        let names = found.hits.prefix(listed).compactMap { map.entity($0)?.name }
        let how = found.bySound ? " по созвучию" : ""
        var line = found.hits.count == 1 ? "Найдено\(how): \(names[0])"
            : "Найдено\(how) \(found.hits.count): " + names.joined(separator: ", ")
        if found.hits.count > listed { line += " и ещё \(found.hits.count - listed)" }
        line += " · \(found.near.count) \(neighboursWord(found.near.count))"
        return line + (found.hits.count > 1 ? ". ↑↓ — по найденным." : ".")
    }

    /// Сколько найденных имён строка называет поимённо.
    static let listed = 5

    /// Что сказать о пустой карте: почему пусто и что сделать.
    static let empty = "В этом контексте ещё нет фактов. Их достаёт разбор очереди — `crosskit index jobs run --limit <N>` в контексте, которому разрешено отправлять содержимое."

    /// Карточка: заголовок, до трёх строк под ним и действие.
    struct Card: Equatable {
        let type: String
        let title: String
        let lines: [String]
        let key: String?
        let action: String?
    }

    static func card(of entity: KnowledgeAnswer.Entity) -> Card {
        var lines: [String] = []
        if !entity.description.isEmpty { lines.append(entity.description) }
        if entity.spellings.count > 1 {
            lines.append("также: " + entity.spellings.dropFirst().joined(separator: ", "))
        }
        lines.append("\(entity.degree) \(factsWord(entity.degree)) · \(entity.sources) \(sourcesWord(entity.sources))")
        return Card(type: entity.type, title: entity.name, lines: lines, key: nil, action: nil)
    }

    /// «Пакет —belongs-to→ Контрагент», цитата в ёлочках и где её нашли.
    static func card(of fact: KnowledgeAnswer.Fact, in map: KnowledgeAnswer.Map) -> Card {
        let subject = map.entity(fact.from)?.name ?? "?"
        let object = map.entity(fact.to)?.name ?? "?"
        var lines = ["«\(fact.quote)»"]
        if !fact.description.isEmpty { lines.append(fact.description) }
        lines.append(source(fact.source))
        switch fact.source.kind {
        case .file:
            return Card(type: "связь", title: "\(subject) —\(fact.predicate)→ \(object)", lines: lines,
                        key: "⏎", action: "Открыть файл")
        case .conversation:
            return Card(type: "связь", title: "\(subject) —\(fact.predicate)→ \(object)", lines: lines,
                        key: "⏎", action: "Скопировать claude --resume")
        }
    }

    /// «беседа «Разбор реестра» · реплика 12» или «~/Projects/erp/registry.md · часть 3».
    static func source(_ source: KnowledgeAnswer.Source) -> String {
        switch source.kind {
        case .file:
            return "\(GraphWording.shortened(source.path)) · часть \(source.part)"
        case .conversation:
            let name = (source.conversation ?? "").isEmpty ? "без названия" : "«\(source.conversation ?? "")»"
            let at = source.reply.map { "реплика \($0)" } ?? "часть \(source.part)"
            return "беседа \(name) · \(at)"
        }
    }

    static func entitiesWord(_ count: Int) -> String {
        GraphWording.plural(count, one: "сущность", few: "сущности", many: "сущностей")
    }

    static func factsWord(_ count: Int) -> String {
        GraphWording.plural(count, one: "связь", few: "связи", many: "связей")
    }

    static func sourcesWord(_ count: Int) -> String {
        GraphWording.plural(count, one: "источник", few: "источника", many: "источников")
    }

    static func neighboursWord(_ count: Int) -> String {
        GraphWording.plural(count, one: "сосед", few: "соседа", many: "соседей")
    }
}
