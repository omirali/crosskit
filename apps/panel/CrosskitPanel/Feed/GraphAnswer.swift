import Foundation

/// Ответ ядра на `crosskit graph --json`: ограниченная окрестность одного узла.
///
/// Схема объявлена в ядре (`src/presentation/machine_graph.rs`, номер — в `machine_answer.rs`),
/// здесь её зеркало ровно на те поля, которые рисует экран графа. Номер и отказ сверяются тем же
/// конвертом, что у страницы выдачи (`CoreAnswer.checked`): ответ другого номера не читается.
///
/// Здесь нет ни процесса, ни окна — только байты на входе и вид на выходе, поэтому файл
/// проверяется тестами без запуска приложения.
enum GraphAnswer {

    struct View: Decodable, Equatable {
        let context: String
        /// Узел, на котором стоит вид; он же первый в `nodes`.
        let focus: String
        let depth: Int
        let limit: Int
        let nodes: [Node]
        let edges: [Edge]
        let truncated: Bool
        let counts: Counts
        let notShown: NotShown

        func node(_ id: String) -> Node? { nodes.first { $0.id == id } }
    }

    struct Node: Decodable, Equatable {
        let id: String
        let kind: Kind
        /// У беседы без названия — пустая строка; сессия — в `conversation`.
        let label: String
        let depth: Int
        /// Можно ли встать сюда фокусом — тогда `id` и есть следующий `--focus`.
        let focusable: Bool
        let conversation: ConversationFacts?
        let reply: ReplyFacts?
        let file: FileFacts?
        let project: RootFacts?
    }

    enum Kind: String, Decodable, Equatable, CaseIterable {
        case conversation
        case reply
        case file
        case project
        case topic
    }

    struct ConversationFacts: Decodable, Equatable {
        let sessionId: String
        let messageCount: Int
        /// Реплики, которых вид не рисует: не трогавшие файлов и срезанные границей.
        let repliesNotShown: Int
        let lastMessageAtUnixMs: Int64?
    }

    struct ReplyFacts: Decodable, Equatable {
        let conversation: String
        let ordinal: Int
        /// `owner` или `model`.
        let speaker: String
        let atUnixMs: Int64?
    }

    struct FileFacts: Decodable, Equatable {
        /// У файла вне контекста — путь, как его записало касание.
        let path: String
        let state: FileState
    }

    /// `outside` — места с этим путём в контексте нет: файл другого контекста или ничей.
    /// Состояние файла, а не отдельный вид узла (схема 2).
    enum FileState: String, Decodable, Equatable {
        case present
        case missing
        case outside
    }

    struct RootFacts: Decodable, Equatable {
        let root: String
    }

    struct Edge: Decodable, Equatable {
        let from: String
        let to: String
        /// `has-reply`, `touched`, `in-project`, `about`.
        let kind: String
        let touch: Touch?
    }

    struct Touch: Decodable, Equatable {
        let verbs: [String]
        let named: Bool
        let times: Int
        /// Реплика, от которой идёт ребро, найдена чтением по времени, а не записью графа.
        let byTime: Bool
    }

    struct Count: Decodable, Equatable {
        let shown: Int
        let found: Int
    }

    struct Counts: Decodable, Equatable {
        let conversation: Count
        let reply: Count
        let file: Count
        let project: Count
        let topic: Count

        subscript(kind: Kind) -> Count {
            switch kind {
            case .conversation: return conversation
            case .reply: return reply
            case .file: return file
            case .project: return project
            case .topic: return topic
            }
        }

        var shown: Int { Kind.allCases.reduce(0) { $0 + self[$1].shown } }
        var found: Int { Kind.allCases.reduce(0) { $0 + self[$1].found } }
    }

    struct NotShown: Decodable, Equatable {
        let inPassing: Int
        let byPolicy: Int
    }

    /// Вид — или отказ, брошенный как `FeedFailure`.
    static func view(from data: Data) throws -> View {
        let decoder = try CoreAnswer.checked(data)
        return try CoreAnswer.decode(View.self, from: data, with: decoder)
    }

    /// Фокус строки выдачи: у беседы — её сессия, у файла — его путь. Оба поля строка уже
    /// несёт, и ядро понимает обе формы (`conversation:<сессия>`, `path:<путь>`).
    static func focus(of row: FeedRow) -> String? {
        switch row {
        case .conversation(let conversation):
            return conversation.sessionID.map { "conversation:" + $0 }
        case .file(let file):
            return "path:" + file.path
        }
    }
}
