import CrosskitKit
import Foundation

/// Слова окна: состояния, уровни, отказы службы. Окно говорит по-русски, как и панель.
enum Wording {

    static let locale = Locale(identifier: "ru_RU")

    static func mark(_ state: BranchState) -> String {
        switch state {
        case .chosen: "✓"
        case .rejected: "✗"
        case .deferred: "○"
        case .open: "?"
        case .agentDefault: "ᵃ"
        case .abandoned: "⊘"
        }
    }

    static func label(_ state: BranchState) -> String {
        switch state {
        case .chosen: "выбрал владелец"
        case .rejected: "отклонено"
        case .deferred: "отложено"
        case .open: "ждёт решения"
        case .agentDefault: "решил агент"
        case .abandoned: "брошено"
        }
    }

    static func label(_ status: RiskStatus) -> String {
        switch status {
        case .open: "открыт"
        case .accepted: "принят"
        case .resolved: "закрыт"
        }
    }

    static func label(_ mode: MapMode) -> String {
        switch mode {
        case .line: "Основная линия"
        case .conversations: "Беседы"
        case .risks: "Риски"
        }
    }

    /// Почему книга задач не приняла решение — по виду отказа, который служба отдаёт данными.
    static func sentence(_ declined: Declined) -> String {
        let task = declined.task.map { "«\($0)»" } ?? "задача"
        switch declined.kind {
        case "open-critical-risks":
            return "У \(task) ещё \(count(declined.count ?? 0, "открытый критичный риск", "открытых критичных риска", "открытых критичных рисков")). Задачу с открытым critical нельзя отметить готовой: сначала примите или закройте его с причиной."
        case "empty-reason":
            return "Решение о риске хранит причину: напишите, почему."
        case "no-risk-decision":
            return "О риске в реплике \(declined.reply ?? 0) вы ещё ничего не решали — возвращать нечего."
        case "name-taken":
            return "Задача «\(declined.name ?? "")» уже есть. Выберите другое имя."
        case "empty-name":
            return "У задачи должно быть имя."
        case "no-such-task":
            return "Задачи \(task) больше нет. Обновите список (⌘R)."
        case "in-another-task":
            return "Беседа \(declined.session ?? "") уже в \(task): сначала уберите её оттуда."
        case "not-in-task":
            return "Беседы \(declined.session ?? "") нет в \(task)."
        case "unknown-conversation", "ambiguous-conversation":
            return "Беседа \(declined.reference ?? "") не найдена однозначно. Обновите список (⌘R)."
        case "no-conversations":
            return "В задаче должна остаться хотя бы одна беседа."
        default:
            return "Книга задач отказала: \(declined.kind)."
        }
    }

    static func title(_ column: TaskColumn) -> String {
        switch column {
        case .unmapped: "Не размечены"
        case .inWork: "В работе"
        case .waiting: "Ждут решения"
        case .done: "Готово"
        }
    }

    /// Слово владельца о риске, коротко: «принят вами 30 окт».
    static func decided(_ decided: Decided) -> String {
        let verb = decided.verdict == .resolved ? "закрыт" : "принят"
        return "\(verb) вами \(day(decided.at))"
    }

    static func speaker(owner: Bool) -> String {
        owner ? "Владелец" : "Агент"
    }

    static func day(_ date: Date?) -> String {
        guard let date else { return "без даты" }
        return date.formatted(Date.FormatStyle(locale: locale).day().month(.abbreviated))
    }

    static func tool(_ platform: Platform?) -> String {
        platform == .codex ? "Codex" : "Claude Code"
    }

    /// Команда, которой беседу продолжают в её инструменте. Окно не управляет чужим
    /// приложением, а отдаёт команду в буфер обмена, как панель.
    static func resume(session: String, platform: Platform?) -> String {
        platform == .codex ? "codex resume \(session)" : "claude --resume \(session)"
    }

    static func count(_ number: Int, _ one: String, _ few: String, _ many: String) -> String {
        let tens = number % 100
        let units = number % 10
        let word = (11...14).contains(tens) ? many : units == 1 ? one : (2...4).contains(units) ? few : many
        return "\(number) \(word)"
    }

    /// Что сказать человеку, когда служба не дала ответа. `state` — что служба сама пишет о
    /// контексте, который не отдаёт.
    static func failure(_ error: ServiceError, state: ContextState? = nil) -> (title: String, explanation: String) {
        switch error {
        case .refused("not-served") where state?.phase == "starting":
            ("Служба поднимает контекст «\(state?.name ?? "")»",
             "Идёт первый проход: граф дописывается из файлов на диске. Окно спросит снова само."
                + (state?.lastPass.map { "\nПоследний проход: \($0.text)" } ?? ""))
        case .refused("not-served") where state?.phase == "waiting":
            ("Служба не может открыть контекст «\(state?.name ?? "")»",
             (state?.waiting ?? "Причину служба не назвала.") + "\nОкно спросит снова само.")
        case .notRunning:
            ("Служба не запущена",
             "Окно читает задачи у фоновой службы Crosskit. Запустите её: crosskit service install, если её ещё нет, или crosskit service restart.")
        case .refused("not-served"):
            ("Служба ещё не держит этот контекст",
             "Она запускается или ждёт, пока другой процесс отпустит граф. Обновите через несколько секунд (⌘R).")
        case .refused("protocol-mismatch"):
            ("Служба другой версии",
             "Окно и служба говорят на разных версиях протокола. Пересоберите ядро и выполните crosskit service restart.")
        case .refused("overloaded"):
            ("Служба занята", "Она отвечает на столько вопросов сразу, сколько берёт. Обновите позже (⌘R).")
        case .refused(let reason):
            ("Служба отказала", "Причина: \(reason).")
        case .commandRefused(let category):
            ("Служба не смогла ответить", "Категория отказа: \(category). Тот же вопрос из терминала скажет больше: crosskit tasks map <задача>.")
        case .noSuchTask(let task):
            ("Задачи «\(task)» нет", "В книге задач этого контекста её больше нет. Обновите список (⌘R).")
        case .declined(let declined):
            ("Решение не принято", sentence(declined))
        case .timedOut:
            ("Служба отвечает слишком долго",
             "Обычно так бывает сразу после её перезапуска, пока она поднимает граф. Обновите через минуту (⌘R).")
        case .unreadable:
            ("Ответ не прочитан",
             "Служба ответила строкой, которую окно не понимает. Вероятно, служба старее окна: crosskit service restart.")
        }
    }
}
