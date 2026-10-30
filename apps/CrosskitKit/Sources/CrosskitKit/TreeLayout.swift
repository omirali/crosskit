import CoreGraphics

/// Где на холсте стоит каждый узел дерева решений (макет `design/CCDesktop.dc.html`).
///
/// Развилка — точка. Ведущая ветка уходит вбок и вниз, и от неё линия идёт к следующей
/// развилке, поэтому основная линия идёт зигзагом и ветки соседних развилок не сходятся.
/// Остальные ветки — отростки в другую сторону. Открытый риск — полоса слева от линии от шага,
/// где он появился, до HEAD. Холст фиксированной ширины: подписи ветки держатся у своего узла.
public struct TreeLayout: Sendable, Equatable {

    public enum EdgeStyle: Sendable, Equatable {
        /// Основная линия.
        case main
        /// Отклонённая ветка.
        case rejected
        /// Отложенная или брошенная.
        case loose
        /// Ждёт решения.
        case open
    }

    /// Изгиб — от точки вбок и вниз; отрезок — прямо.
    public struct Edge: Sendable, Equatable {
        public let from: CGPoint
        public let to: CGPoint
        public let curved: Bool
        public let style: EdgeStyle
    }

    public enum NodeKind: Sendable, Equatable {
        case start
        case fork(step: Int)
        case branch(step: Int, branch: Int, state: BranchState)
    }

    public struct Node: Sendable, Equatable {
        public let kind: NodeKind
        public let center: CGPoint
        public let diameter: CGFloat
    }

    /// Подпись ветки: прямоугольник рядом с узлом, текст прижат к узлу.
    public struct Label: Sendable, Equatable {
        public let step: Int
        public let branch: Int
        public let frame: CGRect
        public let towardsRight: Bool
    }

    public struct Question: Sendable, Equatable {
        public let step: Int
        public let origin: CGPoint
    }

    public struct Pin: Sendable, Equatable {
        public let risk: String
        public let origin: CGPoint
    }

    public struct Rail: Sendable, Equatable {
        public let risk: String
        public let level: RiskLevel
        public let points: [CGPoint]
    }

    public static let width: CGFloat = 720
    static let firstForkX: CGFloat = 330
    static let stepHeight: CGFloat = 176
    static let leadOffset = CGSize(width: 72, height: 66)
    static let sideOffset: CGFloat = 100
    static let labelWidth: CGFloat = 200

    public let size: CGSize
    public let edges: [Edge]
    public let nodes: [Node]
    public let labels: [Label]
    public let questions: [Question]
    public let pins: [Pin]
    public let rails: [Rail]
    public let startLabel: CGPoint

