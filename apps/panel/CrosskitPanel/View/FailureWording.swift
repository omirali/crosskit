import Foundation

/// Что панель говорит человеку, когда страницы нет.
///
/// Ядро отказывает категорией, а не текстом (`src/presentation/machine_answer.rs`), и слова
/// здесь — панели: заголовок называет, что случилось, объяснение — что с этим сделать.
/// Пустым списком отказ не показывается никогда: «ничего нет» прочиталось бы как ответ.
extension FeedFailure {

    func wording(context: String) -> (title: String, explanation: String) {
        switch self {
        case .contextNotConfigured(let name):
            return ("Контекст \(name) не настроен", "⌘K — к настроенному контексту.")
        case .core(let category):
            return category.wording(context: context)
        case .coreNotFound:
            return ("Не найден crosskit",
                    "Укажите путь к ядру ключом запуска --core=<путь> или положите crosskit в PATH.")
        case .coreUnreadable:
            return ("Ответ ядра не читается",
                    "Схема ответа не та, под которую написана панель: ядро и панель разных версий. "
                    + "Соберите их из одного дерева.")
        case .coreTimedOut:
            return ("Ядро не ответило за 10 секунд",
                    "Процесс завершён. Если рядом идёт index watch, повторите, когда проход закончится.")
        case .scopeNotAvailable:
            return ("Тем пока нет",
                    "Ядро не ищет по теме: тема только показывается у строки, когда файл разобран. "
                    + "⌘1 — искать во всём.")
        }
    }
}

private extension CoreCategory {

    func wording(context: String) -> (title: String, explanation: String) {
        switch self {
        case .busy:
            return ("Индекс обновляется",
                    "Служба crosskit или index watch записывает проход в контекст \(context), "
                    + "и пока он идёт, граф не открывается на чтение. Панель спросит снова сама.")
        case .notIndexed:
            return ("Контекст \(context) ещё не проиндексирован",
                    "crosskit --context \(context) index init <каталог> напишет его граф.")
        case .unknownContext:
            return ("Контекста \(context) нет в конфигурации", "⌘K — к настроенному контексту.")
        case .noContext:
            return ("Ни одного контекста не настроено", "crosskit setup ~ предложит их.")
        case .contextRequired:
            return ("Контекст не назван", "Настроено несколько контекстов; ⌘K выбирает один.")
        case .configuration:
            return ("Конфигурация не читается", "crosskit doctor скажет, что в ней не так.")
        case .needsMigration:
            return ("Граф написан другой сборкой", "crosskit migrate приведёт контексты к этой.")
        case .needsRecovery:
            return ("Запись в граф оборвалась",
                    "crosskit --context \(context) index recover, затем обычная сверка.")
        case .indexUnreadable:
            return ("Поисковый индекс не читается",
                    "crosskit --context \(context) index rebuild --search напишет его заново.")
        case .unreadableQuery:
            return ("Запрос не читается",
                    "Понимаются слова, \"фраза\", +нужно, -без, AND и OR. Допечатайте его или "
                    + "уберите лишнюю кавычку.")
        case .usage:
            return ("Ядро не приняло запрос",
                    "Так панель спрашивать не должна — это её ошибка, а не ваша.")
        case .notFound:
            return ("В контексте \(context) этого узла нет",
                    "Граф строится от беседы или файла этого контекста. Возможно, файл удалён из "
                    + "графа или его путь скрыт политикой.")
        case .failed:
            return ("Ядро не ответило",
                    "Причину crosskit печатает в stderr; crosskit doctor проверит контекст.")
        }
    }
}
