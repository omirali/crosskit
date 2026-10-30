/// Узел карты, на который ссылается реплика беседы: развилка шага или риск.
///
/// Связь в обе стороны — суть окна (макет `design/UXMain.dc.html`): у реплики в чате видно, что
/// из неё стало на карте, а узел карты открывает чат на своей реплике.
public enum MapAnchor: Sendable, Hashable {
    case step(Int)
    case risk(String)
}

extension DecisionTree {

    /// Узлы, которые доказаны этой репликой: шаг, если её цитирует развилка или ветка, и риски,
    /// которые она подняла. Каждый узел — один раз, в порядке карты.
    public func anchors(session: String, reply: Int) -> [MapAnchor] {
        let here: (Quote) -> Bool = { $0.session == session && $0.reply == reply }
        let steps = steps.filter { step in
            step.quotes.contains(where: here) || step.branches.contains { here($0.quote) }
        }
        let risks = risks.filter { risk in !risk.outside && risk.quotes.contains(where: here) }
        return steps.map { .step($0.id) } + risks.map { .risk($0.id) }
    }

    /// Где узел доказан первым: туда чат прокручивается, когда узел выбирают на карте. У шага —
    /// ведущая ветка, у шага без выбора — первая ветка, ждущая решения.
    public func source(of anchor: MapAnchor) -> Quote? {
        switch anchor {
        case .step(let id):
            guard steps.indices.contains(id) else { return nil }
            let step = steps[id]
            return step.lead?.quote ?? step.branches.first { $0.state == .open }?.quote
                ?? step.branches.first?.quote ?? step.quotes.first
        case .risk(let id):
            return risks.first { $0.id == id }?.quotes.first
        }
    }

    /// Шаг, на котором задача ждёт решения владельца: последний, и на нём никто не выбрал.
    public var waitingStep: Step? {
        guard let head, steps[head].lead == nil, steps[head].branches.contains(where: { $0.state == .open })
        else { return nil }
        return steps[head]
    }
}
