import Foundation

/// Какой ответ ещё нужен, а какой опоздал.
///
/// Запросы уходят по одной букве, ответы приходят не в том порядке: медленный ответ на
/// «инде» может прийти после быстрого на «индекс». Каждый запрос получает номер, и ответ
/// принимается, только если он новее последнего принятого — иначе старая буква затёрла бы
/// новую выдачу.
struct AnswerOrder {

    private(set) var issued = 0
    private(set) var accepted = 0

    /// Номер следующего запроса.
    mutating func issue() -> Int {
        issued += 1
        return issued
    }

    /// Принять ли ответ на запрос `generation`. Принятый становится точкой отсчёта.
    mutating func accept(_ generation: Int) -> Bool {
        guard generation > accepted else { return false }
        accepted = generation
        return true
    }

    /// Последний отправленный запрос ещё без ответа.
    var awaiting: Bool { issued != accepted }
}
