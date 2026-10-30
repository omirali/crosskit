import CrosskitKit
import SwiftUI

/// Терминал снизу: ваши команды в каталоге задачи и команды, которые запускал агент.
struct TerminalPanel: View {

    let model: DesktopModel
    @Bindable var terminal: TerminalModel

    private static let ink = Color(nsColor: .init(white: 0.91, alpha: 1))
    private static let ground = Color(nsColor: .init(white: 0.11, alpha: 1))
    private static let prompt = Color(red: 0.49, green: 0.89, blue: 0.66)

    var body: some View {
        let directory = terminal.directory ?? model.taskDirectory(model.taskID) ?? model.scopeDirectories.first ?? NSHomeDirectory()
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                Text("Терминал").font(.caption.weight(.bold)).foregroundStyle(Self.ink.opacity(0.85)).padding(.trailing, 6)
                tab("zsh · вы", .mine)
                tab("агент · Claude Code", .agent)
                Spacer()
                Text("\(Wording.path(directory)) · zsh").font(.caption.monospaced()).foregroundStyle(Self.ink.opacity(0.55))
                    .lineLimit(1).truncationMode(.head)
                if terminal.running != nil {
                    Button("Прервать", systemImage: "stop.fill") { terminal.interrupt() }
                }
                Button("Очистить", systemImage: "eraser") { terminal.clear() }
                Button(terminal.isTall ? "Вернуть высоту" : "Развернуть", systemImage: terminal.isTall ? "chevron.down" : "chevron.up") {
                    terminal.isTall.toggle()
                }
                Button("Скрыть терминал", systemImage: "xmark") { terminal.isOpen = false }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .foregroundStyle(Self.ink.opacity(0.75))
            .padding(.horizontal, 10).padding(.vertical, 5)
            Divider().overlay(Self.ink.opacity(0.1))
            ScrollViewReader { scroll in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        if terminal.tab == .mine {
                            ForEach(terminal.lines) { line in
                                Text(line.text).foregroundStyle(color(line.kind)).textSelection(.enabled)
                            }
                            HStack(spacing: 8) {
                                Text("\(URL(fileURLWithPath: directory).lastPathComponent) ❯").foregroundStyle(Self.prompt)
                                TextField("например: cargo test, git status", text: $terminal.input)
                                    .textFieldStyle(.plain)
                                    .foregroundStyle(Self.ink)
                                    .onSubmit { terminal.run(in: directory) }
                                    .disabled(terminal.running != nil)
                            }
                            .id("prompt")
                        } else {
                            agentLines
                        }
                    }
                    .font(.system(.callout, design: .monospaced))
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: terminal.lines.count) { scroll.scrollTo("prompt", anchor: .bottom) }
            }
        }
        .background(Self.ground)
        .environment(\.colorScheme, .dark)
    }

    private func tab(_ title: String, _ tab: TerminalModel.Tab) -> some View {
        Button(title) { terminal.tab = tab }
            .buttonStyle(.plain)
            .font(.caption.monospaced())
            .foregroundStyle(terminal.tab == tab ? Self.ink : Self.ink.opacity(0.6))
            .padding(.horizontal, 10).frame(height: 22)
            .background(terminal.tab == tab ? Self.ink.opacity(0.12) : .clear, in: .rect(cornerRadius: 6))
    }

    private func color(_ kind: TerminalModel.Line.Kind) -> Color {
        switch kind {
        case .command: Self.prompt
        case .output: Self.ink
        case .error: Color(red: 1, green: 0.56, blue: 0.54)
        case .note: Self.ink.opacity(0.5)
        }
    }

    /// Команды, которые запускал агент открытой беседы, с их выводом.
    @ViewBuilder
    private var agentLines: some View {
        let chat = model.shownChatKey.flatMap(model.live.chat)
        let runs: [(String, String?, Bool)] = (chat?.items ?? []).compactMap { item in
            guard case .tool(_, "Bash", let input, let result, let failed) = item else { return nil }
            return (ToolActivity(tool: "Bash", input: input).subject, result, failed)
        }
        if runs.isEmpty {
            Text(chat == nil ? "Здесь будут команды агента, когда беседа идёт в окне." : "Агент в этой беседе ещё ничего не запускал.")
                .foregroundStyle(Self.ink.opacity(0.5))
        }
        ForEach(Array(runs.enumerated()), id: \.offset) { _, run in
            Text("$ " + run.0).foregroundStyle(Self.prompt)
            if let result = run.1 {
                Text(result.count > 4_000 ? String(result.prefix(4_000)) + "\n…" : result)
                    .foregroundStyle(run.2 ? color(.error) : Self.ink).textSelection(.enabled)
            }
        }
    }
}
