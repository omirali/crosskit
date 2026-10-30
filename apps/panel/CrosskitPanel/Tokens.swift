import AppKit

/// Токены макета `design/Crosskit.dc.html`, раздел «04 Токены».
/// Здесь только перевод значений макета в AppKit — ни одного решения о поведении окна.
enum Tokens {

    // MARK: Цвет

    /// `--fg` — имена, заголовки.
    static let fg = dynamic(light: fgLight, dark: fgDark)
    /// `--fg2` — фрагменты, 68 % от `--fg`.
    static let fg2 = dynamic(light: fgLight.withAlphaComponent(0.68), dark: fgDark.withAlphaComponent(0.68))
    /// `--fg3` — мета и каталог пути, 48 % / 46 % от `--fg`.
    static let fg3 = dynamic(light: fgLight.withAlphaComponent(0.48), dark: fgDark.withAlphaComponent(0.46))
    /// `--fill2` — чипы, клавиши.
    static let fill2 = dynamic(light: NSColor(white: 0, alpha: 0.07), dark: NSColor(white: 1, alpha: 0.10))
    /// `--mark` — совпавшие слова.
    static let mark = dynamic(light: NSColor(srgbRed: 1, green: 0.769, blue: 0, alpha: 0.36),
                              dark: NSColor(srgbRed: 1, green: 0.800, blue: 0, alpha: 0.30))
    /// `--n-topic` — узел темы; им обведена метка темы в строке.
    static let topicMark = dynamic(light: NSColor(srgbRed: 0.761, green: 0.267, blue: 0.494, alpha: 1),
                                   dark: NSColor(srgbRed: 0.878, green: 0.404, blue: 0.616, alpha: 1))
    /// `--inv` — текст на заливке `--fg`: выбранный фильтр.
    static let inverse = dynamic(light: NSColor.white, dark: NSColor(srgbRed: 0.110, green: 0.110, blue: 0.118, alpha: 1))
    /// `--ok` — «индекс свежий».
    static let ok = dynamic(light: NSColor(srgbRed: 0.118, green: 0.620, blue: 0.282, alpha: 1),
                            dark: NSColor(srgbRed: 0.204, green: 0.780, blue: 0.349, alpha: 1))

    /// `--warn` — «нет на диске», отставший индекс, ошибки.
    static let warn = dynamic(light: NSColor(srgbRed: 0.773, green: 0.416, blue: 0, alpha: 1),
                              dark: NSColor(srgbRed: 1, green: 0.624, blue: 0.039, alpha: 1))

    /// Монограммы инструментов, `--t-claude` / `--t-cursor` / `--t-codex` / `--t-chatgpt`.
    static let toolClaude = dynamic(light: NSColor(srgbRed: 0.769, green: 0.416, blue: 0.271, alpha: 1),
                                    dark: NSColor(srgbRed: 0.851, green: 0.482, blue: 0.333, alpha: 1))
    static let toolCursor = dynamic(light: NSColor(srgbRed: 0.482, green: 0.361, blue: 0.839, alpha: 1),
                                    dark: NSColor(srgbRed: 0.608, green: 0.510, blue: 0.925, alpha: 1))
    static let toolCodex = dynamic(light: NSColor(srgbRed: 0.290, green: 0.302, blue: 0.333, alpha: 1),
                                   dark: NSColor(srgbRed: 0.490, green: 0.510, blue: 0.553, alpha: 1))
    static let toolChatGPT = dynamic(light: NSColor(srgbRed: 0.180, green: 0.604, blue: 0.471, alpha: 1),
                                     dark: NSColor(srgbRed: 0.247, green: 0.714, blue: 0.561, alpha: 1))

    // MARK: Граф (`design/CCGraph.dc.html`)

    /// `--n-conv`, `--n-msg`, `--n-file`, `--n-proj` — узлы графа; тема — `topicMark`.
    static let nodeConversation = dynamic(light: hex(0x3f63d9), dark: hex(0x6d8cf0))
    static let nodeReply = dynamic(light: hex(0x6e7178), dark: hex(0xa2a5ad))
    static let nodeFile = dynamic(light: hex(0x1f8fa3), dark: hex(0x39b3c6))
    static let nodeProject = dynamic(light: hex(0xc98a12), dark: hex(0xe0a531))
    /// `--edge` — линия между узлами, `--fg` · 28 %.
    static let edge = dynamic(light: fgLight.withAlphaComponent(0.28), dark: fgDark.withAlphaComponent(0.28))
    /// `--graph` — подложка холста графа.
    static let graphBackground = dynamic(light: NSColor(white: 1, alpha: 0.45), dark: NSColor(white: 0, alpha: 0.14))
    /// `--bg-solid` — заливка пустых значков поверх линий.
    static let solidBackground = dynamic(light: hex(0xf7f7f9), dark: hex(0x26262a))

