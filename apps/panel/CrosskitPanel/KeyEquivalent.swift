import AppKit

/// Разбор ⌘-сочетаний под нелатинской раскладкой.
///
/// `charactersIgnoringModifiers` возвращает букву **текущей** раскладки: под кириллицей
/// у ⌘K это «л», а у ⌘3 — «і». Сравнивать сочетание с ней значит потерять его все, как
/// только человек переключит язык, — и потерять молча.
///
/// При нажатом ⌘ система кладёт в `characters` латинский эквивалент, чтобы сочетания
/// работали в любой раскладке. Проверено на этой машине: ⌘3 даёт `characters = "3"`,
/// `charactersIgnoringModifiers = "і"`; ⌘K — `"k"` и `"л"`.
///
/// Смотреть надо на обе строки: `characters` покрывает нелатинскую раскладку, а
/// `charactersIgnoringModifiers` — случай, когда сочетание набрано с ⇧ или ⌥.
extension NSEvent {

    /// Обозначает ли событие сочетание ⌘ с этим символом.
    func isCommand(_ symbol: String) -> Bool {
        guard modifierFlags.contains(.command) else { return false }
        let lowered = symbol.lowercased()
        return [characters, charactersIgnoringModifiers]
            .compactMap { $0?.lowercased() }
            .contains(lowered)
    }

    /// Цифра сочетания ⌘1…⌘9, если это оно.
    func commandDigit() -> Int? {
        guard modifierFlags.contains(.command) else { return nil }
        for text in [characters, charactersIgnoringModifiers] {
            if let digit = text.flatMap(Int.init), (1...9).contains(digit) { return digit }
        }
        return nil
    }
}
