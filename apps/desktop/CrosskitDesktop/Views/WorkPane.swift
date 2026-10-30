import CrosskitKit
import SwiftUI

/// Середина окна: карта решений и расширения вокруг задачи — канбан, изменения, файлы, браузер,
/// — и терминал снизу, общий для всех.
struct WorkPane: View {

    @Bindable var model: DesktopModel
    let terminal: TerminalModel
    let browser: BrowserModel

    var body: some View {
        VStack(spacing: 0) {
            tabs
            Divider()
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if terminal.isOpen {
                TerminalPanel(model: model, terminal: terminal)
                    .frame(height: terminal.isTall ? 460 : 260)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if case .failed(let error) = model.board {
            ServiceUnavailable(error: error, state: model.contextState)
        } else if let board = model.board.value, board.confirmed.isEmpty, board.candidates.isEmpty {
            FirstRun(model: model)
        } else {
            switch model.pane {
            case .map: MapPane(model: model)
            case .board: KanbanPane(model: model)
            case .changes: ChangesPane(model: model, terminal: terminal)
            case .files: FilesPane(model: model, terminal: terminal)
            case .browser: BrowserPane(browser: browser)
            }
        }
    }

    /// Узкая середина прячет подписи расширений, но не карты: значки с подсказкой остаются.
    private var tabs: some View {
        ViewThatFits(in: .horizontal) {
            tabRow(compact: false)
            tabRow(compact: true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
    }

    private func tabRow(compact: Bool) -> some View {
        HStack(spacing: 4) {
            ForEach(Pane.allCases) { pane in
                PaneTab(title: Self.title(pane), systemImage: Self.symbol(pane), isOn: model.pane == pane,
                        primary: pane == .map, iconOnly: compact && pane != .map) { model.pane = pane }
                    .help(Self.title(pane) + ": " + Self.hint(pane))
            }
            Spacer(minLength: 8)
            PaneTab(title: "Терминал", systemImage: "terminal", isOn: terminal.isOpen, primary: false, iconOnly: compact) {
                terminal.isOpen.toggle()
            }
            .keyboardShortcut("`", modifiers: .control)
            .help("Терминал снизу (⌃`)")
        }
    }

    static func title(_ pane: Pane) -> String {
        switch pane {
        case .map: "Карта решений"
        case .board: "Канбан"
        case .changes: "Изменения"
        case .files: "Файлы"
        case .browser: "Браузер"
        }
    }

    static func symbol(_ pane: Pane) -> String {
        switch pane {
        case .map: "point.3.connected.trianglepath.dotted"
        case .board: "rectangle.split.3x1"
        case .changes: "plusminus"
        case .files: "folder"
        case .browser: "globe"
        }
    }

    static func hint(_ pane: Pane) -> String {
        switch pane {
        case .map: "Главное: развилки, решения и риски задачи"
        case .board: "Все задачи контекста по колонкам"
        case .changes: "Что изменено в репозитории задачи: всё, что ещё не влито в основную ветку"
        case .files: "Папки проекта и файлы, которые трогали"
        case .browser: "Встроенный браузер: localhost и превью"
        }
    }
}

/// Кнопка вкладки: выбранная — с подложкой цвета акцента.
struct PaneTab: View {
    let title: String
    let systemImage: String
    let isOn: Bool
    let primary: Bool
    var iconOnly = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .labelStyle(PaneTabLabel(iconOnly: iconOnly))
                .font(.callout.weight(primary ? .bold : .semibold))
                .fixedSize()
                .padding(.horizontal, 10)
                .frame(height: 28)
                .foregroundStyle(isOn ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
                .background(isOn ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
                .overlay {
                    if isOn { RoundedRectangle(cornerRadius: 8).strokeBorder(.tint.opacity(0.35)) }
                }
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

private struct PaneTabLabel: LabelStyle {
    let iconOnly: Bool

    func makeBody(configuration: Configuration) -> some View {
        if iconOnly {
            configuration.icon
        } else {
            HStack(spacing: 6) {
                configuration.icon
                configuration.title
            }
        }
    }
}
