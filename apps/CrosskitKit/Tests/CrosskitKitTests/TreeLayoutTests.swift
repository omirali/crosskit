import CoreGraphics
import Testing
@testable import CrosskitKit

@Suite("Геометрия дерева")
struct TreeLayoutTests {

    private func layout(showsRejected: Bool = true) throws -> (DecisionTree, TreeLayout) {
        let tree = DecisionTree(map: try MapFixture.answer())
        return (tree, TreeLayout(tree: tree, showsRejected: showsRejected))
    }

    private func center(_ layout: TreeLayout, _ kind: TreeLayout.NodeKind) throws -> CGPoint {
        try #require(layout.nodes.first { $0.kind == kind }).center
    }

    @Test("Ведущая ветка продолжает линию: следующая развилка под ней, отклонённая — с другой стороны")
    func theLeadCarriesTheLine() throws {
        let (_, layout) = try layout()
        let firstFork = try center(layout, .fork(step: 0))
        let lead = try center(layout, .branch(step: 0, branch: 1, state: .chosen))
        let rejected = try center(layout, .branch(step: 0, branch: 0, state: .rejected))
        let secondFork = try center(layout, .fork(step: 1))

        #expect(secondFork.x == lead.x)
        #expect(secondFork.y > lead.y)
        #expect((lead.x - firstFork.x) * (rejected.x - firstFork.x) < 0, "отклонённая уходит в другую сторону")
        #expect(layout.edges.contains { $0.from == lead && $0.to == secondFork && $0.style == .main })
    }

    @Test("Без отклонённых веток остаётся чистая линия")
    func rejectedBranchesCanBeHidden() throws {
        let (_, layout) = try layout(showsRejected: false)
        #expect(!layout.nodes.contains { if case .branch(_, _, .rejected) = $0.kind { true } else { false } })
        #expect(!layout.edges.contains { $0.style == .rejected })
    }

    @Test("На HEAD без выбора ветки расходятся по обе стороны, и линия дальше не идёт")
    func theHeadOpensBothWays() throws {
        let (_, layout) = try layout()
        let head = try center(layout, .fork(step: 1))
        let open = layout.nodes.filter { if case .branch(1, _, .open) = $0.kind { true } else { false } }
        #expect(open.count == 2)
        #expect(Set(open.map { $0.center.x > head.x }) == [true, false])
        #expect(!layout.edges.contains { $0.from == head && $0.style == .main })
    }

    @Test("Открытый риск — полоса от своего шага до низа холста; подписи не выходят за холст")
    func anOpenRiskRunsDownToTheHead() throws {
        let (tree, layout) = try layout()
        let rail = try #require(layout.rails.first)
        #expect(layout.rails.count == tree.risks.filter(\.pullsDown).count)
        #expect(rail.level == .critical)
        #expect(try #require(rail.points.last).y > layout.nodes.map(\.center.y).max() ?? 0)
        #expect(layout.pins.count == tree.risks.filter { !$0.outside }.count)
        for label in layout.labels {
            #expect(label.frame.minX >= 0 && label.frame.maxX <= TreeLayout.width, "подпись \(label.step).\(label.branch)")
        }
    }
}
