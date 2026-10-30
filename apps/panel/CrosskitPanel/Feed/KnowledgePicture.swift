import Foundation
import CoreGraphics

/// Что страница карты знаний рисует: карта ядра, переведённая в элементы Cytoscape, и то, что
/// подсвечивает поиск.
///
/// Как и у графа окрестности (`GraphPicture`), решения — здесь, без WebKit, и их видят тесты:
///
/// * **цвет — тип**: номер типа в словаре конфигурации, а не в этой карте, — иначе один тип
///   менял бы цвет от контекста к контексту;
/// * **размер — число связей**, по корню: у реестра в сорок связей узел больше, но не застилает
///   карту;
/// * **поиск** находит сущность по любому её написанию и подсвечивает её соседей — соседей
///   считает панель, страница только гасит остальное.
struct KnowledgePicture: Encodable, Equatable {

    struct Node: Encodable, Equatable {
        let id: String
        let label: String
        let type: String
        /// Цвет типа строкой CSS.
        let color: String
        let size: Double
        let degree: Int
        /// Имя видно на любом приближении: одна из самых связанных сущностей карты.
        let pinned: Bool
    }

    struct Edge: Encodable, Equatable {
        let id: String
        let source: String
        let target: String
        let label: String
        let weight: Int
    }

    let nodes: [Node]
    let edges: [Edge]

    /// Самая длинная подпись на холсте; целиком — в карточке.
    static let labelLimit = 24

    /// Сколько самых связанных сущностей подписаны всегда. Остальные имена появляются, когда
    /// карту приближают и шрифт становится читаемым.
    static let pinnedNames = 20

    /// Что показать: какие типы скрыты и из какой папки источники.
    struct Filter: Equatable {
        var hiddenTypes: Set<String> = []
        /// Папка источников; `nil` — все источники.
        var folder: String?

        static let everything = Filter()

        var isEverything: Bool { hiddenTypes.isEmpty && folder == nil }
    }

    /// Карта, сведённая фильтром: связи из источников папки, между сущностями нескрытых типов;
    /// сущность — когда у неё осталась связь. Решено здесь, а не на странице, чтобы раскладка
    /// разложила то, что видно, а не оставила дыры на месте скрытого.
    static func filtered(_ map: KnowledgeAnswer.Map, by filter: Filter) -> KnowledgeAnswer.Map {
        guard !filter.isEverything else { return map }
        let shownType = Dictionary(uniqueKeysWithValues: map.entities.map {
            ($0.id, !filter.hiddenTypes.contains($0.type))
        })
        let facts = map.facts.filter { fact in
            guard shownType[fact.from] == true, shownType[fact.to] == true else { return false }
            guard let folder = filter.folder else { return true }
            return fact.source.path.hasPrefix(folder.hasSuffix("/") ? folder : folder + "/")
        }
        let kept = Set(facts.flatMap { [$0.from, $0.to] })
        return KnowledgeAnswer.Map(
            context: map.context, limit: map.limit, truncated: map.truncated, counts: map.counts,
            entityTypes: map.entityTypes, relationTypes: map.relationTypes, roots: map.roots,
            entities: map.entities.filter { kept.contains($0.id) }, facts: facts)
    }

    static func of(_ map: KnowledgeAnswer.Map, palette: [String]) -> KnowledgePicture {
        let pinned = Set(map.entities
            .sorted { ($0.degree, $1.name) > ($1.degree, $0.name) }
            .prefix(pinnedNames)
            .map(\.id))
        let nodes = map.entities.map { entity in
            Node(id: entity.id, label: GraphPicture.short(entity.name, limit: labelLimit), type: entity.type,
                 color: color(of: entity.type, in: map.entityTypes, palette: palette),
                 size: size(degree: entity.degree), degree: entity.degree, pinned: pinned.contains(entity.id))
        }
        let edges = map.facts.map { fact in
            Edge(id: fact.id, source: fact.from, target: fact.to, label: fact.predicate, weight: fact.weight)
        }
        return KnowledgePicture(nodes: nodes, edges: edges)
    }

