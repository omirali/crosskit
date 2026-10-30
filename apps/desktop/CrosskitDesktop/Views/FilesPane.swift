import AppKit
import CrosskitKit
import SwiftUI

/// Файлы проекта: папки, где шли беседы, с тем, что в них изменено (по git) и что агент в окне
/// читал или правил. Отмеченный файл открывает свою беседу в чате.
struct FilesPane: View {

    let model: DesktopModel
    let terminal: TerminalModel
    @State private var onlyTouched = false
    @State private var copies: [String: WorkingCopy] = [:]
    @State private var collapsed: Set<String> = []
    @State private var expanded: Set<String> = []

    var body: some View {
        let roots = ChangesPane.roots(of: model)
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Файлы · \(model.project?.name ?? model.currentTask?.name ?? "")").font(.headline)
                    Text(Wording.count(roots.count, "папка", "папки", "папок") + " бесед").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Picker("Какие файлы показывать", selection: $onlyTouched) {
                    Text("Все файлы").tag(false)
                    Text("Тронутые").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
            .background(.background)
            Divider()
            if roots.isEmpty {
                ContentUnavailableView("Папок нет", systemImage: "folder",
                                       description: Text("У бесед того, что открыто, нет каталога на диске."))
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(roots, id: \.self) { root in
                            rootCard(root)
                        }
                    }
                    .padding(14)
                }
            }
            Divider()
            Text("M — изменён · A — добавлен · ? — новый · R — агент в окне только читал. Нажмите отмеченный файл — откроется беседа, которая его трогала.")
                .font(.caption).foregroundStyle(.secondary)
                .padding(.horizontal, 16).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(.background.secondary)
        .task(id: roots) {
            var found: [String: WorkingCopy] = [:]
            for root in roots { found[root] = await Git.workingCopy(at: root) }
            copies = found
        }
    }

    private func rootCard(_ root: String) -> some View {
        let copy = copies[root]
        let isOpen = !collapsed.contains(root)
        let marks = marks(in: root, copy: copy)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Button {
                    if isOpen { collapsed.insert(root) } else { collapsed.remove(root) }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: isOpen ? "chevron.down" : "chevron.right").font(.caption).foregroundStyle(.tertiary)
                        Image(systemName: copy != nil ? "arrow.triangle.branch" : "folder")
                            .foregroundStyle(copy != nil ? Palette.color(.claudeCode) : .accentColor)
                            .frame(width: 26, height: 26)
                            .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 7))
                        VStack(alignment: .leading, spacing: 1) {
                            HStack(spacing: 6) {
                                Text(URL(fileURLWithPath: root).lastPathComponent).fontWeight(.semibold)
                                Text(copy.map { "⎇ \($0.branch)" } ?? "папка")
                                    .font(.caption.monospaced()).foregroundStyle(.secondary)
                                    .padding(.horizontal, 5).background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 4))
                                let changed = marks.values.filter { $0 != "R" }.count
                                if changed > 0 {
                                    Text(Wording.count(changed, "изменение", "изменения", "изменений"))
                                        .font(.caption.weight(.semibold)).foregroundStyle(.orange)
                                }
                            }
                            Text(Wording.path(root)).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                Spacer()
                Button("Терминал здесь", systemImage: "terminal") { terminal.open(at: root) }
                    .labelStyle(.iconOnly).buttonStyle(.borderless)
                Button("Показать в Finder", systemImage: "arrow.up.forward.square") {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: root)])
                }
                .labelStyle(.iconOnly).buttonStyle(.borderless)
            }
            .padding(10)
            if isOpen {
                Divider()
                VStack(alignment: .leading, spacing: 0) {
                    if onlyTouched {
                        ForEach(marks.keys.sorted(), id: \.self) { path in
                            fileRow(name: path, depth: 0, path: root + "/" + path, mark: marks[path])
                        }
                        if marks.isEmpty {
                            Text("Здесь ничего не трогали").font(.callout).foregroundStyle(.secondary).padding(10)
                        }
                    } else {
                        directory(root, relative: "", depth: 0, marks: marks)
                    }
                }
                .padding(4)
            }
        }
        .background(.background, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).strokeBorder(.quaternary) }
    }

    /// Содержимое папки: папки раскрываются по нажатию и читаются тогда же.
    private func directory(_ root: String, relative: String, depth: Int, marks: [String: Character]) -> AnyView {
        let url = URL(fileURLWithPath: root).appendingPathComponent(relative)
        let entries = ((try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: [.isDirectoryKey],
                                                                     options: [.skipsHiddenFiles])) ?? [])
            .map { entry in (name: entry.lastPathComponent, isDirectory: (try? entry.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true) }
            .filter { !Self.skipped.contains($0.name) }
            .sorted { ($0.isDirectory ? 0 : 1, $0.name.lowercased()) < ($1.isDirectory ? 0 : 1, $1.name.lowercased()) }
            .prefix(Self.shownEntries)
        return AnyView(ForEach(Array(entries), id: \.name) { entry in
            let path = relative.isEmpty ? entry.name : relative + "/" + entry.name
            let full = root + "/" + path
            if entry.isDirectory {
                let open = expanded.contains(full)
                let touched = marks.keys.contains { $0.hasPrefix(path + "/") }
                Button {
                    if open { expanded.remove(full) } else { expanded.insert(full) }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: open ? "chevron.down" : "chevron.right").font(.caption2).foregroundStyle(.tertiary).frame(width: 12)
                        Image(systemName: "folder.fill").foregroundStyle(.tint)
                        Text(entry.name).fontWeight(touched ? .semibold : .regular)
                        Spacer()
                    }
                    .padding(.leading, CGFloat(8 + depth * 16)).frame(height: 26).contentShape(.rect)
                }
                .buttonStyle(.plain)
                if open { directory(root, relative: path, depth: depth + 1, marks: marks) }
            } else {
                fileRow(name: entry.name, depth: depth, path: full, mark: marks[path])
            }
        })
    }

    /// Большие служебные каталоги дерево не раскрывает: в них нечего искать глазами.
    private static let skipped: Set<String> = ["node_modules", "target", ".build", "DerivedData", "build", "dist"]
    private static let shownEntries = 300

    private func fileRow(name: String, depth: Int, path: String, mark: Character?) -> some View {
        let chat = mark.flatMap { _ in Self.liveTouch(of: path, in: model) }
        return Button {
            if let chat {
                model.chatKey = chat
                model.target = nil
            }
        } label: {
            HStack(spacing: 6) {
                Color.clear.frame(width: 12)
                Image(systemName: "doc").foregroundStyle(.secondary)
                Text(name).fontWeight(mark != nil ? .semibold : .regular).lineLimit(1).truncationMode(.middle)
                Spacer()
                if chat != nil { Text("в чате").font(.caption).foregroundStyle(.secondary) }
                Text(mark.map { String($0) } ?? "").font(.caption.monospaced().weight(.bold))
                    .foregroundStyle(mark == "R" ? .secondary : mark == "?" ? Color.accentColor : mark == "A" ? .green : .orange)
                    .frame(width: 16)
            }
            .padding(.leading, CGFloat(8 + depth * 16)).padding(.trailing, 8).frame(height: 26).contentShape(.rect)
        }
        .buttonStyle(.plain)
        .allowsHitTesting(chat != nil)
    }

    /// Метки файлов папки: что изменено по git и что агент в окне читал.
    private func marks(in root: String, copy: WorkingCopy?) -> [String: Character] {
        var marks: [String: Character] = [:]
        let prefix = (copy?.root ?? root) + "/"
        for file in copy?.files ?? [] {
            let full = prefix + file.path
            guard full.hasPrefix(root + "/") else { continue }
            marks[String(full.dropFirst(root.count + 1))] = file.mark
        }
        for (_, chat) in model.live.all {
            for item in chat.items {
                guard case .tool(_, let name, let input, _, _) = item else { continue }
                let activity = ToolActivity(tool: name, input: input)
                guard let path = activity.path, path.hasPrefix(root + "/") else { continue }
                let relative = String(path.dropFirst(root.count + 1))
                if marks[relative] == nil { marks[relative] = activity.kind == .read ? "R" : "M" }
            }
        }
        return marks
    }

    /// Живая беседа окна, которая трогала файл.
    static func liveTouch(of path: String, in model: DesktopModel) -> String? {
        model.live.all.first { _, chat in
            chat.items.contains { item in
                guard case .tool(_, let name, let input, _, _) = item else { return false }
                return ToolActivity(tool: name, input: input).path == path
            }
        }?.key
    }
}
