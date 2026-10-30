import AppKit
import CrosskitKit
import SwiftUI

/// Служба не дала ответа: что случилось и что сделать.
struct ServiceUnavailable: View {
    let error: ServiceError
    var state: ContextState?

    var body: some View {
        let failure = Wording.failure(error, state: state)
        ContentUnavailableView(failure.title, systemImage: "bolt.horizontal.circle",
                               description: Text(failure.explanation))
    }
}

/// Задача подтверждена, но карты ещё нет: построение спрашивает модель и идёт командой.
struct NotMapped: View {
    let task: ConfirmedTask
    let answer: TaskMapAnswer
    let build: () -> Void

    var body: some View {
        let replies = answer.conversations.map(\.replies).reduce(0, +)
        ContentUnavailableView {
            Label("Карта ещё не построена", systemImage: "point.3.connected.trianglepath.dotted")
        } description: {
            Text("\(Wording.count(answer.conversations.count, "беседа", "беседы", "бесед")), \(Wording.count(replies, "реплика", "реплики", "реплик")). Построение спрашивает модель о каждой реплике и тратит лимит подписки.")
        } actions: {
            Button("Построить карту…", action: build)
                .buttonStyle(.borderedProminent)
            CommandBox(command: "crosskit --context \(answer.context) tasks map \"\(task.name)\" --build")
        }
    }
}

/// Кандидат в задачи: беседы, которые связала сшивка, и как его подтвердить.
struct CandidateView: View {
    let model: DesktopModel
    let candidate: CandidateTask

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("КАНДИДАТ В ЗАДАЧИ").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            Text(candidate.conversations.first?.title ?? "Кандидат").font(.title2.weight(.bold))
            Text("Беседы связаны сшивкой. Карта строится для задачи, которую вы подтвердили.")
                .foregroundStyle(.secondary)
            ForEach(candidate.conversations) { conversation in
                HStack(spacing: 10) {
                    ToolBadge(platform: conversation.platform)
                    Text(conversation.title).lineLimit(1)
                    Spacer()
                    Text(Wording.day(conversation.firstAt)).foregroundStyle(.secondary)
                }
                if !conversation.signals.isEmpty {
                    Text(conversation.signals.joined(separator: " · ")).font(.caption).foregroundStyle(.tertiary)
                        .padding(.leading, 32)
                }
            }
            HStack {
                CandidateActions(model: model, candidate: candidate)
            }
            .buttonStyle(.bordered)
        }
        .padding(32)
        .frame(maxWidth: 560, alignment: .leading)
    }
}

/// Команда терминала, которую можно скопировать.
struct CommandBox: View {
    let command: String

    var body: some View {
        HStack {
            Text(command).font(.system(.callout, design: .monospaced)).textSelection(.enabled).lineLimit(2)
            Spacer(minLength: 8)
            Button("Скопировать", systemImage: "doc.on.doc") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
        }
        .padding(10)
        .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
    }
}
