# ADR 0002: начальная схема Evidence Layer

Статус: принято 2026-09-18

## Контекст

Нужен минимальный граф, который уже соблюдает provenance и неизменяемость ревизий,
но не выдаёт смысловые выводы за факты до реализации парсеров и Insight Layer.

## Решение

Миграция `evidence-foundation` создаёт:

- `File(id)` — стабильная логическая идентичность файла;
- `FileVersion(id, file_id, content_hash, created_at_unix_ms)`;
- `FileObservation(id, file_id, file_version_id, observed_at_unix_ms)` — неизменяемый
  факт чтения конкретной версии источника;
- `ObservationOrder(observation_id, observed_at_unix_ms, sequence)` и
  `CURRENT_OBSERVATION` — сохранённый строгий порядок наблюдений и указатель на
  наблюдение, из которого получено current-состояние;
- `ContentChunk` с диапазоном байтов, текстом, hash, языком и полным контрактом
  происхождения (`extractor_*`, `config_hash`, `input_content_hash`);
- `Extractor(id, extractor_id, extractor_version)`;
- связи `HAS_VERSION`, `HAS_OBSERVATION`, `OBSERVES`, `ORDERS`, `CURRENT_OBSERVATION`,
  `CURRENT_VERSION`, `HAS_CHUNK`, `GENERATED_BY`.

Идентификаторы `FileVersion` и `ContentChunk` создаёт будущий индексатор по правилам
из архитектурного документа. Репозиторий не обновляет существующую ревизию или chunk:
повторная запись должна быть идентичной, иначе вся транзакция откатывается. У `File`
может быть много `HAS_VERSION`, но строго одна `CURRENT_VERSION` — прежняя связь
заменяется в той же транзакции.

`FileVersion` описывает контент и потому дедуплицируется по immutable ID. Порядок
current-состояния задаёт отдельный `FileObservation`: retry повторяет его ID и order
key, а новое чтение того же контента создаёт новую observation. Пара
`(observed_at_unix_ms, sequence)` строго упорядочивает события даже внутри одной
миллисекунды. `CURRENT_VERSION` и `CURRENT_OBSERVATION` обновляются атомарно; поэтому
`A(100) → B(200) → A(300) → retry(B(200))` оставляет current на версии `A`.

Semantic-узлы (`Observation`, `Statement`, `Entity`) появятся следующей миграцией
вместе с use case и детерминированными экстракторами, а не как незадействованная схема.

_Поправка 2026-10-05:_ семантические узлы появились миграцией v17 (ADR 0048) — `Entity`,
`EntityMention` и `Fact` вместо `Statement`; `Observation` по-прежнему не построен.

## Последствия

Уже можно безопасно коммитить воспроизводимые версии и chunks. Первая вертикаль читает
только явно переданную директорию и не создаёт `Observation`: это следующая задача
Evidence Layer вместе с детерминированными экстракторами.

## Дополнение 2026-09-18: физический слой `FileLocation` (schema v5)

Миграция `file-locations` (версия 5) создаёт `FileLocation(id, file_id, normalized_path,
display_name, kind, size_bytes, modified_at_unix_ms, first_seen_at_unix_ms,
last_seen_at_unix_ms, status)` и связь `File -[:LOCATED_AT]-> FileLocation` (ONE_MANY).
Backfill не нужен: базы v1–v4 не хранили путей. Существующие `FileVersion`, chunks,
observations и current-state не изменяются.

Идентичность: `location_id = file_id + "@" + normalized_path`. Один и тот же файл по тому
же пути — одна location при любом числе сканов: identity-поля (`file_id`,
`normalized_path`, `kind`) неизменяемы и проверяются при повторной записи, `first_seen`
сохраняется от первого наблюдения, `size`, `modified_at`, `last_seen` (не раньше
сохранённого) и `status = present` обновляются. Другие location в статусе `present` по
тому же `normalized_path` переводятся в `missing` в той же транзакции: на одном пути в
каждый момент один файл. Это уточняет ключ `volume_id + normalized_path` из раздела 4.6:
`file_id` уже содержит идентификатор устройства.

Будущий rename/move: тот же `file_id` по новому пути создаст новую location, прежняя
станет `missing` при reconcile; новый файл на старом пути получит свою location, не
переписывая историю прежней. Изменение identity-полей требует новой миграции и ADR.

