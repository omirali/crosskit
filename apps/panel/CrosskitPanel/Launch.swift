import Foundation

/// Разбор ключей запуска.
///
/// Источник по умолчанию — ядро (`--feed=core`): панель ищет в настоящих контекстах через
/// процесс `crosskit`. Остальные ключи нужны для одного: снять снимок каждого состояния, не
/// подделывая данные. Состояние — это не заранее нарисованный экран, а начальные условия:
/// какой источник, какой запрос, какой фильтр и насколько медленно источник отвечает. Дальше
/// панель ведёт себя обычно. `--state=…` выбирает выдуманный источник, если `--feed` не назван.
struct LaunchOptions {

    /// Готовые наборы начальных условий. Каждое соответствует состоянию макета.
    enum Scene: String {
        case main
        case empty
        case typing
        case noresults
        case openfail
    }

    let feed: ResultFeed
    let context: String
    /// Файл запросов для замера моста, по одному на строку (`--replay=<файл>`).
    let replay: String?
    /// Файл фокусов для замера графа, `<глубина> <фокус>` на строку (`--graph-replay=<файл>`).
    let graphReplay: String?
    /// Граф, открытый сразу после показа (`--graph-focus=<фокус>`). Нужно снимкам: ⇥ на строке
    /// вызывает то же `showGraph`.
    let graphFocus: String?
    /// Оформление вместо системного (`--appearance=dark|light`), чтобы снять обе темы, не
    /// переключая систему.
    let appearance: String?
    /// Карта знаний, открытая сразу после показа (`--knowledge`). Нужно снимкам и замеру: ⌘M
    /// вызывает то же `showKnowledge`.
    let knowledge: Bool
    /// Что выбрать на карте, когда она разложена (`--pick=<id сущности или связи>`): карточку
    /// связи иначе не снять без клика, а кликнуть за человека панель не может.
    let pick: String?
    /// С `--knowledge`: проба жестов карты, затем снимок окна и выход
    /// (`--probe-gestures=<файл>`).
    let probeGestures: String?
    /// Снимок окна в файл, когда граф или карта нарисованы, затем выход (`--snapshot=<файл>`).
    let snapshot: String?
    let meter: BridgeMeter?
    let query: String
    let scope: FeedScope
    /// Попытка открыть всегда заканчивается отказом. Нужно состоянию «ошибка открытия»:
    /// открывает панель сама, и отказ знает тоже она, а не источник выдачи.
    let openAlwaysFails: Bool

    static func fromCommandLine(_ arguments: [String] = CommandLine.arguments) -> LaunchOptions {
        let values = Self.values(of: arguments)
        let named = Scene(rawValue: values["state"] ?? "")
        let scene = named ?? .main
        let replay = values["replay"]
        let graphReplay = values["graph-replay"]
        let meter = (replay ?? graphReplay).map { _ in BridgeMeter() }

        let feed: ResultFeed
        switch values["feed"] ?? (named == nil ? "core" : "invented") {
        case "invented":
            let thinking = values["slow-ms"].flatMap(Int.init) ?? scene.thinkingMilliseconds
            feed = scene == .noresults ? EmptyFeed() : InventedFeed(thinkingMilliseconds: thinking)
        default:
            feed = CoreFeed(corePath: values["core"],
                            environmentPath: ProcessInfo.processInfo.environment["PATH"],
                            meter: meter)
        }

        return LaunchOptions(
            feed: feed,
            context: values["context"] ?? feed.contexts.first ?? "work",
            replay: replay,
            graphReplay: graphReplay,
            graphFocus: values["graph-focus"],
            appearance: values["appearance"],
            knowledge: arguments.contains("--knowledge"),
            pick: values["pick"],
            probeGestures: values["probe-gestures"],
            snapshot: values["snapshot"],
            meter: meter,
            // У ядра состояние без ключа — пустое окно: выдуманный запрос «маркер политика»
            // в чужих данных ничего не значит.
            query: values["query"] ?? (named == nil && feed is CoreFeed ? "" : scene.query),
            scope: scene.scope,
            openAlwaysFails: scene == .openfail || arguments.contains("--fail-open"))
    }

    /// `--ключ=значение`; всё остальное игнорируется.
    private static func values(of arguments: [String]) -> [String: String] {
        var values: [String: String] = [:]
        for argument in arguments where argument.hasPrefix("--") {
            let body = argument.dropFirst(2)
            guard let separator = body.firstIndex(of: "=") else { continue }
            values[String(body[..<separator])] = String(body[body.index(after: separator)...])
        }
        return values
    }
}

private extension LaunchOptions.Scene {

    var query: String {
        switch self {
        case .main: return "маркер политика"
        case .empty: return ""
        // Состояние набора снимается так: панель отвечает на «идемпо», скрипт дописывает
        // букву, и снимок ловит окно, пока следующий ответ в пути.
        case .typing: return "идемпо"
        case .noresults: return "reconcile ledger v3"
        case .openfail: return "webhook ретрай"
        }
    }

    var scope: FeedScope {
        self == .noresults ? .conversations : .everything
    }

    /// Обычный ответ ядра — ~5 мс поверх открытия хранилища; столько источник и «думает».
    var thinkingMilliseconds: Int {
        self == .typing ? 4000 : 6
    }
}
