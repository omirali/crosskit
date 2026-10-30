import SwiftUI

/// Центр уведомлений поверх чата: что ждёт вас во всех задачах контекста, и решения прямо здесь.
struct InboxPanel: View {

    @Bindable var model: DesktopModel

    var body: some View {
        let inbox = model.inbox
        let items = model.inboxItems
        let unread = items.filter { !inbox.isRead($0.id) }.count
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text(inbox.showsSettings ? "Настройки уведомлений" : "Уведомления").font(.headline)
                if unread > 0, !inbox.showsSettings {
                    Text("\(unread)").font(.caption.weight(.bold)).foregroundStyle(.white)
                        .padding(.horizontal, 6).background(.red, in: .capsule)
                }
                Spacer()
                if !inbox.showsSettings {
                    Button("Прочитать все") { inbox.markRead(items) }
                        .buttonStyle(.borderless).font(.callout)
                }
                Toggle(isOn: Bindable(model.inbox).showsSettings) { Image(systemName: "gearshape") }
                    .toggleStyle(.button).buttonStyle(.borderless)
                    .help("Настройки уведомлений")
                Button("Закрыть", systemImage: "xmark") { withAnimation(.snappy) { inbox.isOpen = false } }
                    .labelStyle(.iconOnly).buttonStyle(.borderless)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            if inbox.showsSettings {
                InboxSettings(inbox: inbox)
            } else {
                list(items)
            }
        }
        .background(.regularMaterial, in: .rect(cornerRadius: 16))
        .overlay { RoundedRectangle(cornerRadius: 16).strokeBorder(.quaternary) }
        .shadow(color: .black.opacity(0.2), radius: 24, y: 10)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private func list(_ items: [InboxItem]) -> some View {
        let inbox = model.inbox
        let shown = items.filter { inbox.tab.holds($0.kind) }
        return VStack(alignment: .leading, spacing: 0) {
            Picker("Фильтр", selection: Bindable(model.inbox).tab) {
                ForEach(InboxTab.allCases) { tab in
                    let count = items.filter { tab.holds($0.kind) && !inbox.isRead($0.id) }.count
                    Text(count > 0 && tab != .all ? "\(tab.rawValue) \(count)" : tab.rawValue).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            if inbox.quiet {
                HStack {
                    Text("«Не беспокоить» — системные уведомления молчат, кроме запросов разрешения; здесь всё копится.")
                    Spacer()
                    Button("Выключить") { inbox.quiet = false }.buttonStyle(.borderless)
                }
                .font(.caption).foregroundStyle(.purple)
                .padding(8)
                .background(.purple.opacity(0.08), in: .rect(cornerRadius: 10))
                .padding(.horizontal, 12).padding(.bottom, 8)
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 6) {
                    ForEach(InboxItem.Group.allCases, id: \.self) { group in
                        let here = shown.filter { $0.group == group }
                        if !here.isEmpty {
                            Text(group.rawValue).font(.caption2.weight(.bold)).foregroundStyle(.secondary)
                                .padding(.horizontal, 6).padding(.top, 6)
                            ForEach(here) { item in
                                InboxRow(item: item, isRead: inbox.isRead(item.id)) { inbox.markRead(item.id) }
                            }
                        }
                    }
                    if shown.isEmpty {
                        VStack(spacing: 6) {
                            Text("Всё спокойно").fontWeight(.semibold)
                            Text("Запросы агентов, развилки без ответа и предложения задач появятся здесь.")
                                .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 12)
            }
        }
    }
}

private struct InboxRow: View {
    let item: InboxItem
    let isRead: Bool
    let markRead: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                markRead()
                item.open()
            } label: {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: Self.symbol(item.kind))
                        .foregroundStyle(Self.tint(item.kind))
                        .frame(width: 28, height: 28)
                        .background(Self.tint(item.kind).opacity(0.13), in: .rect(cornerRadius: 8))
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(item.title).fontWeight(isRead ? .medium : .bold)
                            Spacer()
                            Text(item.time).font(.caption).foregroundStyle(.secondary)
                            if !isRead {
                                Circle().fill(.tint).frame(width: 8, height: 8).accessibilityLabel("не прочитано")
                            }
                        }
                        Text(item.text).font(.callout).multilineTextAlignment(.leading).lineLimit(3)
                        Text(item.place).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if !item.actions.isEmpty {
                HStack(spacing: 6) {
                    ForEach(item.actions) { action in
                        if action.prominent {
                            Button(action.label) {
                                markRead()
                                action.perform()
                            }
                            .buttonStyle(.borderedProminent)
                        } else {
                            Button(action.label) {
                                markRead()
                                action.perform()
                            }
                            .buttonStyle(.bordered)
                        }
                    }
                }
                .controlSize(.small)
                .padding(.leading, 38)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.background, in: .rect(cornerRadius: 12))
        .overlay(alignment: .leading) {
            if !isRead { UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 12).fill(.tint).frame(width: 3) }
        }
    }

    static func symbol(_ kind: InboxItem.Kind) -> String {
        switch kind {
        case .permission: "hand.raised.fill"
        case .decision: "questionmark"
        case .risk: "exclamationmark.triangle.fill"
        case .turn: "checkmark"
        case .map: "arrow.clockwise"
        case .candidate: "sparkles"
        }
    }

    static func tint(_ kind: InboxItem.Kind) -> Color {
        switch kind {
        case .permission: .orange
        case .decision: .accentColor
        case .risk: .red
        case .turn: .green
        case .map: .secondary
        case .candidate: .purple
        }
    }
}

private struct InboxSettings: View {
    @Bindable var inbox: InboxState

    var body: some View {
        Form {
            Section("Системные уведомления macOS, когда окно не в фокусе") {
                Toggle(isOn: $inbox.systemPermission) {
                    Text("Агент ждёт разрешения")
                    Text("всегда, даже в «Не беспокоить»")
                }
                Toggle(isOn: $inbox.systemTurn) {
                    Text("Агент закончил ход")
                    Text("только когда окно не в фокусе")
                }
            }
            Section {
                Toggle(isOn: $inbox.quiet) {
                    Text("Не беспокоить")
                    Text("кроме «агент ждёт разрешения»")
                }
            } footer: {
                Text("Развилки, риски и предложения задач система не присылает: они копятся здесь и видны на карте.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
    }
}
