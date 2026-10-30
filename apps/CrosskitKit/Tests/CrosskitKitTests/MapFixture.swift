import Foundation
@testable import CrosskitKit

/// Карта выдуманной задачи (`payments-api`) в той форме, в которой её отдаёт `task-map`: две
/// беседы, сводка из двух шагов, риск из двух бесед и находка вне задачи.
enum MapFixture {

    static let line = #"""
    {"schema":2,"context":"work","task":"Двойная скидка","question":"decision-map-v4",
     "joined":{"question":"task-synthesis-v1","stale":false,
       "main_line":[
         {"title":"Считать скидку только в заказе","state":"chosen","level":null,
          "sources":[{"session":"aaaa-1","node":"c0.f0","reply":12},{"session":"bbbb-2","node":"c1.f0","reply":3}]},
         {"title":"Как показывать скидку до заказа","state":"open","level":null,
          "sources":[{"session":"bbbb-2","node":"c1.f1","reply":40}]}],
       "risks":[
         {"title":"Частичный возврат не пересчитан","state":"open","level":"critical",
          "sources":[{"session":"aaaa-1","node":"c0.r0","reply":20},{"session":"bbbb-2","node":"c1.r0","reply":9}]},
         {"title":"TODO округления","state":"resolved","level":"info",
          "sources":[{"session":"bbbb-2","node":"c1.r1","reply":45}]}],
       "findings":[
         {"title":"Экспорт отчётов падает","state":"open","level":"high",
          "sources":[{"session":"bbbb-2","node":"c1.r2","reply":30}]}]},
     "conversations":[
       {"session":"aaaa-1","name":"Почему скидка дважды","first_at_unix_ms":1759474800000,"replies":30,"mapped_through":30,
        "forks":[{"question":"Где считать скидку?","reply":12,"owner":true,"quote":"считаем в заказе",
          "branches":[
            {"option":"флаг в заказе","state":"rejected","reason":"ломает возврат","reply":11,"owner":true,"quote":"флаг не надо"},
            {"option":"только в order.rs","state":"chosen","reason":"корзина показывает","reply":12,"owner":true,"quote":"считаем в заказе"}]}],
        "risks":[{"description":"возврат не пересчитан","level":"high","status":"open","in_task":true,"reply":20,"owner":false,"quote":"возврат не трогает скидку"}],
        "kept":5,"dropped":1},
       {"session":"bbbb-2","name":"Скидка: продолжение","first_at_unix_ms":1759734000000,"replies":50,"mapped_through":44,
        "forks":[
          {"question":"Где считать скидку?","reply":3,"quote":"в заказе, как решили",
           "branches":[{"option":"в заказе","state":"chosen","reason":"","reply":3,"quote":"в заказе, как решили"}]},
          {"question":"Как показывать до заказа?","reply":40,"owner":false,"quote":"могу через API",
           "branches":[
             {"option":"через API заказа","state":"open","reason":"ждёт","reply":40,"owner":false,"quote":"могу через API"},
             {"option":"спросить продакта","state":"open","reason":"ждёт","reply":40,"owner":false,"quote":"или спросить"}]}],
        "risks":[
          {"description":"возврат всё ещё не пересчитан","level":"critical","status":"open","in_task":true,"reply":9,"owner":false,"quote":"всё ещё не пересчитан"},
          {"description":"TODO округления","level":"info","status":"resolved","in_task":true,"reply":45,"owner":false,"quote":"TODO убрал"},
          {"description":"экспорт падает","level":"high","status":"open","in_task":false,"reply":30,"owner":false,"quote":"экспорт падает"}],
        "kept":9,"dropped":2}]}
    """#

    static func answer() throws -> TaskMapAnswer {
        try ServiceAnswer.decode(TaskMapAnswer.self, from: Data(line.utf8))
    }
}
