import AppKit
import Carbon.HIToolbox
import QuartzCore

/// Замер «от нажатия ⌥Space до первого кадра с нарисованной панелью», разложенный
/// на три отрезка — потому что измеримы они разными часами и означают разное:
///
/// * `нажатие → обработчик` — от штампа, который система поставила событию (`GetEventTime`),
///   до входа в обработчик горячей клавиши. Обе величины отсчитываются от загрузки машины,
///   и приложение печатает их расхождение при запуске, чтобы это было видно, а не принято
///   на веру.
/// * `показ` — сколько в обработчике заняла собственно работа: позиционирование окна,
///   `orderFrontRegardless`, `makeKey`, установка первого отклика.
/// * `показ → развёртка` — до ближайшего вызова `CADisplayLink` после этого, то есть до
///   обновления дисплея, на котором панель могла впервые оказаться на экране.
///
/// Момента, когда свет дошёл до глаза, здесь нет: изнутри процесса его не видно вовсе.
/// Поэтому число проверено ещё и снаружи — по данным оконного сервера о том, когда окно
/// стало экранным (`tools/appear.swift` рядом со спайком).
///
/// Отдельно, чтобы не повторять ошибку: первая версия этого замера ждала completion-блок
/// транзакции CoreAnimation и устойчиво давала 264 мс. Это была не отрисовка, а спящий
/// run loop: блок лежал до ближайшего пробуждения. Транзакция как часы кадра не годится.
final class FirstFrameProbe {

    private struct Measurement {
        let ordinal: Int
        let eventTime: EventTime
        let handlerEntered: CFTimeInterval
        var shown: CFTimeInterval?
    }

    private var current: Measurement?
    private var displayLink: CADisplayLink?
    private var count = 0

    /// Расхождение часов событий и часов CoreAnimation. Печатается при запуске: если бы базы
    /// отсчёта не совпадали, первый отрезок был бы бессмыслицей, и это было бы видно сразу.
    static func clockOffsetMilliseconds() -> Double {
        (CACurrentMediaTime() - GetCurrentEventTime()) * 1000
    }

    /// Вызывается первым делом в обработчике горячей клавиши.
    func begin(eventTime: EventTime) {
        count += 1
        current = Measurement(ordinal: count, eventTime: eventTime, handlerEntered: CACurrentMediaTime())
    }

    func trackingShow(in view: NSView, _ show: () -> Void) {
        show()
        guard current != nil else { return }
        current?.shown = CACurrentMediaTime()

        displayLink?.invalidate()
        let link = view.displayLink(target: self, selector: #selector(displayRefreshed))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    @objc private func displayRefreshed() {
        displayLink?.invalidate()
        displayLink = nil
        guard let measurement = current, let shown = measurement.shown else { return }
        current = nil

        let onScreen = CACurrentMediaTime()
        let delivery = (measurement.handlerEntered - measurement.eventTime) * 1000
        let work = (shown - measurement.handlerEntered) * 1000
        let refresh = (onScreen - shown) * 1000
        let total = (onScreen - measurement.eventTime) * 1000
        print(String(format: "вызов %d: нажатие→обработчик %.2f · показ %.2f · показ→развёртка %.2f · "
                             + "всего %.2f мс",
                     measurement.ordinal, delivery, work, refresh, total))
        fflush(stdout)
    }
}