    /// Цвет типа: его место в словаре; тип, которого словарь не знает, — последним цветом.
    static func color(of type: String, in types: [String], palette: [String]) -> String {
        guard !palette.isEmpty else { return "gray" }
        guard let at = types.firstIndex(of: type) else { return palette[palette.count - 1] }
        return palette[at % palette.count]
    }

    /// Диаметр узла в точках: 12 у одинокой сущности, 46 — потолок.
    static func size(degree: Int) -> Double {
        min(46, 12 + 5 * Double(max(degree, 0)).squareRoot())
    }

    /// Папки, по которым карту можно сузить: каждый корень контекста, где есть источники
    /// фактов, и папки первого уровня под ним. С числом фактов, по убыванию.
    static func folders(of map: KnowledgeAnswer.Map) -> [(folder: String, facts: Int)] {
        var counts: [String: Int] = [:]
        for fact in map.facts {
            for root in map.roots {
                let prefix = root.hasSuffix("/") ? root : root + "/"
                guard fact.source.path.hasPrefix(prefix) else { continue }
                counts[root, default: 0] += 1
                let rest = fact.source.path.dropFirst(prefix.count)
                if let slash = rest.firstIndex(of: "/") {
                    counts[prefix + rest[..<slash], default: 0] += 1
                }
            }
        }
        return counts.map { (folder: $0.key, facts: $0.value) }
            .sorted { ($0.facts, $1.folder) > ($1.facts, $0.folder) }
    }

    /// Что нашёл набранный текст: найденные сущности, их соседи, и — когда не нашлось ничего —
    /// ближайшие по написанию имена.
    struct Found: Encodable, Equatable {
        let hits: [String]
        let near: [String]
        /// Имена, ближайшие к набранному, когда сущностей не нашлось.
        let closest: [String]
        /// Найдено не по основе, а по созвучию с латинским словом: «контракт» — `contract`.
        var bySound = false

        static let nothing = Found(hits: [], near: [], closest: [])

        private enum CodingKeys: String, CodingKey { case hits, near }
    }

    /// Поиск по основам: слово запроса совпадает с основой имени, написания или описания, если
    /// одна начинается с другой — «контракты» находит основу «контракт», «дог» — «договор». Короче
    /// двух букв слово не ищется; основа короче четырёх букв принимается только целиком. Нужны
    /// все слова запроса. Совпавшие по имени идут первыми, по описанию — следом.
    static func found(_ query: String, in map: KnowledgeAnswer.Map) -> Found {
        let words = query.lowercased().replacingOccurrences(of: "ё", with: "е")
            .split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
        guard let longest = words.map(\.count).max(), longest >= 2 else { return .nothing }
        func matches(_ word: String, _ term: String) -> Bool {
            term.hasPrefix(word) || (word.hasPrefix(term) && term.count >= 4)
        }
        func covers(_ terms: [String]) -> Bool {
            words.allSatisfy { word in terms.contains { matches(word, $0) } }
        }
        var byName: [String] = []
        var byDescription: [String] = []
        for entity in map.entities {
            let nameTerms = entity.spellings.flatMap(wordsOf)
            if covers(nameTerms) {
                byName.append(entity.id)
            } else if covers(entity.terms + nameTerms) {
                byDescription.append(entity.id)
            }
        }
        var hits = byName + byDescription
        var bySound = false
        if hits.isEmpty {
            // Русское слово против латинской основы: имена таблиц и кода пишутся по-английски.
            let sounds = words.map(latin)
            hits = map.entities.filter { entity in
                let terms = entity.terms + entity.spellings.flatMap(wordsOf)
                return sounds.allSatisfy { sound in terms.contains { soundsLike(sound, $0) } }
            }.map(\.id)
            bySound = !hits.isEmpty
        }
        guard !hits.isEmpty else {
            return Found(hits: [], near: [], closest: closest(to: words.joined(separator: " "), in: map))
        }
        let hitSet = Set(hits)
        var near: [String] = []
        for fact in map.facts {
            for (one, other) in [(fact.from, fact.to), (fact.to, fact.from)]
            where hitSet.contains(one) && !hitSet.contains(other) && !near.contains(other) {
                near.append(other)
            }
        }
        return Found(hits: hits, near: near, closest: [], bySound: bySound)
    }

