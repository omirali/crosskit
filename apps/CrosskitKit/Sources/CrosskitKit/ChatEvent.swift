import Foundation

/// Что `claude -p --output-format stream-json` говорит по ходу беседы, по строке на событие
/// (ADR 0062). Формат проверен на установленном Claude Code 2.1.41: `system/init`, куски текста в
/// `stream_event`, сообщения `assistant` и `user`, запрос разрешения `control_request` и итог
/// хода `result`.
public enum ChatEvent: Sendable, Equatable {
    /// Беседа началась: её сессия, каталог и модель.
    case started(session: String, directory: String, model: String?)
    /// Кусок текста ответа, как только он пришёл.
    case text(String)
    /// Кусок размышлений модели.
    case thinking(String)
    /// Сообщение модели целиком: его текст и вызовы инструментов.
    case message([ChatBlock])
    /// Что вернул инструмент.
    case toolResult(id: String, text: String, isError: Bool)
    /// Инструмент просит разрешения у человека.
    case permission(PermissionRequest)
    /// CLI спросил то, чего окно не умеет: ему ответят отказом, чтобы он не ждал.
    case unansweredRequest(id: String, subtype: String)
    /// Ход закончился.
    case finished(TurnResult)
}

public enum ChatBlock: Sendable, Equatable {
    case text(String)
    case thinking(String)
    /// Вызов инструмента: его номер, имя и вход, как его прислала модель.
    case toolUse(id: String, name: String, input: String)
}

/// Просьба инструмента о разрешении.
public struct PermissionRequest: Sendable, Equatable, Identifiable {
    /// Номер запроса: ответ называет его.
    public let id: String
    public let tool: String
    /// Вход инструмента — JSON, как его прислал CLI. Ответ «можно» возвращает его без правок.
    public let input: String
    public let toolUseID: String?
}

/// Чем кончился ход.
public struct TurnResult: Sendable, Equatable {
    public let isError: Bool
    public let session: String?
    public let costUSD: Double?
    public let durationMs: Int?
    public let text: String?
}

extension ChatEvent {

    /// События одной строки потока; строка, в которой окну нечего показать, не даёт ни одного.
    public static func parse(_ line: Data) -> [ChatEvent] {
        guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              let type = object["type"] as? String
        else { return [] }
        switch type {
        case "system" where object["subtype"] as? String == "init":
            return [.started(session: object["session_id"] as? String ?? "",
                             directory: object["cwd"] as? String ?? "",
                             model: object["model"] as? String)]
        case "stream_event":
            let delta = (object["event"] as? [String: Any])?["delta"] as? [String: Any]
            switch delta?["type"] as? String {
            case "text_delta": return (delta?["text"] as? String).map { [.text($0)] } ?? []
            case "thinking_delta": return (delta?["thinking"] as? String).map { [.thinking($0)] } ?? []
            default: return []
            }
        case "assistant":
            let content = (object["message"] as? [String: Any])?["content"] as? [[String: Any]] ?? []
            let blocks = content.compactMap(block)
            return blocks.isEmpty ? [] : [.message(blocks)]
        case "user":
            let content = (object["message"] as? [String: Any])?["content"] as? [[String: Any]] ?? []
            return content.compactMap { item in
                guard item["type"] as? String == "tool_result" else { return nil }
                return .toolResult(id: item["tool_use_id"] as? String ?? "",
                                   text: text(of: item["content"]),
                                   isError: item["is_error"] as? Bool ?? false)
            }
        case "control_request":
            let id = object["request_id"] as? String ?? ""
            guard let request = object["request"] as? [String: Any] else { return [] }
            let subtype = request["subtype"] as? String ?? ""
            guard subtype == "can_use_tool" else { return [.unansweredRequest(id: id, subtype: subtype)] }
            return [.permission(PermissionRequest(
                id: id, tool: request["tool_name"] as? String ?? "",
                input: json(request["input"] ?? [String: Any]()),
                toolUseID: request["tool_use_id"] as? String))]
        case "result":
            return [.finished(TurnResult(
                isError: object["is_error"] as? Bool ?? false,
                session: object["session_id"] as? String,
                costUSD: object["total_cost_usd"] as? Double,
                durationMs: object["duration_ms"] as? Int,
                text: object["result"] as? String))]
        default:
            return []
        }
    }

    private static func block(_ item: [String: Any]) -> ChatBlock? {
        switch item["type"] as? String {
        case "text": (item["text"] as? String).map(ChatBlock.text)
        case "thinking": (item["thinking"] as? String).map(ChatBlock.thinking)
        case "tool_use": .toolUse(id: item["id"] as? String ?? "", name: item["name"] as? String ?? "",
                                  input: json(item["input"] ?? [String: Any]()))
        default: nil
        }
    }

    /// Результат инструмента — строка или список кусков текста.
    private static func text(of content: Any?) -> String {
        if let text = content as? String { return text }
        let parts = (content as? [[String: Any]] ?? []).compactMap { $0["text"] as? String }
        return parts.joined(separator: "\n")
    }

    static func json(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes])
        else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }
}
