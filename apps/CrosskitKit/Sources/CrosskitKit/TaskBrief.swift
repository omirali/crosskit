import Foundation

/// Что агент узнаёт о задаче до первого слова новой беседы (ADR 0062): основная линия с тем,
/// что выбрано и что отклонено и почему, где задача стоит сейчас, и открытые риски.
///
/// Это и есть смысл карты решений для агента (`docs/decision-map.md`, раздел 8): агенты раз
/// за разом предлагают уже отвергнутое, а причины отказа в каждом новом ходе это прекращают.
/// Вводная идёт в системную подсказку беседы (`--append-system-prompt`), а не первым словом
/// владельца: это знание о задаче, а не его просьба.
public enum TaskBrief {

    /// Самое длинное, что вводная себе позволяет: она занимает место в контексте каждого хода.
    static let limit = 12_000

    public static func of(_ tree: DecisionTree) -> String {
        var lines = [
            "Ты продолжаешь задачу «\(tree.task)». Ниже — её карта решений, которую Crosskit собрал из прошлых бесед по дословным цитатам: что владелец выбрал, что отклонил и почему, где задача стоит сейчас и какие риски открыты.",
            "Не предлагай отклонённое снова без новой причины. Если меняешь решение, скажи об этом прямо. Решение «выбрал агент» владелец не подтверждал.",
            "",
            "Основная линия:",
        ]
        for step in tree.steps {
            let lead = step.lead.map { " → \(label($0.state)): \($0.option)" + reason($0.reason) } ?? ""
            lines.append("\(step.id + 1). \(step.title)\(lead)")
            for branch in step.branches where branch.id != step.lead?.id && branch.state == .rejected {
                lines.append("   отклонено: \(branch.option)" + reason(branch.reason))
            }
            for branch in step.branches where branch.state == .deferred {
                lines.append("   отложено: \(branch.option)" + reason(branch.reason))
            }
        }
        if let head = tree.head.map({ tree.steps[$0] }), head.lead == nil {
            let options = head.branches.filter { $0.state == .open }.map(\.option)
            lines += ["", "Сейчас задача ждёт решения владельца: «\(head.question)»"
                      + (options.isEmpty ? "" : " Варианты: " + options.joined(separator: "; ") + ".")]
        }
        let open = tree.openRisks.sorted { $0.level < $1.level }.filter { $0.level <= .high }
        if !open.isEmpty {
            lines += ["", "Открытые риски (никто не закрыл и не принял):"]
            lines += open.map { "- \($0.level.rawValue): \($0.title)" + ($0.raisedIn > 1 ? " (поднимался в \($0.raisedIn) беседах)" : "") }
        }
        let decided = tree.risks.filter { $0.decided != nil && !$0.outside }
        if !decided.isEmpty {
            lines += ["", "Риски, о которых владелец уже решил:"]
            lines += decided.compactMap { risk in
                risk.decided.map { "- \($0.verdict == .resolved ? "закрыт" : "принят"): \(risk.title) — \($0.reason)" }
            }
        }
        let text = lines.joined(separator: "\n")
        return text.count <= limit ? text : String(text.prefix(limit)) + "\n…(карта длиннее, остальное в Crosskit)"
    }

    private static func label(_ state: BranchState) -> String {
        state == .agentDefault ? "выбрал агент" : "выбрал владелец"
    }

    private static func reason(_ text: String) -> String {
        text.isEmpty ? "" : " — \(text)"
    }
}