`SourceCommit` объединяет `FileLocation` и опциональный `EvidenceCommit`; репозиторий
пишет их одной транзакцией, поэтому ошибка evidence откатывает и location.

## Дополнение 2026-09-18: reconcile runs (schema v6, историческая)

Миграция `reconcile-runs` (версия 6) создала `ReconcileRun(id, sequence, root, status,
started_at_unix_ms, finished_at_unix_ms)` и связь `ReconcileRun -[:OBSERVED]->
FileLocation` (MANY_MANY). Эта модель отмечала sightings рёбрами на каждый run и была
заменена в v7. Применённая миграция остаётся неизменной: таблица `OBSERVED` сохраняется
как есть, но больше не записывается и в решениях не участвует.

## Дополнение 2026-09-18: эпоха наблюдений (schema v7)

Миграция `reconcile-epoch` (версия 7) создаёт singleton `ReconcileEpoch(id = 'current',
sequence)` и добавляет `FileLocation.seen_in_epoch INT64 DEFAULT 0` через `ALTER TABLE
… ADD IF NOT EXISTS`. Эпоха инициализируется значением `MAX(ReconcileRun.sequence)`
существующих run (0, если run не было), поэтому следующий run получает новый номер и
id, а все прежние locations с `seen_in_epoch = 0` ранжируются раньше него. Ladybug
выполняет этот DDL, что подтверждено интеграционными тестами v5 → v7 и v6 → v7: locations,
evidence, current-state и legacy `OBSERVED` сохраняются.

Эпоха — один durable счётчик. `begin_reconcile_run` увеличивает его в транзакции под
общим write-lock и запоминает новое значение как `sequence` run (`id =
reconcile-run:<sequence>`). Каждая запись location любым writer-путём, будь то `index
init` или reconcile, штампует текущую эпоху в `seen_in_epoch` вместе с обновлением
самой location. Объём данных reconcile не растёт с числом запусков: одна строка
`ReconcileRun` на запуск и одно целое на location.

Статусы run: `running` → `completed | incomplete`. Перед любой операцией от имени run
хранилище сверяет переданный run с persisted записью по id, sequence, root и статусу
`running`; несовпадение или закрытый run дают `InvariantViolation`. Reconcile меняет у
существующих узлов только `FileLocation.status`.

## Дополнение 2026-09-18: relocated locations (Этап 2C, без изменения схемы)

Финализация reconcile классифицирует каждую location, переведённую этим run в
`missing`, как relocated, если тот же `File` имеет другую `LOCATED_AT` location со
статусом `present`, `seen_in_epoch >= run.sequence` и путём, видимым этому run (внутри
root и не под `Exclude` его политики). Классификация читается из
существующих `File`, `FileLocation`, `LOCATED_AT`, `status` и `seen_in_epoch`; новых
таблиц, связей и миграций нет. Граф не содержит факта rename: одна и та же `file_id`
возможна и при hard link, а reconcile не наблюдает событие переноса. Результат
возвращается как typed `RetirementReport(marked_missing, relocated)` и живёт только в
отчёте команды.

## Дополнение 2026-09-18: retention закрытых `ReconcileRun` (Этап 2D, без изменения схемы)

`crosskit index runs prune --keep <N> [--apply]` удаляет узлы `ReconcileRun` со статусом
`completed` или `incomplete`, сохраняя для каждого `root` N самых новых из них. Это явная
ручная maintenance-операция: `index reconcile` записи не удаляет и никакой автоматической
очистки нет. Схема не меняется, миграции v6 и v7 остаются как применены.

Run в статусе `running` не удаляется. Run исторической модели v6, у которого есть хотя бы
одна связь `OBSERVED`, не удаляется, и сама связь `OBSERVED` не удаляется: хранилище
выполняет обычный `DELETE`, а не `DETACH DELETE`, поэтому LadybugDB отвергает удаление
узла с рёбрами даже при ошибке отбора, и транзакция откатывается целиком.
`ReconcileEpoch.sequence` не пересчитывается: после очистки следующий `begin_reconcile_run`
продолжает монотонный счётчик, поэтому id и sequence удалённых run никогда не
переиспользуются. `File`, `FileLocation` (включая `status` и `seen_in_epoch`),
`FileVersion`, `FileObservation`, `ContentChunk` и current-state не затрагиваются.
Результат возвращается как typed `RunRetentionReport` со счётчиками и живёт только в
отчёте команды.

## Дополнение 2026-09-19: insight outbox (schema v8)

