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
}
