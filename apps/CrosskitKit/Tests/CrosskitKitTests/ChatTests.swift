import Foundation
import Testing
@testable import CrosskitKit

@Suite("Чат с claude", .serialized)
struct ChatTests {

    /// Строки в том виде, в каком их пишет Claude Code 2.1.41 (данные выдуманные).
    static let lines = [
        #"{"type":"system","subtype":"init","cwd":"/Users/alice/Projects/payments-api","session_id":"s-1","model":"claude-sonnet","tools":["Bash"]}"#,
        #"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Смотрю "}}}"#,
        #"{"type":"stream_event","event":{"type":"message_start"}}"#,
        #"{"type":"assistant","message":{"content":[{"type":"text","text":"Смотрю order.rs"},{"type":"tool_use","id":"t-1","name":"Write","input":{"file_path":"/w/a.txt","content":"ok"}}]}}"#,
        #"{"type":"control_request","request_id":"r-1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/w/a.txt","content":"ok"},"tool_use_id":"t-1"}}"#,
        #"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t-1","type":"tool_result","content":"File created","is_error":false}]}}"#,
        #"{"type":"control_request","request_id":"r-2","request":{"subtype":"hook_callback"}}"#,
        #"{"type":"result","subtype":"success","is_error":false,"duration_ms":3032,"result":"готово","session_id":"s-1","total_cost_usd":0.03}"#,
    ]

    @Test("Строки потока становятся событиями чата")
    func linesBecomeEvents() {
        let events = Self.lines.flatMap { ChatEvent.parse(Data($0.utf8)) }
        #expect(events.count == 7, "message_start ничего не показывает")
        #expect(events[0] == .started(session: "s-1", directory: "/Users/alice/Projects/payments-api", model: "claude-sonnet"))
        #expect(events[1] == .text("Смотрю "))
        #expect(events[2] == .message([.text("Смотрю order.rs"),
                                       .toolUse(id: "t-1", name: "Write", input: #"{"content":"ok","file_path":"/w/a.txt"}"#)]))
        #expect(events[3] == .permission(PermissionRequest(id: "r-1", tool: "Write",
                                                           input: #"{"content":"ok","file_path":"/w/a.txt"}"#, toolUseID: "t-1")))
        #expect(events[4] == .toolResult(id: "t-1", text: "File created", isError: false))
        #expect(events[5] == .unansweredRequest(id: "r-2", subtype: "hook_callback"))
        #expect(events[6] == .finished(TurnResult(isError: false, session: "s-1", costUSD: 0.03, durationMs: 3032, text: "готово")))
    }

    @Test("Аргументы claude: поток в обе стороны, разрешения в потоке, вводная и продолжение")
    func theArguments() {
        let start = ChatSession.Start(directory: URL(fileURLWithPath: "/w"), resume: "s-1", brief: "вводная",
                                      permissionMode: "acceptEdits")
        #expect(start.arguments == ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
                                    "--include-partial-messages", "--permission-prompt-tool", "stdio",
                                    "--resume", "s-1", "--append-system-prompt", "вводная", "--permission-mode", "acceptEdits"])
        #expect(!start.arguments.contains("--no-session-persistence"), "беседа из окна сохраняется, как из терминала")
    }

    /// Поддельный `claude`, который ведёт себя как настоящий: молчит до первого слова человека
    /// (настоящий в режиме потокового ввода присылает init только после него), отвечает кусками
    /// и просит разрешения, ждёт ответа окна и записывает его в `answers`, заканчивает ход и
    /// выходит, когда окно закрыло ввод.
    private func fakeClaude(answers: URL) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("ck-chat-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let script = directory.appendingPathComponent("claude")
        let body = """
            #!/bin/sh
            while IFS= read -r line; do
              case "$line" in
                *control_response*) printf '%s\\n' "$line" >> '\(answers.path)'
                  printf '%s\\n' '\(Self.lines[5])'
                  printf '%s\\n' '\(Self.lines[7])' ;;
                *'"type":"user"'*) printf '%s\\n' '\(Self.lines[0])'
                  printf '%s\\n' '\(Self.lines[1])'
                  printf '%s\\n' '\(Self.lines[3])'
                  printf '%s\\n' '\(Self.lines[4])' ;;
              esac
            done
            """
        try Data(body.utf8).write(to: script)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    @Test("Беседа с поддельным claude: поток, разрешение от человека, конец хода")
    func aWholeTurn() async throws {
        let answers = FileManager.default.temporaryDirectory.appendingPathComponent("ck-answers-\(UUID().uuidString.prefix(8))")
        let program = try fakeClaude(answers: answers)
        let session = ChatSession(start: .init(directory: FileManager.default.temporaryDirectory), program: program.path)
        var seen: [ChatEvent] = []
        let events = await session.events()
        await session.send("Почему скидка дважды?")
        for try await event in events {
            seen.append(event)
            switch event {
            case .permission(let request): await session.answer(request, allow: true)
            case .finished: await session.close()
            default: break
            }
        }
        #expect(seen.first == .started(session: "s-1", directory: "/Users/alice/Projects/payments-api", model: "claude-sonnet"))
        #expect(seen.contains(.text("Смотрю ")))
        #expect(seen.contains(.toolResult(id: "t-1", text: "File created", isError: false)))
        #expect(seen.last.map { if case .finished = $0 { true } else { false } } == true)

        let answer = try JSONSerialization.jsonObject(with: Data(contentsOf: answers)) as? [String: Any]
        let response = (answer?["response"] as? [String: Any])?["response"] as? [String: Any]
        #expect(response?["behavior"] as? String == "allow")
        #expect((response?["updatedInput"] as? [String: Any])?["content"] as? String == "ok", "вход инструмента возвращён без правок")
    }

    @Test("Вводная называет выбранное, отклонённое с причиной, HEAD и открытые риски")
    func theBriefCarriesTheMap() throws {
        let brief = TaskBrief.of(DecisionTree(map: try MapFixture.answer()))
        #expect(brief.contains("«Двойная скидка»"))
        #expect(brief.contains("1. Считать скидку только в заказе → выбрал владелец: только в order.rs — корзина показывает"))
        #expect(brief.contains("отклонено: флаг в заказе — ломает возврат"))
        #expect(brief.contains("ждёт решения владельца: «Как показывать до заказа?» Варианты: через API заказа; спросить продакта."))
        #expect(brief.contains("- critical: Частичный возврат не пересчитан (поднимался в 2 беседах)"))
        #expect(!brief.contains("Экспорт отчётов"), "находка вне задачи — не риск задачи")
    }
}
