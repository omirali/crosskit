import AppKit
import CrosskitKit
import Foundation
import Observation
import UserNotifications

/// Центр уведомлений (макет `design/UXMain.dc.html`): всё, что ждёт человека во всех задачах
/// контекста, одним списком — и из него же решается.
///
/// Списка у службы нет: окно собирает его из того, что знает, — запросов разрешения живых
/// бесед, задач, где последняя развилка ждёт решения, открытых критичных рисков, карт, которые
/// отстают от бесед, и кандидатов сшивки. Поэтому у пункта нет своего времени: время — у беседы.
struct InboxItem: Identifiable {
    enum Kind: CaseIterable {
        case permission, decision, risk, turn, map, candidate

        /// Нужен ли тут человек, а не просто сведения.
        var needsYou: Bool { self == .permission || self == .decision || self == .risk }
    }

    enum Group: String, CaseIterable {
        case now = "СЕЙЧАС"
        case waiting = "ЖДУТ ВАС"
        case map = "КАРТА И ПРЕДЛОЖЕНИЯ"
    }

    struct Action: Identifiable {
        let id = UUID()
        let label: String
        var prominent = false
        let perform: @MainActor () -> Void
    }

    let id: String
    let kind: Kind
    let group: Group
    let title: String
    let text: String
    let place: String
    let time: String
    let open: @MainActor () -> Void
    let actions: [Action]
}

enum InboxTab: String, CaseIterable, Identifiable {
    case all = "Все", you = "Нужны вы", agents = "Агенты", map = "Карта и риски"
    var id: Self { self }

    func holds(_ kind: InboxItem.Kind) -> Bool {
        switch self {
        case .all: true
        case .you: kind.needsYou
        case .agents: kind == .permission || kind == .turn
        case .map: kind == .map || kind == .risk || kind == .candidate
        }
    }
}

/// Что из уведомлений прочитано и что человек разрешил окну говорить системе.
@Observable
final class InboxState {
    var isOpen = false
    var showsSettings = false
    var tab: InboxTab = .all
    private(set) var read: Set<String> = []

    /// Какие события идут в системные уведомления macOS, когда окно не в фокусе.
    var systemPermission = UserDefaults.standard.object(forKey: "inbox.permission") as? Bool ?? true {
        didSet { UserDefaults.standard.set(systemPermission, forKey: "inbox.permission") }
    }
    var systemTurn = UserDefaults.standard.object(forKey: "inbox.turn") as? Bool ?? false {
        didSet { UserDefaults.standard.set(systemTurn, forKey: "inbox.turn") }
    }
    /// «Не беспокоить»: молчит всё, кроме запросов разрешения — агент без ответа стоит.
    var quiet = false

    func markRead(_ id: String) { read.insert(id) }
    func markRead(_ items: [InboxItem]) { read.formUnion(items.map(\.id)) }
    func isRead(_ id: String) -> Bool { read.contains(id) }

    /// Говорит системе о беседе, которой нужен человек, если окно сейчас не перед ним.
    func announce(_ attention: ChatAttention, in chat: ChatModel, place: String) {
        guard !NSApp.isActive else { return }
        let (title, body): (String, String)
        switch attention {
        case .permission(let request):
            guard systemPermission else { return }
            let activity = ToolActivity(tool: request.tool, input: request.input, directory: chat.target.directory)
            title = "Агент ждёт разрешения"
            body = "\(activity.verb) \(activity.subject)"
        case .turnFinished(let text):
            guard systemTurn, !quiet else { return }
            title = "Claude Code закончил ход"
            body = text.map { String($0.prefix(140)) } ?? ""
        }
        let content = UNMutableNotificationContent()
        content.title = title
        content.subtitle = place
        content.body = body
        let center = UNUserNotificationCenter.current()
        Task {
            guard (try? await center.requestAuthorization(options: [.alert, .sound])) == true else { return }
            try? await center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }
}

extension DesktopModel {

    /// Уведомления контекста: сначала то, что стоит сейчас, потом решения, потом карта.
    var inboxItems: [InboxItem] {
        permissionItems + turnItems + taskItems + candidateItems
    }

    private var permissionItems: [InboxItem] {
        live.all.flatMap { key, chat in
            chat.pendingPermissions.map { request in
                let activity = ToolActivity(tool: request.tool, input: request.input, directory: chat.target.directory)
                return InboxItem(
                    id: "permission-\(request.id)", kind: .permission, group: .now,
                    title: "Агент ждёт разрешения", text: "\(activity.verb) \(activity.subject)",
                    place: place(of: chat), time: "сейчас",
                    open: { [weak self] in self?.openChat(key, of: chat) },
                    actions: [
                        .init(label: "Разрешить", prominent: true) { chat.answer(request, .allowed) },
                        .init(label: "Отклонить") { chat.answer(request, .denied) },
                        .init(label: "Показать") { [weak self] in self?.openChat(key, of: chat) },
                    ])
            }
        }
    }

    private var turnItems: [InboxItem] {
        live.all.compactMap { key, chat in
            guard chat.turns > 0, chat.state == .waiting, chat.pendingPermissions.isEmpty else { return nil }
            return InboxItem(
                id: "turn-\(key)-\(chat.turns)", kind: .turn, group: .now,
                title: "Claude Code закончил ход", text: chat.lastText.map { "«\($0.prefix(160))»" } ?? "",
                place: place(of: chat) + (chat.costUSD > 0 ? " · " + Wording.usd(chat.costUSD) : ""), time: "сейчас",
                open: { [weak self] in self?.openChat(key, of: chat) },
                actions: [.init(label: "К чату") { [weak self] in self?.openChat(key, of: chat) }])
        }
    }

