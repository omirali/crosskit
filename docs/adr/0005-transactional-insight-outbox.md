# ADR 0005: transactional insight outbox

Статус: принято 2026-09-19 (Этап 2F)

## Контекст

Insight Layer будет обогащать сохранённые ревизии асинхронно. Если очередь задач для
обогащения жила бы отдельно от Evidence-транзакции, сбой между записью ревизии и
постановкой задачи терял бы обогащение молча. Архитектурный документ (§7.6) требует
outbox в той же транзакции, что и новая ревизия. Worker, AI-провайдеры, модели, кэш,
claim/retry lifecycle, daemon и IPC ещё не существуют, и вводить их сейчас рано.

## Решение

Каждый успешно сохранённый `EvidenceCommit` создаёт в той же транзакции durable
`InsightJob` — запись «для этой неизменяемой ревизии есть ожидающая задача enrichment».

Domain (`domain/insight_job.rs`):

- `InsightJob { id, source_revision_id, input_content_hash, pipeline_version, state,
  created_at_unix_ms }`;
- `pipeline_version` — константа `insight-outbox-v1`; новая версия pipeline означает новую
  задачу для каждой ревизии, поэтому id включает её: `insight-job:<pipeline>:<FileVersion.id>`;
- job детерминированно выводится из `FileVersion` методом `InsightJob::queued_for`; поля
  берутся только из уже валидированной ревизии, `created_at_unix_ms` равен времени
  создания ревизии. `EvidenceCommit::insight_job()` — единственная точка получения job,
  расчёт id не дублируется в application и infrastructure;
- единственное состояние — `Queued`; claim, retry и результат AI не моделируются;
- job не содержит путь, имя файла, raw event, текст chunk и raw error.

Schema v8 (`insight-outbox`): node table `InsightJob(id PRIMARY KEY, source_revision_id,
input_content_hash, pipeline_version, state, created_at_unix_ms)` и связь
`REQUESTS_INSIGHT(FROM FileVersion TO InsightJob ONE_MANY)`. Миграция в одной транзакции с
marker-записью выполняет idempotent backfill всех существующих `FileVersion`: keyset-страницы
по `FileVersion.id` (500 строк), в памяти только одна страница. Уже существующий job не
пересоздаётся и не сбрасывается; evidence, current state и locations не меняются.
Прежние миграции v1–v7 неизменны.

Infrastructure: `insight_job_store.rs` пишет job после location → version → observation →
chunks → current state в том же `persist_source`. Повторная встреча job (retry, новое
наблюдение той же ревизии) сверяет immutable поля `source_revision_id`,
`input_content_hash`, `pipeline_version`; `state` и `created_at_unix_ms` принадлежат
outbox и не переписываются. Несовпадение — `InvariantViolation`, вся транзакция
откатывается вместе с location, version, observation и chunks.

Application: отдельный read-порт `InsightQueueReader` с одним агрегатом
`queued_insight_jobs`; `KnowledgeGraphRepository` не расширяется. Команда
`crosskit index jobs` печатает только `Queued insight jobs: N`; лишние аргументы дают
exit code 1 до открытия базы.

## Инварианты

1. `FileLocation`, `FileVersion`, observation, chunks, current state и `InsightJob`
   публикуются атомарно.
2. Ошибка outbox откатывает весь source commit.
3. Metadata-only commit не создаёт job.
4. Для одной ревизии и одной версии pipeline существует ровно один job.
5. Повторное наблюдение или retry не дублирует job и не сбрасывает его state/timestamp.
6. Новая ревизия создаёт новый job.
7. Watcher не пишет job напрямую: job появляется только как следствие сохранённого
   evidence через reconcile-проход.

## Что не входит в 2F

Insight worker, AI-провайдеры и модели, provider-specific кэш, claim/lease, retry
lifecycle, состояния помимо `queued`, durable per-path `IndexJob`, daemon и IPC. Per-path
`IndexJob` появится вместе с daemon/IPC отдельным этапом; текущий watcher намеренно
работает как «событие → dirty flag → полный reconcile», и вторая параллельная модель
индексации не вводится.

## Последствия

Ни одна сохранённая ревизия не может остаться без задачи обогащения, а будущий worker
получает очередь, которую можно опрашивать по `state = queued` без чтения путей и текста.
Бэкфил v8 делает очередь полной и для баз, созданных до этого этапа.
