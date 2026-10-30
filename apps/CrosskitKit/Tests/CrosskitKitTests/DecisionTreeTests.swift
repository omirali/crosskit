import Foundation
import Testing
@testable import CrosskitKit

@Suite("Дерево решений из ответа ядра")
struct DecisionTreeTests {

    @Test("Шаг сводки берёт ветки из развилки, на которую ссылается, и ведущую — по своему состоянию")
    func aJoinedStepReadsItsFork() throws {
        let tree = DecisionTree(map: try MapFixture.answer())

        #expect(tree.joined)
        #expect(tree.steps.map(\.title) == ["Считать скидку только в заказе", "Как показывать скидку до заказа"])
        let first = tree.steps[0]
        #expect(first.question == "Где считать скидку?")
        #expect(first.branches.map(\.state) == [.rejected, .chosen])
        #expect(first.lead?.option == "только в order.rs")
        #expect(first.lead?.quote.owner == true)
        #expect(first.quotes.count == 2, "шаг сведён из двух бесед")
        #expect(tree.steps[1].lead == nil, "на HEAD ничего не выбрано")
        #expect(tree.head == 1)
    }

    @Test("Риск висит на шаге, после которого появился, и знает, в скольких беседах поднимался")
    func aRiskHangsWhereItAppeared() throws {
        let tree = DecisionTree(map: try MapFixture.answer())
        let refund = try #require(tree.risks.first { $0.title == "Частичный возврат не пересчитан" })

        #expect(refund.level == .critical)
        #expect(refund.step == 0)
        #expect(refund.raisedIn == 2)
        #expect(refund.pullsDown)

        let rounding = try #require(tree.risks.first { $0.title == "TODO округления" })
        #expect(rounding.step == 1)
        #expect(!rounding.pullsDown, "закрытый риск полосой не тянется")

        let export = try #require(tree.risks.first { $0.outside })
        #expect(!export.pullsDown, "находка вне задачи на линии не висит")
        #expect(tree.openRisks.map(\.id) == [refund.id])
    }

    @Test("Без сводки шагами становятся развилки бесед по времени")
    func withoutAJoinForksAreSteps() throws {
        let unjoined = try ServiceAnswer.decode(
            TaskMapAnswer.self,
            from: Data(MapFixture.line.replacingOccurrences(of: #""joined":{"#, with: #""ignored":{"#).utf8))
        let tree = DecisionTree(map: unjoined)

        #expect(!tree.joined)
        #expect(tree.steps.map(\.question) == ["Где считать скидку?", "Где считать скидку?", "Как показывать до заказа?"])
        #expect(tree.steps[0].state == .chosen)
        #expect(tree.risks.contains { $0.outside && $0.title == "экспорт падает" })
        let late = try #require(tree.risks.first { $0.title == "TODO округления" })
        #expect(late.step == 2)
    }

    @Test("Ответ ядра, где у узла нет `owner`, читается: реплика считается не владельца")
    func anAnswerWithoutOwnerReads() throws {
        let tree = DecisionTree(map: try MapFixture.answer())
        let quote = try #require(tree.steps[0].quotes.last)
        #expect(quote.session == "bbbb-2")
        #expect(quote.owner == false)
    }
}
