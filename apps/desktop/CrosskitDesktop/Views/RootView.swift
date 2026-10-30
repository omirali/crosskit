import CrosskitKit
import SwiftUI

/// Окно (макет `design/UXMain.dc.html`): слева проекты и задачи, в середине карта и расширения
/// вокруг задачи, справа — чат с агентом. Карта и чат связаны: узел открывает свою реплику, а
/// у реплики видно, что из неё стало на карте.
struct RootView: View {

    @Bindable var model: DesktopModel
    @State private var terminal = TerminalModel()
    @State private var browser = BrowserModel()

    var body: some View {
        NavigationSplitView {
            ProjectSidebar(model: model)
                .navigationSplitViewColumnWidth(min: 220, ideal: 250, max: 320)
        } detail: {
            HSplitView {
                WorkPane(model: model, terminal: terminal, browser: browser)
                    .frame(minWidth: 460, maxWidth: .infinity)
                HStack(spacing: 0) {
                    if model.showsChats {
                        ChatList(model: model)
                            .frame(width: 250)
                        Divider()
                    }
                    ChatPane(model: model, terminal: terminal)
                        .frame(minWidth: 380, idealWidth: 500, maxWidth: 720)
                }
            }
        }
        .navigationTitle(model.context ?? "Crosskit")
        .task { await model.start() }
        .task(id: model.taskID) {
            if let id = model.taskID { await model.loadTask(id) }
        }
        .task(id: model.shownChatKey) {
            // Беседа проекта — из чужой задачи: её реплики и карта нужны чату.
            if let key = model.shownChatKey, let id = model.taskID(of: key), id != model.taskID,
               model.speech(of: id).value == nil {
                await model.loadTask(id)
            }
        }
        .onAppear {
            // Браузер не держит окно: только строку ввода, и цикла владения нет.
            let composer = model.composer
            browser.handOff = { text in
                composer.draft = composer.draft.isEmpty ? text : composer.draft + " " + text
            }
        }
        .sheet(item: $model.nameRequest) { request in
            NameSheet(request: request) { decision in
                model.nameRequest = nil
                if let decision { Task { await model.decide(decision) } }
            }
        }
        .sheet(item: $model.reasonRequest) { request in
            ReasonSheet(request: request) { decision in
                model.reasonRequest = nil
                if let decision { Task { await model.decide(decision) } }
            }
        }
        .sheet(isPresented: Binding(get: { model.build != nil }, set: { if !$0 { model.closeBuild() } })) {
            BuildSheet(model: model)
                .interactiveDismissDisabled(model.build?.finished == false)
        }
        .fileImporter(isPresented: Binding(get: { model.composer.waitingForDirectory != nil },
                                           set: { if !$0 { model.composer.waitingForDirectory = nil } }),
                      allowedContentTypes: [.folder]) { result in
            let text = model.composer.waitingForDirectory
            model.composer.waitingForDirectory = nil
            if case .success(let url) = result, let text {
                model.startChat(saying: text, in: url.path)
            }
        }
        .alert(model.notice?.title ?? "", isPresented: Binding(get: { model.notice != nil }, set: { if !$0 { model.notice = nil } })) {
            Button("Понятно") { model.notice = nil }
        } message: {
            Text(model.notice?.explanation ?? "")
        }
    }
}