    /// `showsRejected: false` убирает отклонённые и отложенные ветки, и остаётся чистая линия.
    public init(tree: DecisionTree, showsRejected: Bool) {
        var edges: [Edge] = []
        var nodes: [Node] = []
        var labels: [Label] = []
        var questions: [Question] = []
        var pins: [Pin] = []
        // Точки основной линии; полоса риска идёт по ним от своего шага.
        var spine: [CGPoint] = []
        var railStart: [Int: Int] = [:]

        let start = CGPoint(x: Self.firstForkX - 70, y: 46)
        nodes.append(Node(kind: .start, center: start, diameter: 14))
        var fork = CGPoint(x: Self.firstForkX, y: 132)
        if !tree.steps.isEmpty {
            edges.append(Edge(from: start, to: fork, curved: true, style: .main))
        }

        for step in tree.steps {
            let side: CGFloat = step.id.isMultiple(of: 2) ? 1 : -1
            let isHead = step.id == tree.head
            spine.append(fork)
            nodes.append(Node(kind: .fork(step: step.id), center: fork, diameter: 10))
            questions.append(Question(step: step.id, origin: CGPoint(x: fork.x + 12, y: fork.y - 30)))

            let down = step.lead == nil
                ? CGPoint(x: fork.x, y: fork.y + Self.leadOffset.height)
                : CGPoint(x: fork.x + side * Self.leadOffset.width, y: fork.y + Self.leadOffset.height)
            if let lead = step.lead {
                edges.append(Edge(from: fork, to: down, curved: true, style: .main))
                nodes.append(Node(kind: .branch(step: step.id, branch: lead.id, state: lead.state), center: down, diameter: 22))
                labels.append(Self.label(of: lead, in: step, at: down, diameter: 22, towardsRight: side > 0))
                spine.append(down)
            } else if !isHead {
                edges.append(Edge(from: fork, to: down, curved: false, style: .main))
            }
            railStart[step.id] = spine.count - 1

            let others = step.branches.filter { branch in
                branch.id != step.lead?.id && (showsRejected || (branch.state != .rejected && branch.state != .deferred))
            }
            for (index, branch) in others.enumerated() {
                // С ведущей веткой остальные уходят в другую сторону столбиком; без неё — по
                // обе стороны поровну.
                let away = step.lead != nil ? -side : (index.isMultiple(of: 2) ? -side : side)
                let row = step.lead != nil ? index : index / 2
                let at = CGPoint(x: fork.x + away * Self.sideOffset, y: fork.y + 40 + CGFloat(row) * 50)
                edges.append(Edge(from: fork, to: at, curved: true, style: Self.style(of: branch.state)))
                let diameter: CGFloat = branch.state == .open ? 18 : 16
                nodes.append(Node(kind: .branch(step: step.id, branch: branch.id, state: branch.state), center: at, diameter: diameter))
                labels.append(Self.label(of: branch, in: step, at: at, diameter: diameter, towardsRight: away > 0))
            }

            let onLine = tree.risks.filter { !$0.outside && $0.step == step.id }
            for (index, risk) in onLine.enumerated() {
                let origin = isHead
                    ? CGPoint(x: fork.x - 150, y: fork.y + 112 + CGFloat(index) * 26)
                    : CGPoint(x: down.x + 16, y: down.y + 36 + CGFloat(index) * 26)
                pins.append(Pin(risk: risk.id, origin: origin))
            }

            if !isHead {
                let next = CGPoint(x: down.x, y: fork.y + Self.stepHeight)
                edges.append(Edge(from: down, to: next, curved: false, style: .main))
                fork = next
            }
        }

        let bottom = fork.y + 150
        let rails = tree.risks.filter(\.pullsDown).enumerated().compactMap { index, risk -> Rail? in
            guard let first = railStart[risk.step], first < spine.count else { return nil }
            let dx = -(12 + 6 * CGFloat(index))
            let along = Array(spine[first...]) + [CGPoint(x: spine[spine.count - 1].x, y: bottom - 30)]
            return Rail(risk: risk.id, level: risk.level, points: along.map { CGPoint(x: $0.x + dx, y: $0.y) })
        }

        self.size = CGSize(width: Self.width, height: bottom + 20)
        self.edges = edges
        self.nodes = nodes
        self.labels = labels
        self.questions = questions
        self.pins = pins
        self.rails = rails
        self.startLabel = CGPoint(x: start.x + 14, y: start.y - 22)
    }

    private static func style(of state: BranchState) -> EdgeStyle {
        switch state {
        case .rejected: .rejected
        case .open: .open
        case .chosen, .agentDefault: .main
        case .deferred, .abandoned: .loose
        }
    }

    private static func label(
        of branch: DecisionTree.Branch, in step: DecisionTree.Step, at center: CGPoint, diameter: CGFloat, towardsRight: Bool
    ) -> Label {
        let gap = diameter / 2 + 8
        let x = towardsRight ? center.x + gap : center.x - gap - labelWidth
        return Label(step: step.id, branch: branch.id,
                     frame: CGRect(x: x, y: center.y - 8, width: labelWidth, height: 40), towardsRight: towardsRight)
    }
}
