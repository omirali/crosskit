/// Почему служба не дала ответа, которым можно пользоваться.
public enum ServiceError: Error, Equatable, Sendable {
    /// Сокета нет или он не отвечает: служба не запущена.
    case notRunning
    /// Служба ответила отказом своего протокола: `not-served`, `overloaded`,
    /// `protocol-mismatch`, `configuration-changed` или `malformed`.
    case refused(String)
    /// Команда, которую служба выполнила, отказала: категория её машинного ответа.
    case commandRefused(category: String)
    /// В книге задач контекста такой задачи нет.
    case noSuchTask(String)
    /// Строка пришла, но это не ответ, который ждали.
    case unreadable
}
