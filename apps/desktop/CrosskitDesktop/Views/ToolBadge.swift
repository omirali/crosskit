import CrosskitKit
import SwiftUI

/// Значок инструмента, в котором шла беседа: Claude Code или Codex. Цвета — панели
/// (`--t-claude`, `--t-codex`). Знаки — системные символы, а не логотипы: чужие логотипы в
/// открытом репозитории — решение владельца, а не кода.
struct ToolBadge: View {
    let platform: Platform?
    var size: CGFloat = 22

    var body: some View {
        Image(systemName: platform == .codex ? "terminal" : "asterisk")
            .font(.system(size: size * 0.5, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(Palette.color(platform), in: .rect(cornerRadius: size * 0.27))
            .accessibilityLabel(Wording.tool(platform))
    }
}
