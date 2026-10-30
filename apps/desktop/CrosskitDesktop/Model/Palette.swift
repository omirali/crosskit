import CrosskitKit
import SwiftUI

/// Цвета состояний и уровней. Системные цвета сами меняются со светлой и тёмной темой.
enum Palette {

    static func color(_ state: BranchState) -> Color {
        switch state {
        case .chosen: .green
        case .agentDefault: .orange
        case .open: .accentColor
        case .rejected, .deferred, .abandoned: .secondary
        }
    }

    static func color(_ level: RiskLevel) -> Color {
        switch level {
        case .critical: .red
        case .high: .orange
        case .medium: .yellow
        case .info: .gray
        }
    }

    static func color(_ platform: Platform?) -> Color {
        platform == .codex ? Color(red: 0.29, green: 0.30, blue: 0.33) : Color(red: 0.77, green: 0.42, blue: 0.27)
    }

    /// Точка задачи в списке: где она стоит.
    static func dot(_ task: ConfirmedTask) -> Color {
        if task.finishedAtUnixMs != nil { return .secondary.opacity(0.5) }
        if task.progress?.openCritical ?? 0 > 0 { return .red }
        switch task.column {
        case .waiting: return .accentColor
        case .inWork: return .green
        case .unmapped, .done: return .secondary.opacity(0.5)
        }
    }

    static func dotMeaning(_ task: ConfirmedTask) -> String {
        if task.finishedAtUnixMs != nil { return "готово" }
        let critical = task.progress?.openCritical ?? 0 > 0 ? ", открыт критичный риск" : ""
        switch task.column {
        case .waiting: return "ждёт вашего решения" + critical
        case .inWork: return "в работе" + critical
        case .unmapped: return "карта не построена"
        case .done: return "готово"
        }
    }
}