    /// Русское слово латиницей, как его записали бы в коде: «контракты» — `kontrakty`.
    static func latin(_ word: String) -> String {
        let table: [Character: String] = [
            "а": "a", "б": "b", "в": "v", "г": "g", "д": "d", "е": "e", "ж": "zh", "з": "z", "и": "i",
            "й": "y", "к": "k", "л": "l", "м": "m", "н": "n", "о": "o", "п": "p", "р": "r", "с": "s",
            "т": "t", "у": "u", "ф": "f", "х": "kh", "ц": "ts", "ч": "ch", "ш": "sh", "щ": "sch",
            "ъ": "", "ы": "y", "ь": "", "э": "e", "ю": "yu", "я": "ya"]
        return word.map { table[$0] ?? String($0) }.joined()
    }

    /// Созвучны ли слово, записанное латиницей, и латинская основа: начало слова длиной в основу
    /// отличается от неё не больше чем одной правкой, когда `c` читается как `k`. Основы короче
    /// пяти букв не сравниваются — короткому созвучно слишком многое.
    static func soundsLike(_ sound: String, _ term: String) -> Bool {
        guard term.count >= 5, term.allSatisfy({ $0.isASCII }) else { return false }
        let spoken = term.replacingOccurrences(of: "ck", with: "k")
            .replacingOccurrences(of: "c", with: "k")
            .replacingOccurrences(of: "q", with: "k")
            .replacingOccurrences(of: "w", with: "v")
            .replacingOccurrences(of: "x", with: "ks")
        return edits(String(sound.prefix(spoken.count)), spoken) <= 1
    }

    private static func wordsOf(_ text: String) -> [String] {
        text.lowercased().replacingOccurrences(of: "ё", with: "е")
            .split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
    }

    /// Три имени, ближайшие к набранному по числу правок до любого слова имени.
    static func closest(to typed: String, in map: KnowledgeAnswer.Map) -> [String] {
        let ranked = map.entities.map { entity -> (Int, String) in
            let distance = ([entity.name.lowercased()] + wordsOf(entity.name))
                .map { edits(typed, $0) }.min() ?? Int.max
            return (distance, entity.name)
        }
        return ranked.sorted { $0 < $1 }.prefix(3).map(\.1)
    }

    /// Расстояние Левенштейна: сколько вставок, удалений и замен превращают одно в другое.
    static func edits(_ left: String, _ right: String) -> Int {
        let a = Array(left), b = Array(right)
        guard !a.isEmpty else { return b.count }
        guard !b.isEmpty else { return a.count }
        var previous = Array(0...b.count)
        for i in 1...a.count {
            var current = [i] + Array(repeating: 0, count: b.count)
            for j in 1...b.count {
                current[j] = min(previous[j] + 1, current[j - 1] + 1,
                                 previous[j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1))
            }
            previous = current
        }
        return previous[b.count]
    }
}

/// Тема страницы карты: цвета токенов и палитра типов в текущем оформлении, строками CSS.
struct KnowledgeTheme: Encodable, Equatable {
    let fg: String
    let fg2: String
    let edge: String
    let accent: String
    let solid: String
    let palette: [String]
}

extension GraphPageCall {

    static func renderKnowledge(_ picture: KnowledgePicture, theme: KnowledgeTheme, found: KnowledgePicture.Found,
                                generation: Int) -> GraphPageCall {
        struct Render: Encodable {
            let picture: KnowledgePicture
            let theme: KnowledgeTheme
            let found: KnowledgePicture.Found
            let generation: Int
        }
        return GraphPageCall(page: .knowledge, function: .render,
                             payload: encode(Render(picture: picture, theme: theme, found: found,
                                                    generation: generation)))
    }

    static func highlight(_ found: KnowledgePicture.Found) -> GraphPageCall {
        GraphPageCall(page: .knowledge, function: .highlight, payload: encode(found))
    }

    /// Выбор узла или связи на карте; `nil` — снять выбор.
    static func pick(_ id: String?) -> GraphPageCall {
        struct Pick: Encodable { let id: String? }
        return GraphPageCall(page: .knowledge, function: .select, payload: encode(Pick(id: id)))
    }
}
