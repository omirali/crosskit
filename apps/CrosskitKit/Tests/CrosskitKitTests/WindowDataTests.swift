import Foundation
import Testing
@testable import CrosskitKit

@Suite("Данные окна: чат, проекты, правки")
struct WindowDataTests {

    @Test("Реплика знает свои узлы карты, а узел — свою реплику")
    func repliesAndNodesPointAtEachOther() throws {
        let tree = DecisionTree(map: try MapFixture.answer())
        #expect(tree.anchors(session: "aaaa-1", reply: 12) == [.step(0)])
        #expect(tree.anchors(session: "bbbb-2", reply: 9) == [.risk("r0")])
        #expect(tree.anchors(session: "bbbb-2", reply: 30).isEmpty, "находка вне задачи на карте не висит")
        #expect(tree.anchors(session: "aaaa-1", reply: 13).isEmpty)

        #expect(tree.source(of: .step(0))?.reply == 12, "у шага с выбором — ведущая ветка")
        #expect(tree.source(of: .step(1))?.reply == 40, "у шага без выбора — первая ветка, ждущая решения")
        #expect(tree.source(of: .risk("r0"))?.session == "aaaa-1")
        #expect(tree.waitingStep?.id == 1)
    }

    @Test("Реплики задачи читаются из ответа task-speech")
    func speechIsRead() throws {
        let line = #"{"schema":2,"task":"Двойная скидка","task_id":"t-1","conversations":[{"session":"aaaa-1","name":"Почему скидка дважды","first_at_unix_ms":1759474800000,"replies":[{"n":1,"owner":true,"text":"Почему?"},{"n":2,"owner":false,"text":"Вижу второе применение."}]}],"risk_decisions":[]}"#
        let speech = try ServiceAnswer.decode(TaskSpeech.self, from: Data(line.utf8))
        #expect(speech.taskId == "t-1")
        #expect(speech.conversation("aaaa-1")?.replies.map(\.owner) == [true, false])
    }

    @Test("Задача — в проекте, где шло больше её бесед; без каталога — «Без папки» в конце")
    func tasksFallIntoProjects() throws {
        func conversation(_ session: String, _ directory: String?, last: Int64 = 0) -> String {
            let dir = directory.map { "\"\($0)\"" } ?? "null"
            return #"{"session":"\#(session)","platform":"claude-code","name":null,"opening":null,"first_at_unix_ms":null,"last_at_unix_ms":\#(last),"tie":"strong","signals":[],"working_directory":\#(dir)}"#
        }
        let line = """
        {"context":"work","confirmed":[
          {"id":"t1","name":"Без каталога","conversations":[\(conversation("s0", nil))],"proposed":[],"progress":null,"finished_at_unix_ms":null},
          {"id":"t2","name":"Скидка","conversations":[\(conversation("s1", "/w/pay/src")),\(conversation("s2", "/w/pay")),\(conversation("s3", "/w/web", last: 9))],"proposed":[],"progress":null,"finished_at_unix_ms":null}],
         "tasks":[{"conversations":[\(conversation("s4", "/w/web"))]}]}
        """
        let board = try ServiceAnswer.decode(TaskBoard.self, from: Data(line.utf8))
        let projects = ProjectGrouping.projects(of: board) { $0.hasPrefix("/w/pay") ? "/w/pay" : $0 }

        #expect(projects.map(\.name) == ["pay", "web", Project.unplaced])
        #expect(projects[0].taskIDs == ["t2"])
        #expect(projects[1].candidateSessions == ["s4"])
        #expect(projects[2].taskIDs == ["t1"])
    }

    @Test("git: числа строк, новые файлы и строки разницы с номерами")
    func gitIsRead() {
        let files = Git.files(numstat: "3\t2\tsrc/cart/view.rs\n1\t0\t.env.example\n",
                              status: " M src/cart/view.rs\n M .env.example\n?? src/preview.rs\n")
        #expect(files.map(\.path) == ["src/cart/view.rs", ".env.example", "src/preview.rs"])
        #expect(files.map(\.mark) == ["M", "M", "?"])
        #expect(files[0].added == 3 && files[0].removed == 2)

        let lines = Git.lines(ofUnified: """
        diff --git a/x b/x
        --- a/x
        +++ b/x
        @@ -41,3 +41,3 @@ fn render_total
         fn render_total(cart: &Cart) -> Html {
        -    let total = sum();
        +    let total = preview.total;
             html! {}
        """)
        #expect(lines.map(\.kind) == [.hunk, .context, .removed, .added, .context])
        #expect(lines[1].old == 41 && lines[1].new == 41)
        #expect(lines[2].old == 42 && lines[3].new == 42)
        #expect(lines[4].old == 43 && lines[4].new == 43)
    }

    @Test("Вызов инструмента — словами: что сделал, с чем и разница правки")
    func toolCallsInWords() {
        let edit = ToolActivity(tool: "Edit",
                                input: #"{"file_path":"/w/pay/src/cart/view.rs","old_string":"fn a() {\n    let t = sum();\n}","new_string":"fn a() {\n    let p = preview();\n    let t = p.total;\n}"}"#,
                                directory: "/w/pay")
        #expect(edit.verb == "Изменил")
        #expect(edit.subject == "src/cart/view.rs")
        #expect(edit.added == 2 && edit.removed == 1)
        #expect(edit.lines.map(\.kind) == [.context, .removed, .added, .added, .context])

        let run = ToolActivity(tool: "Bash", input: #"{"command":"cargo test order::preview"}"#)
        #expect(run.kind == .run && run.subject == "cargo test order::preview" && run.path == nil)
    }

    @Test("Быстрый вопрос: без журнала, только чтение, ответ — поле result")
    func aQuickQuestionLeavesNoTrace() throws {
        let arguments = QuickQuestion.arguments(question: "Где считается скидка?", model: "haiku")
        #expect(arguments.contains("--no-session-persistence"))
        #expect(arguments.firstIndex(of: "--tools").map { arguments[$0 + 1] } == "Read,Grep,Glob")

        let answer = try #require(QuickQuestion.answer(from: Data(#"{"type":"result","is_error":false,"result":"В order.rs","total_cost_usd":0.002}"#.utf8)))
        #expect(answer.text == "В order.rs" && answer.costUSD == 0.002)
        #expect(QuickQuestion.answer(from: Data(#"{"type":"result","is_error":true,"result":"нет входа"}"#.utf8)) == nil)
    }
}
