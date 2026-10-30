import CrosskitKit
import SwiftUI

/// Первый запуск (макет `design/UXEmpty.dc.html`): задач и предложений ещё нет. Три шага —
/// где лежат беседы агентов, какие папки читает Crosskit и первая карта.
///
/// Окно здесь только смотрит: имена файлов бесед оно считает само, а папки контекста задаёт
/// `crosskit setup` — конфигурацию пишет ядро, а не окно.
struct FirstRun: View {

    let model: DesktopModel
    @State private var found: [(name: String, platform: Platform, count: Int)]?

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                VStack(spacing: 10) {
                    Image(systemName: "point.3.connected.trianglepath.dotted").font(.system(size: 44)).foregroundStyle(.tint)
                    Text("Здесь появится карта решений").font(.title.weight(.bold))
                    Text("Crosskit читает ваши беседы с Claude Code и Codex, собирает их в задачи и показывает, что решили, что отвергли и что осталось без ответа. Всё хранится на этом компьютере.")
                        .multilineTextAlignment(.center).foregroundStyle(.secondary).frame(maxWidth: 520)
                }
                VStack(spacing: 10) {
                    step(1, "Найти агентов на этом компьютере", done: found.map { !$0.isEmpty } ?? false, active: found == nil) {
                        if let found {
                            if found.isEmpty {
                                Text("Бесед Claude Code и Codex в обычных местах нет: ~/.claude/projects и ~/.codex/sessions пусты.")
                                    .foregroundStyle(.secondary)
                            }
                            ForEach(found, id: \.name) { agent in
                                HStack(spacing: 8) {
                                    ToolBadge(platform: agent.platform, size: 18)
                                    Text(agent.name).fontWeight(.semibold)
                                    Text(Wording.count(agent.count, "беседа", "беседы", "бесед")).foregroundStyle(.secondary)
                                    Spacer()
                                    Text("найден").font(.caption.weight(.semibold)).foregroundStyle(.green)
                                }
                                .padding(8)
                                .background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 9))
                            }
                        } else {
                            Text("Crosskit посмотрит, где агенты хранят беседы. Только имена файлов — содержимое не читается.")
                                .foregroundStyle(.secondary)
                            Button("Найти агентов") { found = Self.scan() }.buttonStyle(.borderedProminent)
                        }
                    }
                    step(2, "Добавить папки проектов", done: false, active: found != nil) {
                        Text("Проект — папка или репозиторий, где вы работаете с агентами. Какие папки читать, спрашивает настройка Crosskit в терминале; служба подхватит их сама, и беседы станут задачами.")
                            .foregroundStyle(.secondary)
                        CommandBox(command: "crosskit setup")
                    }
                    step(3, "Построить первую карту", done: false, active: false) {
                        Text("Когда появятся задачи, модель прочитает их беседы и найдёт развилки, решения и риски. Это стоит около цента за реплику и запускается только по вашей команде — с ценой заранее.")
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: 620)
                Text("Контекст «\(model.context ?? "")» пока пуст. Чат справа работает и без задач.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 32)
            .padding(.vertical, 48)
            .frame(maxWidth: .infinity)
        }
    }

    private func step(_ number: Int, _ title: String, done: Bool, active: Bool, @ViewBuilder content: () -> some View) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Text(done ? "✓" : "\(number)").fontWeight(.bold)
                .foregroundStyle(done || active ? .white : .secondary)
                .frame(width: 28, height: 28)
                .background(done ? AnyShapeStyle(.green) : active ? AnyShapeStyle(.tint) : AnyShapeStyle(.quaternary), in: .circle)
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.title3.weight(.semibold))
                content()
            }
            Spacer(minLength: 0)
        }
        .padding(16)
        .background(.background, in: .rect(cornerRadius: 14))
        .overlay { RoundedRectangle(cornerRadius: 14).strokeBorder(active ? AnyShapeStyle(.tint) : AnyShapeStyle(.quaternary), lineWidth: active ? 1.5 : 0.5) }
        .opacity(done || active ? 1 : 0.65)
    }

    /// Сколько журналов бесед лежит там, где их держат агенты: только имена файлов.
    static func scan() -> [(name: String, platform: Platform, count: Int)] {
        let home = URL(fileURLWithPath: NSHomeDirectory())
        let places: [(String, Platform, String)] = [("Claude Code", .claudeCode, ".claude/projects"), ("Codex", .codex, ".codex/sessions")]
        return places.compactMap { name, platform, folder in
            guard let walk = FileManager.default.enumerator(at: home.appendingPathComponent(folder), includingPropertiesForKeys: nil)
            else { return nil }
            let count = walk.compactMap { ($0 as? URL)?.pathExtension == "jsonl" ? 1 : nil }.count
            return count > 0 ? (name, platform, count) : nil
        }
    }
}