    private var taskItems: [InboxItem] {
        guard let board = board.value else { return [] }
        return board.confirmed.filter { $0.finishedAtUnixMs == nil }.flatMap { task -> [InboxItem] in
            var items: [InboxItem] = []
            let place = [context, projects.first { $0.taskIDs.contains(task.id) }?.name, task.name]
                .compactMap { $0 }.joined(separator: " · ")
            let day = Wording.day(task.conversations.compactMap(\.lastAtUnixMs).max().map(Date.init(unixMilliseconds:)))
            let tree = tree(of: task.id)
            if task.column == .waiting {
                let waiting = tree?.waitingStep
                let choose = (waiting?.branches.filter { $0.state == .open } ?? []).prefix(2).map { branch in
                    InboxItem.Action(label: branch.option) { [weak self] in self?.choose(branch, in: task.id) }
                }
                items.append(InboxItem(
                    id: "decision-\(task.id)-\(task.progress?.steps ?? 0)", kind: .decision, group: .waiting,
                    title: "Ждёт вашего решения",
                    text: waiting.map { "«\($0.question)»" } ?? "Последняя развилка задачи без выбора",
                    place: place + (waiting.map { " · шаг \($0.id + 1)" } ?? ""), time: day,
                    open: { [weak self] in self?.openStep(waiting?.id, of: task.id) },
                    actions: choose + [.init(label: "На карте") { [weak self] in self?.openStep(waiting?.id, of: task.id) }]))
            }
            if let critical = task.progress?.openCritical, critical > 0 {
                let risk = tree?.openRisks.first { $0.level == .critical }
                items.append(InboxItem(
                    id: "risk-\(task.id)-\(critical)", kind: .risk, group: .waiting,
                    title: critical == 1 ? "Критичный риск открыт" : "Открытых критичных рисков: \(critical)",
                    text: (risk.map { "«\($0.title)» — " } ?? "") + "задачу нельзя отметить готовой",
                    place: place, time: day,
                    open: { [weak self] in self?.openRisk(risk?.id, of: task.id) },
                    actions: [.init(label: "Открыть риск") { [weak self] in self?.openRisk(risk?.id, of: task.id) }]))
            }
            if let progress = task.progress, progress.mapped > 0, progress.mapped < progress.conversations {
                let behind = progress.conversations - progress.mapped
                items.append(InboxItem(
                    id: "map-\(task.id)-\(progress.mapped)-\(progress.conversations)", kind: .map, group: .map,
                    title: "Карта отстаёт от бесед",
                    text: "\(Wording.count(behind, "беседа ещё не прочитана", "беседы ещё не прочитаны", "бесед ещё не прочитаны")) Crosskit",
                    place: place, time: day,
                    open: { [weak self] in self?.open(.task(task.id)) },
                    actions: [.init(label: "Открыть карту") { [weak self] in
                        self?.open(.task(task.id))
                        self?.pane = .map
                    }]))
            }
            return items
        }
    }

    private var candidateItems: [InboxItem] {
        guard let board = board.value else { return [] }
        return board.candidates.compactMap { candidate in
            guard let first = candidate.conversations.first else { return nil }
            let project = projects.first { $0.candidateSessions.contains(first.session) }?.name
            return InboxItem(
                id: "candidate-\(first.session)", kind: .candidate, group: .map,
                title: "Crosskit предлагает задачу",
                text: "«\(first.title)» — \(Wording.count(candidate.conversations.count, "беседа", "беседы", "бесед"))"
                    + (Wording.why(candidate).map { ", \($0)" } ?? ""),
                place: [context, project].compactMap { $0 }.joined(separator: " · "),
                time: Wording.day(first.firstAt),
                open: { [weak self] in self?.pane = .board },
                actions: [
                    .init(label: "Сделать задачей") { [weak self] in self?.confirmCandidate(candidate) },
                    .init(label: "Не задача") { [weak self] in
                        Task { await self?.decide(.reject(conversation: first.session)) }
                    },
                ])
        }
    }

    // MARK: Переходы

    func openChat(_ key: String, of chat: ChatModel) {
        if let taskID = chat.target.taskID { open(.task(taskID)) }
        chatKey = key
        target = nil
        inbox.isOpen = false
    }

    func openStep(_ step: Int?, of taskID: String) {
        open(.task(taskID))
        pane = .map
        inbox.isOpen = false
        if let step { select(.step(step), in: taskID) }
    }

    func openRisk(_ risk: String?, of taskID: String) {
        open(.task(taskID))
        pane = .map
        inbox.isOpen = false
        if let risk { select(.risk(risk), in: taskID) }
    }

    /// Выбор на развилке говорится агенту, а не пишется в карту: карта — это доказанное
    /// цитатами, и выбор станет её частью, когда его прочитают в беседе. Окно открывает беседу,
    /// где вопрос задан, и кладёт слово в строку ввода — отправляет человек.
    func choose(_ branch: DecisionTree.Branch, in taskID: String) {
        open(.task(taskID))
        inbox.isOpen = false
        show(branch.quote)
        composer.quickOn = false
        composer.startsNew = false
        composer.draft = "Выбираю: \(branch.option)."
    }

    func confirmCandidate(_ candidate: CandidateTask) {
        guard let first = candidate.conversations.first else { return }
        nameRequest = NameRequest(title: "Сделать задачей", initial: first.name ?? first.opening ?? "") {
            .confirm(conversation: first.session, name: $0)
        }
    }
}
