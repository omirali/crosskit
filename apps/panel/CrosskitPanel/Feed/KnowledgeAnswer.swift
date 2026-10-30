import Foundation

/// Ответ ядра на `crosskit knowledge map --json`: все сущности контекста и связи между ними,
/// у каждой связи — цитата и источник.
///
/// Схема объявлена в ядре (`src/presentation/machine_knowledge.rs`, номер — в
/// `machine_answer.rs`), здесь её зеркало на те поля, которые рисует карта. Номер и отказ
/// сверяются тем же конвертом, что у выдачи и графа (`CoreAnswer.checked`).
enum KnowledgeAnswer {

    struct Map: Decodable, Equatable {
        let context: String
        /// Сколько сущностей карта держит самое большее.
        let limit: Int
        /// Найдено больше, чем показано: срез по числу связей.
        let truncated: Bool
        let counts: Counts
        /// Словарь типов в порядке конфигурации — по нему цвет типа одинаков на любой карте.
        let entityTypes: [String]
        let relationTypes: [String]
        /// Корни контекста — из них панель предлагает папки для фильтра.
        let roots: [String]
        let entities: [Entity]
        let facts: [Fact]

        func entity(_ id: String) -> Entity? { entities.first { $0.id == id } }
        func fact(_ id: String) -> Fact? { facts.first { $0.id == id } }
    }

    struct Counts: Decodable, Equatable {
        let entities: GraphAnswer.Count
        let facts: GraphAnswer.Count
    }

    struct Entity: Decodable, Equatable {
        let id: String
        let name: String
        let type: String
        let description: String
        /// Все написания из источников, `name` первым.
        let spellings: [String]
        /// Сколько файлов и бесед её называют.
        let sources: Int
        /// Сколько связей карты к ней сходится.
        let degree: Int
        /// Основы слов имени, написаний и описания — анализатором поиска ядра.
        let terms: [String]
    }

    struct Fact: Decodable, Equatable {
        let id: String
        let from: String
        let to: String
        let predicate: String
        let description: String
        let weight: Int
        /// Слова источника, которые программа в нём нашла.
        let quote: String
        let source: Source
    }

    struct Source: Decodable, Equatable {
        let kind: Kind
        let path: String
        /// Кусок, в котором найдена цитата, считая с единицы.
        let part: Int
        let sessionId: String?
        let conversation: String?
        /// Реплика, считая с единицы, — когда куски беседы совпадают с её репликами.
        let reply: Int?
    }

    enum Kind: String, Decodable, Equatable {
        case file
        case conversation
    }

    /// Карта — или отказ, брошенный как `FeedFailure`.
    static func map(from data: Data) throws -> Map {
        let decoder = try CoreAnswer.checked(data)
        return try CoreAnswer.decode(Map.self, from: data, with: decoder)
    }
}
