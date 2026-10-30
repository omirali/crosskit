import CrosskitKit
import SwiftUI

/// Просьба назвать задачу: подтвердить кандидата или переименовать.
struct NameRequest: Identifiable {
    let id = UUID()
    let title: String
    let initial: String
    let decision: (String) -> OwnerDecision
}

/// Просьба назвать причину решения о риске: решение без причины книга задач не примет.
struct ReasonRequest: Identifiable {
    let id = UUID()
    let risk: String
    let verdict: RiskStatus
    let decision: (String) -> OwnerDecision
}

struct NameSheet: View {
    let request: NameRequest
    let done: (OwnerDecision?) -> Void
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(request.title).font(.headline)
            TextField("Имя задачи", text: $name)
                .textFieldStyle(.roundedBorder)
                .onSubmit(submit)
            HStack {
                Spacer()
                Button("Отмена", role: .cancel) { done(nil) }
                    .keyboardShortcut(.cancelAction)
                Button("Сохранить", action: submit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 420)
        .onAppear { name = request.initial }
    }

    private func submit() {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        done(request.decision(trimmed))
    }
}

struct ReasonSheet: View {
    let request: ReasonRequest
    let done: (OwnerDecision?) -> Void
    @State private var reason = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(request.verdict == .resolved ? "Риск закрыт" : "Принять риск").font(.headline)
            Text(request.risk).foregroundStyle(.secondary).lineLimit(3)
            Text(request.verdict == .resolved
                 ? "Чем закрыт: коммит, тест, решение. Причина останется у риска."
                 : "Почему работа идёт с этим риском. Принятый риск становится серым и помнит причину.")
                .font(.callout).foregroundStyle(.secondary)
            TextField("Причина", text: $reason, axis: .vertical)
                .lineLimit(3...6)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button("Отмена", role: .cancel) { done(nil) }
                    .keyboardShortcut(.cancelAction)
                Button(request.verdict == .resolved ? "Закрыть риск" : "Принять риск") {
                    done(request.decision(reason.trimmingCharacters(in: .whitespacesAndNewlines)))
                }
                .keyboardShortcut(.defaultAction)
                .disabled(reason.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 460)
    }
}

/// Ход построения карты: строки команды, как в терминале, и чем кончилось.
struct BuildSheet: View {
    @Bindable var model: DesktopModel

    var body: some View {
        let run = model.build ?? BuildRun()
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                if !run.finished { ProgressView().controlSize(.small) }
                Text(title(run)).font(.headline)
            }
            ScrollViewReader { scroll in
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(run.lines.enumerated()), id: \.offset) { index, line in
                            Text(line).font(.system(.caption, design: .monospaced)).id(index)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(height: 220)
                .padding(8)
                .background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 8))
                .onChange(of: run.lines.count) { scroll.scrollTo(run.lines.count - 1, anchor: .bottom) }
            }
            if let failure = run.failure {
                Text(failure).foregroundStyle(.red).textSelection(.enabled)
            }
            HStack {
                Text("Окна, размеченные до остановки, остаются в карте.").font(.caption).foregroundStyle(.secondary)
                Spacer()
                if run.finished {
                    Button("Закрыть") { model.closeBuild() }.keyboardShortcut(.defaultAction)
                } else {
                    Button("Остановить", role: .cancel) { model.cancelBuild() }
                }
            }
        }
        .padding(20)
        .frame(width: 560)
    }

    private func title(_ run: BuildRun) -> String {
        if run.failure != nil { return "Построение остановилось" }
        return run.finished ? "Карта достроена" : "Строю карту…"
    }
}