    // MARK: Карта знаний (ADR 0048)

    /// Цвета типов сущностей, по порядку словаря `[knowledge] entity_types`. Двенадцать — по
    /// числу типов встроенного словаря; тип дальше двенадцатого берёт цвет по кругу. Подобраны
    /// так, чтобы соседние в словаре различались тоном, а не только светлотой: карта читается
    /// и в тёмной теме, и на снимке без материала окна.
    static let knowledgePalette: [NSColor] = [
        dynamic(light: hex(0x3f63d9), dark: hex(0x6d8cf0)),
        dynamic(light: hex(0xc98a12), dark: hex(0xe0a531)),
        dynamic(light: hex(0x1f8fa3), dark: hex(0x39b3c6)),
        dynamic(light: hex(0xc2447e), dark: hex(0xe0679d)),
        dynamic(light: hex(0x8a5cd6), dark: hex(0xa98bef)),
        dynamic(light: hex(0x2e9a5a), dark: hex(0x4cc27a)),
        dynamic(light: hex(0xd0612d), dark: hex(0xee8452)),
        dynamic(light: hex(0x6e7178), dark: hex(0xa2a5ad)),
        dynamic(light: hex(0x9c6b3c), dark: hex(0xc4935f)),
        dynamic(light: hex(0xb8323a), dark: hex(0xe05a61)),
        dynamic(light: hex(0x5f8a1f), dark: hex(0x8dbb3f)),
        dynamic(light: hex(0x4a6b8a), dark: hex(0x7c9cbf)),
    ]

    /// `--accent` — «следует системному акценту macOS» (раздел «05 Решения»).
    static var accent: NSColor { .controlAccentColor }
    /// `--sel` — выделенная строка, accent · 13 % / 26 %.
    /// Акцент читается в момент отрисовки: он может смениться в системных настройках на живом окне.
    static var selection: NSColor {
        NSColor(name: nil) { appearance in
            let base = NSColor.controlAccentColor.usingColorSpace(.sRGB) ?? .systemBlue
            let alpha = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? 0.26 : 0.13
            return base.withAlphaComponent(alpha)
        }
    }

    /// Макет ссылается на `--sep`, `--sep2` и `--fill`, но их значений не задаёт нигде.
    /// Значения ниже выведены из соседних токенов; это решение спайка, а не макета.
    static let separator = dynamic(light: NSColor(white: 0, alpha: 0.10), dark: NSColor(white: 1, alpha: 0.10))
    static let separatorSoft = dynamic(light: NSColor(white: 0, alpha: 0.12), dark: NSColor(white: 1, alpha: 0.14))
    static let fill = dynamic(light: NSColor(white: 0, alpha: 0.04), dark: NSColor(white: 1, alpha: 0.06))

    /// Обводка окна из раздела «Тени и материал»: `0 0 0 .5px rgba(0,0,0,.16)` / `rgba(255,255,255,.14)`.
    static let windowHairline = dynamic(light: NSColor(white: 0, alpha: 0.16), dark: NSColor(white: 1, alpha: 0.14))

    // MARK: Размер

    /// Ширина окна фиксированная — 760 pt.
    static let windowWidth: CGFloat = 760
    /// Радиус окна — 16 pt.
    static let windowRadius: CGFloat = 16
    static let rowRadius: CGFloat = 10
    static let chipRadius: CGFloat = 8
    static let keyCapRadius: CGFloat = 5

    // MARK: Шрифт

    /// SF Pro — системный шрифт, а не его имитация: `NSFont.systemFont` и есть SF Pro.
    static func text(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
        .systemFont(ofSize: size, weight: weight)
    }

    /// SF Mono — синтаксис запроса и идентификаторы.
    static func mono(_ size: CGFloat) -> NSFont {
        .monospacedSystemFont(ofSize: size, weight: .regular)
    }

    // MARK: Служебное

    private static let fgLight = NSColor(srgbRed: 0.110, green: 0.110, blue: 0.118, alpha: 1)
    private static let fgDark = NSColor(srgbRed: 0.949, green: 0.949, blue: 0.961, alpha: 1)

    private static func hex(_ value: Int) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
                blue: CGFloat(value & 0xff) / 255, alpha: 1)
    }

    /// Цвет, который сам следует системной теме: макет требует «светлая и тёмная следуют системной».
    private static func dynamic(light: NSColor, dark: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
        }
    }
}
