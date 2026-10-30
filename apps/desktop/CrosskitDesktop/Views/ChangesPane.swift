import CrosskitKit
import SwiftUI

/// Изменения задачи: что git видит в её репозитории — всё, что ушло от основной ветки и ещё не
/// влито, вместе с незакоммиченным. Правку, которую агент сделал в окне, можно открыть в чате.
struct ChangesPane: View {

    let model: DesktopModel
    let terminal: TerminalModel
    @State private var copies: [WorkingCopy] = []
    @State private var loaded = false
    @State private var open: Set<String> = []
    @State private var diffs: [String: [DiffLine]] = [:]

    var body: some View {
        let roots = Self.roots(of: model)
        VStack(spacing: 0) {
            if !loaded {
                ProgressView("Спрашиваю git…").frame(maxHeight: .infinity)
            } else if copies.isEmpty {
                ContentUnavailableView("Изменений не видно", systemImage: "plusminus",
                                       description: Text(roots.isEmpty
                                                         ? "У бесед задачи нет каталога на диске."
                                                         : "Каталоги задачи — не репозитории git, или в них ничего не менялось."))
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(copies, id: \.root) { copy in
                            section(copy)
                        }
                    }
                    .padding(14)
                }
            }
        }
        .background(.background.secondary)
        .task(id: roots) {
            loaded = false
            var found: [WorkingCopy] = []
            for root in roots {
                if let copy = await Git.workingCopy(at: root), !found.contains(where: { $0.root == copy.root }) {
                    found.append(copy)
                }
            }
            copies = found
            diffs = [:]
            loaded = true
        }
    }

    /// Корни бесед того, что открыто.
    static func roots(of model: DesktopModel) -> [String] {
        var seen: Set<String> = []
        return model.scopeDirectories.map(ProjectGrouping.gitRoot).filter { seen.insert($0).inserted }
    }

    private func section(_ copy: WorkingCopy) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text(URL(fileURLWithPath: copy.root).lastPathComponent).font(.headline)
                    Text("\(copy.base) → \(copy.branch) · \(Wording.count(copy.files.count, "файл", "файла", "файлов")) · +\(copy.added) −\(copy.removed)")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Терминал здесь", systemImage: "terminal") { terminal.open(at: copy.root) }
                    .labelStyle(.iconOnly).buttonStyle(.borderless)
            }
            if copy.files.isEmpty {
                Text("Изменений нет: ветка совпадает с базой.").font(.callout).foregroundStyle(.secondary)
            }
            ForEach(copy.files) { file in
                fileCard(file, in: copy)
            }
        }
    }

    private func fileCard(_ file: WorkingCopy.File, in copy: WorkingCopy) -> some View {
        let key = copy.root + "/" + file.path
        let isOpen = open.contains(key)
        let edit = Self.liveEdit(of: key, in: model)
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                if isOpen { open.remove(key) } else { open.insert(key) }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right").font(.caption).foregroundStyle(.tertiary)
                    Text(file.path).font(.system(.callout, design: .monospaced).weight(.semibold)).lineLimit(1).truncationMode(.middle)
                    if file.added > 0 { Text("+\(file.added)").font(.caption.monospaced()).foregroundStyle(.green) }
                    if file.removed > 0 { Text("−\(file.removed)").font(.caption.monospaced()).foregroundStyle(.red) }
                    Spacer()
                    Text(Self.mark(file.mark)).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    if edit != nil {
                        Text("правка агента в окне").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                    }
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if isOpen {
                Divider()
                if let lines = diffs[key] {
                    DiffView(title: nil, lines: lines)
                        .padding(8)
                } else {
                    ProgressView().controlSize(.small).padding(10)
                        .task { diffs[key] = await Git.diff(of: file, in: copy) }
                }
                if let edit {
                    Divider()
                    Button("Открыть в чате беседу, где сделана правка →") {
                        model.chatKey = edit
                        model.target = nil
                    }
                    .buttonStyle(.link)
                    .font(.callout)
                    .padding(.horizontal, 10).padding(.vertical, 6)
                }
            }
        }
        .background(.background, in: .rect(cornerRadius: 10))
        .overlay { RoundedRectangle(cornerRadius: 10).strokeBorder(.quaternary) }
    }

    static func mark(_ mark: Character) -> String {
        switch mark {
        case "A": "добавлен"
        case "D": "удалён"
        case "?": "новый"
        default: "изменён"
        }
    }

    /// Живая беседа окна, которая правила этот файл.
    static func liveEdit(of path: String, in model: DesktopModel) -> String? {
        model.live.all.first { _, chat in
            chat.items.contains { item in
                guard case .tool(_, let name, let input, _, false) = item else { return false }
                let activity = ToolActivity(tool: name, input: input)
                return (activity.kind == .edit || activity.kind == .create) && activity.path == path
            }
        }?.key
    }
}