Миграция `insight-outbox` (версия 8) создаёт `InsightJob(id, source_revision_id,
input_content_hash, pipeline_version, state, created_at_unix_ms)` и связь
`FileVersion -[:REQUESTS_INSIGHT]-> InsightJob` (ONE_MANY), затем в той же транзакции
ставит в очередь job для каждой существующей `FileVersion` keyset-страницами по id.
Evidence, current state, locations и все прежние миграции остаются неизменными. С этого
момента `persist_source` пишет job последним шагом каждого evidence commit; подробности и
инварианты — в ADR 0005.

## Дополнение 2026-09-21: дата идентичности файла (schema v9, Этап 3)

Идентичность файла — `file:unix:<device>:<inode>` — не была идентичностью во времени. ext4 и
overlayfs отдают освободившийся номер inode следующему созданному файлу; APFS не отдаёт.
Поэтому новый файл наследовал `File`-узел удалённого: его ревизии и наблюдения повисали на
чужой идентичности, а ретайр прежней location читался как relocated. Это порча истории.

Миграция `file-first-seen` (версия 9) добавляет `File.first_seen_at_unix_ms INT64 DEFAULT 0`
через `ALTER TABLE … ADD IF NOT EXISTS` и заполняет её из уже сохранённых locations одним
set-based statement:

```text
MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation)
WITH file, MIN(location.first_seen_at_unix_ms) AS earliest
SET file.first_seen_at_unix_ms = earliest;
```

Ни один идентификатор не переписывается, ничего не читается с диска, и данные для заполнения
уже есть в графе. `File` без locations — база схемы v1–v4, где путей ещё не хранили —
остаётся с нулём.

Правило идентичности (domain, чистая функция от наблюдения и того, что держит граф):

| Что говорит граф про `file:unix:<d>:<i>` | Результат |
| --- | --- |
| такого `File` нет | базовая идентичность |
| `first_seen_at_unix_ms` больше или равно времени создания файла | базовая идентичность |
| файл создан позже первого наблюдения идентичности | ветка `file:unix:<d>:<i>:born:<ns>` |
| `File` есть, но ноль — ничто его не датирует | ветка |
| файловая система не даёт времени создания | базовая идентичность |

Дискриминатор — само время создания в наносекундах, поэтому ветка детерминирована: каждый
следующий проход приходит к той же идентичности, а не выделяет новую. Ветвление затрагивает
только столкнувшиеся случаи: у уже известного файла идентификаторы не меняются.

Дату ставит первая location, которая понесла идентичность, и больше не меняет: `MERGE (file)
ON CREATE SET file.first_seen_at_unix_ms = …`. Поэтому переименование и второй hard link —
по-прежнему один `File` с несколькими locations, как и было в Этапе 2C.

Что граф читает при сканировании: `MATCH (file:File {id: $id}) RETURN
file.first_seen_at_unix_ms` — один statement на источник, план `PRIMARY_KEY_SCAN_NODE_TABLE`
без сканов таблиц, зафиксировано capability-тестом. Форма «взять минимум по locations
идентичности» проверена и отвергнута: её план даёт `SCAN_REL_TABLE` и сканы узловых таблиц,
то есть скан таблицы связей на каждый источник.

Известные ограничения:

- Файловая система без времени создания (не Linux и не macOS, либо ФС без btime) оставляет
  прежнее поведение: переиспользование там неразличимо, и это объявлено, а не скрыто.
- Окно в одну миллисекунду: сравнение идёт в миллисекундах, потому что в них хранится
  first_seen. Столкновение, где удаление, создание и первое наблюдение прежнего файла
  укладываются в одну миллисекунду, не различается. Порог выбран строгим (`>`), чтобы файл,
  проиндексированный watcher-ом в миллисекунду своего создания, не ветвился на каждом проходе.
- `File` из базы схемы v1–v4 не датирован ничем и потому не наследуется: такой файл при
  следующей встрече получает новую идентичность. Путей в тех базах не хранилось, и добраться
  до этой истории по пути всё равно было нельзя.
- Идентичность, не зависящая от устройства (рабочее пространство на сервере, внешние
  источники вроде книг), здесь не реализуется. Дорога ей не закрыта: правило — чистая функция
  в domain, которая получает наблюдение и то, что держит граф, и возвращает строку. Источник
  без устройства и inode даст своё наблюдение и своё правило образования базовой
  идентичности; хранение, порт и точка вызова в сканировании не изменятся.
