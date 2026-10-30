use lbug::{Connection, Value};

use crate::application::RepositoryError;

use super::chunk_store;
use super::insight_job_store;
use super::store_support::{current_unix_ms, no_active_transaction, storage_error};

pub const LATEST_SCHEMA_VERSION: u64 = 18;

/// The v8 backfill reads FileVersion rows in keyset pages of this size, so upgrading a
/// large graph never materializes every revision at once.
const INSIGHT_BACKFILL_PAGE_SIZE: usize = 500;

/// The v12 backfill reads chunk text, which is far heavier per row than a revision, so its
/// page is smaller: a page of this size holds at most a few megabytes of text.
pub(super) const CHUNK_SCRIPT_BACKFILL_PAGE_SIZE: usize = 200;

struct Migration {
    version: u64,
    name: &'static str,
    statements: &'static [&'static str],
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "evidence-foundation",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS File(id STRING PRIMARY KEY);",
            "CREATE NODE TABLE IF NOT EXISTS FileVersion(id STRING PRIMARY KEY, file_id STRING, content_hash STRING, created_at_unix_ms INT64);",
            "CREATE NODE TABLE IF NOT EXISTS ContentChunk(id STRING PRIMARY KEY, source_revision_id STRING, ordinal INT64, start_byte INT64, end_byte INT64, text_hash STRING, text STRING, language STRING, extraction_method STRING, extractor_id STRING, extractor_version STRING, config_hash STRING, input_content_hash STRING, created_at_unix_ms INT64);",
            "CREATE NODE TABLE IF NOT EXISTS Extractor(id STRING PRIMARY KEY, extractor_id STRING, extractor_version STRING);",
            "CREATE REL TABLE IF NOT EXISTS HAS_VERSION(FROM File TO FileVersion ONE_MANY);",
            "CREATE REL TABLE IF NOT EXISTS CURRENT_VERSION(FROM File TO FileVersion ONE_ONE);",
            "CREATE REL TABLE IF NOT EXISTS HAS_CHUNK(FROM FileVersion TO ContentChunk ONE_MANY);",
            "CREATE REL TABLE IF NOT EXISTS GENERATED_BY(FROM ContentChunk TO Extractor MANY_ONE);",
        ],
    },
    Migration {
        version: 2,
        name: "file-observations",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS FileObservation(id STRING PRIMARY KEY, file_id STRING, file_version_id STRING, observed_at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS HAS_OBSERVATION(FROM File TO FileObservation ONE_MANY);",
            "CREATE REL TABLE IF NOT EXISTS OBSERVES(FROM FileObservation TO FileVersion MANY_ONE);",
        ],
    },
    Migration {
        version: 3,
        name: "current-observation-ordering",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS ObservationOrder(id STRING PRIMARY KEY, file_id STRING, observed_at_unix_ms INT64, sequence INT64);",
            "CREATE REL TABLE IF NOT EXISTS ORDERS(FROM ObservationOrder TO FileObservation ONE_ONE);",
            "CREATE REL TABLE IF NOT EXISTS CURRENT_OBSERVATION(FROM File TO FileObservation ONE_ONE);",
        ],
    },
    Migration {
        version: 4,
        name: "backfill-current-observations",
        statements: &[],
    },
    Migration {
        version: 5,
        name: "file-locations",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS FileLocation(id STRING PRIMARY KEY, file_id STRING, normalized_path STRING, display_name STRING, kind STRING, size_bytes INT64, modified_at_unix_ms INT64, first_seen_at_unix_ms INT64, last_seen_at_unix_ms INT64, status STRING);",
            "CREATE REL TABLE IF NOT EXISTS LOCATED_AT(FROM File TO FileLocation ONE_MANY);",
        ],
    },
    // Historical: v6 tracked sightings as OBSERVED edges. v7 replaced them with the shared
    // epoch; the OBSERVED table stays as it was applied and is no longer written.
    Migration {
        version: 6,
        name: "reconcile-runs",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS ReconcileRun(id STRING PRIMARY KEY, sequence INT64, root STRING, status STRING, started_at_unix_ms INT64, finished_at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS OBSERVED(FROM ReconcileRun TO FileLocation MANY_MANY);",
        ],
    },
    Migration {
        version: 7,
        name: "reconcile-epoch",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS ReconcileEpoch(id STRING PRIMARY KEY, sequence INT64);",
            "ALTER TABLE FileLocation ADD IF NOT EXISTS seen_in_epoch INT64 DEFAULT 0;",
        ],
    },
    // The transactional insight outbox: one queued job per immutable revision, backfilled
    // for every revision that already exists.
    Migration {
        version: 8,
        name: "insight-outbox",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS InsightJob(id STRING PRIMARY KEY, source_revision_id STRING, input_content_hash STRING, pipeline_version STRING, state STRING, created_at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS REQUESTS_INSIGHT(FROM FileVersion TO InsightJob ONE_MANY);",
        ],
    },
    // Which file an identity means, dated. The kernel recycles inode numbers, so the pair
    // alone is not an identity over time; a file born after the graph first saw the pair
    // cannot be the file the graph means by it. The date is computed from locations the
    // graph already holds, so no identity changes and nothing is read from disk. A File no
    // location dates keeps zero, which is read as "nothing can date it" and never
    // inherited.
    Migration {
        version: 9,
        name: "file-first-seen",
        statements: &[
            "ALTER TABLE File ADD IF NOT EXISTS first_seen_at_unix_ms INT64 DEFAULT 0;",
            "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) WITH file, MIN(location.first_seen_at_unix_ms) AS earliest SET file.first_seen_at_unix_ms = earliest;",
        ],
    },
    // What a worker needs to carry a job from queued to a result and back again: how many
    // attempts it has cost, when it was claimed, when that claim runs out and the category
    // of its last failure. The four columns are added with defaults, so every job written
    // before this migration starts unclaimed, untried and blameless, and nothing already
    // stored — least of all `created_at_unix_ms` — is rewritten.
    //
    // `InsightResult` holds what a finished job produced. Its `ONE_ONE` link is the reason
    // a repeated run cannot store a second result for one job: the database refuses the
    // second edge, so idempotency is a property of the schema rather than of a check
    // somebody has to remember to write.
    Migration {
        version: 10,
        name: "insight-job-lifecycle",
        statements: &[
            "ALTER TABLE InsightJob ADD IF NOT EXISTS attempts INT64 DEFAULT 0;",
            "ALTER TABLE InsightJob ADD IF NOT EXISTS claimed_at_unix_ms INT64 DEFAULT 0;",
            "ALTER TABLE InsightJob ADD IF NOT EXISTS lease_expires_at_unix_ms INT64 DEFAULT 0;",
            "ALTER TABLE InsightJob ADD IF NOT EXISTS last_error_category STRING DEFAULT '';",
            "CREATE NODE TABLE IF NOT EXISTS InsightResult(id STRING PRIMARY KEY, job_id STRING, source_revision_id STRING, input_content_hash STRING, chunk_count INT64, text_bytes INT64, produced_at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS HAS_INSIGHT_RESULT(FROM InsightJob TO InsightResult ONE_ONE);",
        ],
    },
    // What a model concluded, and what asking it cost. A result written before this
    // migration keeps its counts and gains an empty topic with no model and no tokens
    // behind it, which is the honest record of a finding nobody was billed for.
    //
    // `InsightCacheEntry` is the answer to one question — this content, this pipeline, this
    // model, this prompt — and it lives in the context's own database like everything else,
    // so one context can never be served an answer drawn from another's files.
    Migration {
        version: 11,
        name: "insight-findings",
        statements: &[
            "ALTER TABLE InsightResult ADD IF NOT EXISTS produced_by STRING DEFAULT '';",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS model_id STRING DEFAULT '';",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS prompt_version STRING DEFAULT '';",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS topic STRING DEFAULT '';",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS confidence DOUBLE DEFAULT 0.0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS input_tokens INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS output_tokens INT64 DEFAULT 0;",
            "CREATE NODE TABLE IF NOT EXISTS InsightCacheEntry(key STRING PRIMARY KEY, topic STRING, confidence DOUBLE, produced_by STRING, model_id STRING, prompt_version STRING, input_tokens INT64, output_tokens INT64, created_at_unix_ms INT64);",
        ],
    },
    // `ContentChunk.language` said `en` when the text was ASCII and `und` otherwise, and it
    // said it once for a whole file. English source code was `en` by coincidence, Russian
    // prose was `und`, and a file holding both was `und` from end to end. It was a guess
    // stored under the name of a fact, in a graph whose whole point is provenance.
    //
    // `script` replaces it with what is actually computed, per chunk: which of the two
    // scripts this program can normalize occur in that chunk's text. The step lives in
    // `rewrite_chunk_script` because its two parts are ordered and because of what that
    // function records about dropping columns.
    Migration {
        version: 12,
        name: "chunk-script",
        statements: &[],
    },
    // A job's place in the queue is the age of its revision, and the column now says so.
    //
    // `created_at_unix_ms` was written from the `FileVersion` the pass held, not from the
    // one the graph holds. A revision is identified by its content, so re-reading an
    // unchanged file rebuilds it with the clock of that read; a job first queued by a
    // later pass — which is what approving an analysis mask does — was therefore stamped
    // with the age of the approval instead of the age of the revision, and took its turn
    // by when its row happened to be written. ADR 0030.
    //
    // The rename is the rule written where it cannot be missed, and the rewrite puts every
    // stored job back in the order it should have had. One column of one table, from a
    // value the graph already holds: nothing is read from disk and no id changes.
    Migration {
        version: 13,
        name: "insight-job-ordered-by-its-revision",
        statements: &[
            "ALTER TABLE InsightJob RENAME created_at_unix_ms TO revision_observed_at_unix_ms;",
            "MATCH (job:InsightJob) MATCH (version:FileVersion {id: job.source_revision_id}) SET job.revision_observed_at_unix_ms = version.created_at_unix_ms;",
        ],
    },
    // A transcript is a conversation, not a log, and the graph now says so.
    //
    // Three tables and nothing removed. `FileTouch.path` is a string and deliberately not an
    // edge to `File`: Stage 32 measured that 31% of the paths a conversation names lie in no
    // configured context at all, and most of the rest belong to a context other than the one
    // the transcript is indexed in. A context is its own database (ADR 0008), so an edge
    // could only be drawn to a file that happens to share the database and would be silently
    // absent for everyone else. ADR 0036 and ADR 0037.
    //
    // The statement that deletes chunks is the reindexing of `chats` and it deletes nothing
    // else: only the `plain-text` chunks of files whose name says transcript. Their revisions
    // stay, so history, observations and queued analysis are untouched; the next pass reads
    // those files under the conversation profile and publishes chunks with new identities
    // beside the ones that are gone. A context holding no transcripts passes this migration
    // without a row changing.
    Migration {
        version: 14,
        name: "a-conversation-is-evidence",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS Conversation(id STRING PRIMARY KEY, source_revision_id STRING, session_id STRING, name STRING, named_by STRING, first_message_at_unix_ms INT64, last_message_at_unix_ms INT64, message_count INT64, created_at_unix_ms INT64);",
            "CREATE NODE TABLE IF NOT EXISTS Message(id STRING PRIMARY KEY, conversation_id STRING, role STRING, ordinal INT64, at_unix_ms INT64);",
            "CREATE NODE TABLE IF NOT EXISTS FileTouch(id STRING PRIMARY KEY, conversation_id STRING, path STRING, verb STRING, source STRING, at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS EVIDENCED_BY(FROM Conversation TO FileVersion MANY_ONE);",
            "CREATE REL TABLE IF NOT EXISTS HAS_MESSAGE(FROM Conversation TO Message ONE_MANY);",
            "CREATE REL TABLE IF NOT EXISTS TOUCHED_FILE(FROM Conversation TO FileTouch ONE_MANY);",
            "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) WHERE location.normalized_path ENDS WITH '.jsonl' MATCH (file)-[:HAS_VERSION]->(version:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk) WHERE chunk.extractor_id = 'plain-text' DETACH DELETE chunk;",
        ],
    },
    // A pass that did not happen leaves a record too.
    //
    // `ReconcileRun` was written when a walk began, so a failure before that point left
    // nothing anywhere: Stage 34 found eight roots of one context that had never been
    // indexed and could not say, fifteen stages later, what had stopped them. The attempt is
    // now a run in status `failed`.
    //
    // `failure` is a column of its own and not a spelling of `status`, because they are two
    // facts: what the run was when it stopped, and why. Packing the category into `status`
    // would make every reader of a run parse a string, and the one reader that forgot would
    // silently stop recognising `running`.
    //
    // Rows written before this migration get the empty string, which reads as "this run
    // ended for no recorded reason" — true of every run that completed, and true of the ones
    // whose reason was never kept.
    Migration {
        version: 15,
        name: "a-pass-that-did-not-happen",
        statements: &["ALTER TABLE ReconcileRun ADD IF NOT EXISTS failure STRING DEFAULT '';"],
    },
    // What is known about a touch, so that an answer can be strict without the graph being
    // strict. A file an agent opened while looking for something else is a fact and stays
    // written; whether it belongs in a default answer is decided when the answer is made.
    //
    // `naming` is a closed set of three and not a boolean, and `times` starts at zero, which
    // no real touch has. Both say the same thing about a row written before this migration:
    // nobody looked. Defaulting them to "not named" and "once" would report every touch of an
    // older graph as glanced at in passing — a claim nothing measured. A row gains its marks
    // when its revision is published, so a transcript that goes on gets them at the next pass
    // and one that never changes again keeps them until the context is indexed afresh: a
    // revision already stored is not written twice. ADR 0041.
    Migration {
        version: 16,
        name: "a-touch-carries-what-is-known-about-it",
        statements: &[
            "ALTER TABLE FileTouch ADD IF NOT EXISTS naming STRING DEFAULT 'not_computed';",
            "ALTER TABLE FileTouch ADD IF NOT EXISTS times INT64 DEFAULT 0;",
        ],
    },
    // The knowledge layer: entities, the facts between them, and what each fact rests on.
    // ADR 0048.
    //
    // An `Entity` holds its key and nothing else; its name, type and description are what its
    // mentions say, chosen when the map is read, so a new revision can change the name shown
    // without rewriting a node every other revision points at. A `Fact` points at two entities
    // by edge, never by string, and at the `ContentChunk` its quote was found in. Both carry
    // the file and revision they were read from as columns as well, because a new revision of
    // a file replaces its facts and that is asked by file.
    //
    // Additive and computed from nothing: no row is rewritten and nothing is read from disk.
    // An older result gains zeros and `knowledge_checked = false`, which is true of it — no
    // fact was ever asked of it — and a cache entry gains an empty proposal, which is served
    // to no one, because every prompt version that wrote one was retired with this migration.
    Migration {
        version: 17,
        name: "knowledge-facts",
        statements: &[
            "CREATE NODE TABLE IF NOT EXISTS Entity(id STRING PRIMARY KEY, key STRING, created_at_unix_ms INT64);",
            "CREATE NODE TABLE IF NOT EXISTS EntityMention(id STRING PRIMARY KEY, entity_id STRING, file_id STRING, source_revision_id STRING, name STRING, entity_type STRING, description STRING);",
            "CREATE NODE TABLE IF NOT EXISTS Fact(id STRING PRIMARY KEY, file_id STRING, source_revision_id STRING, source_chunk_id STRING, subject_id STRING, predicate STRING, object_id STRING, description STRING, weight INT64, quote STRING, model_id STRING, prompt_version STRING, created_at_unix_ms INT64);",
            "CREATE REL TABLE IF NOT EXISTS MENTIONED_AS(FROM Entity TO EntityMention ONE_MANY);",
            "CREATE REL TABLE IF NOT EXISTS MENTIONED_IN(FROM EntityMention TO FileVersion MANY_ONE);",
            "CREATE REL TABLE IF NOT EXISTS FACT_SUBJECT(FROM Fact TO Entity MANY_ONE);",
            "CREATE REL TABLE IF NOT EXISTS FACT_OBJECT(FROM Fact TO Entity MANY_ONE);",
            "CREATE REL TABLE IF NOT EXISTS STATED_IN(FROM Fact TO ContentChunk MANY_ONE);",
            "ALTER TABLE InsightCacheEntry ADD IF NOT EXISTS cost_micro_usd INT64 DEFAULT 0;",
            "ALTER TABLE InsightCacheEntry ADD IF NOT EXISTS knowledge STRING DEFAULT '';",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS cost_micro_usd INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS knowledge_checked BOOLEAN DEFAULT false;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS facts_kept INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS facts_rejected_by_quote INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS facts_rejected_otherwise INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS entities_kept INT64 DEFAULT 0;",
            "ALTER TABLE InsightResult ADD IF NOT EXISTS answered_from_cache BOOLEAN DEFAULT false;",
        ],
    },
    // A quote has to name both ends of its fact (ADR 0048, «Поправка»). The facts a quote found
    // in its part and naming one end would have passed before are counted apart from the
    // quotes the part does not hold. A result written before this migration says zero, which
    // is true of it: the rule did not exist, and `knowledge recheck` applies it to what is
    // stored without asking the model again.
    Migration {
        version: 18,
        name: "a-quote-names-both-ends",
        statements: &[
            "ALTER TABLE InsightResult ADD IF NOT EXISTS facts_rejected_unnamed INT64 DEFAULT 0;",
        ],
    },
];

pub fn apply(connection: &Connection<'_>) -> Result<u64, RepositoryError> {
    apply_through(connection, LATEST_SCHEMA_VERSION)
}

/// Applies every pending migration up to `target_version`. Tests build legacy databases
/// with an older target; production always migrates to the latest version.
pub(super) fn apply_through(
    connection: &Connection<'_>,
    target_version: u64,
) -> Result<u64, RepositoryError> {
    connection
        .query("CREATE NODE TABLE IF NOT EXISTS SchemaMigration(version INT64 PRIMARY KEY, name STRING, applied_at_unix_ms INT64);")
        .map_err(|error| storage_error("create migration table", error))?;

    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= target_version)
    {
        if !is_applied(connection, migration.version)? {
            apply_migration(connection, migration)?;
        }
    }

    Ok(target_version)
}

/// The schema version a reader found, or a refusal, without writing anything.
///
/// A writer brings a database to `LATEST_SCHEMA_VERSION` by applying what is missing. A
/// reader cannot: every migration is a write, and this opening has none. So it asks what
/// the database records as applied and refuses anything else, because reading a graph
/// through a schema it was not written with is how a query answers wrongly and says
/// nothing about it.
///
/// The refusal carries the two versions and no recipe. Which command repairs this is the
/// command layer's knowledge, and a message that carried both halves would be said twice.
pub(super) fn require_current_schema(connection: &Connection<'_>) -> Result<u64, RepositoryError> {
    let applied = highest_applied_version(connection)?;
    if applied == LATEST_SCHEMA_VERSION {
        return Ok(applied);
    }
    Err(RepositoryError::SchemaNotThisBuild {
        recorded: applied,
        expected: LATEST_SCHEMA_VERSION,
    })
}

/// The highest version the database records as applied, or zero when it records none.
///
/// A database no writer has prepared has no migration table at all, and asking for one is
/// then a query failure rather than an empty answer. Both mean the same thing here — this
/// build has applied nothing to it — and the caller refuses either way, so the failure is
/// not distinguished from the absence.
fn highest_applied_version(connection: &Connection<'_>) -> Result<u64, RepositoryError> {
    let Ok(mut result) = connection.query("MATCH (m:SchemaMigration) RETURN MAX(m.version);")
    else {
        return Ok(0);
    };
    match result.next().as_deref() {
        Some([Value::Int64(version)]) if *version >= 0 => Ok(*version as u64),
        Some([Value::Null(_)]) | None => Ok(0),
        Some(values) => Err(RepositoryError::Storage(format!(
            "schema version lookup returned unexpected values: {values:?}"
        ))),
    }
}

fn is_applied(connection: &Connection<'_>, version: u64) -> Result<bool, RepositoryError> {
    let mut statement = connection
        .prepare("MATCH (migration:SchemaMigration {version: $version}) RETURN COUNT(migration);")
        .map_err(|error| storage_error("prepare migration lookup", error))?;
    let mut result = connection
        .execute(
            &mut statement,
            vec![("version", Value::Int64(version as i64))],
        )
        .map_err(|error| storage_error("look up migration", error))?;
    let row = result.next().ok_or_else(|| {
        RepositoryError::Storage("migration lookup returned no count".to_string())
    })?;

    match row.as_slice() {
        [Value::Int64(count)] => Ok(*count > 0),
        values => Err(RepositoryError::Storage(format!(
            "migration lookup returned unexpected values: {values:?}"
        ))),
    }
}

fn apply_migration(
    connection: &Connection<'_>,
    migration: &Migration,
) -> Result<(), RepositoryError> {
    in_transaction(connection, |connection| {
        for statement in migration.statements {
            connection
                .query(statement)
                .map_err(|error| storage_error("apply schema migration", error))?;
        }

        if migration.version == 4 {
            backfill_current_observations(connection)?;
        }
        if migration.version == 7 {
            initialize_reconcile_epoch(connection)?;
        }
        if migration.version == 8 {
            insight_job_store::backfill_insight_jobs(connection, INSIGHT_BACKFILL_PAGE_SIZE)?;
        }
        if migration.version == 12 {
            rewrite_chunk_script(connection)?;
        }

        let mut mark_applied = connection
            .prepare(
                "CREATE (:SchemaMigration {version: $version, name: $name, applied_at_unix_ms: $applied_at_unix_ms});",
            )
            .map_err(|error| storage_error("prepare migration marker", error))?;
        connection
            .execute(
                &mut mark_applied,
                vec![
                    ("version", Value::Int64(migration.version as i64)),
                    ("name", Value::String(migration.name.to_string())),
                    ("applied_at_unix_ms", Value::Int64(current_unix_ms()?)),
                ],
            )
            .map_err(|error| storage_error("record schema migration", error))?;

        Ok(())
    })
}

/// Renames `language` to `script` and fills it from the text already stored. Nothing is
/// read from disk: every value comes from the chunk it belongs to.
///
/// Rename rather than add-and-drop, and that is not a preference. `ALTER TABLE ContentChunk
/// DROP language` is accepted by the engine and leaves the table unreadable: the next point
/// access to it dies in `ColumnChunk::lookup` on a null pointer, taking the whole process
/// with it — a SIGSEGV, not an error a caller could handle. Measured on lbug 0.20.4 by
/// running the migration both ways. **No migration may drop a column from a table that
/// holds rows** until that is shown to be fixed.
fn rewrite_chunk_script(connection: &Connection<'_>) -> Result<(), RepositoryError> {
    connection
        .query("ALTER TABLE ContentChunk RENAME language TO script;")
        .map_err(|error| storage_error("rename chunk language column", error))?;
    chunk_store::backfill_chunk_scripts(connection, CHUNK_SCRIPT_BACKFILL_PAGE_SIZE)?;
    Ok(())
}

fn backfill_current_observations(connection: &Connection<'_>) -> Result<(), RepositoryError> {
    let mut lookup = connection
        .prepare(
            "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion) RETURN file.id, version.id, version.created_at_unix_ms;",
        )
        .map_err(|error| storage_error("prepare current-state backfill lookup", error))?;
    let mut result = connection
        .execute(&mut lookup, vec![])
        .map_err(|error| storage_error("look up current state for backfill", error))?;
    // Ladybug's query result borrows this connection. Backfill also writes through the
    // same transaction, so rows must be materialized before issuing mutations. A bounded
    // implementation needs keyset pagination rather than interleaving this iterator with
    // writes; keep this explicit until that migration protocol is introduced.
    let mut current_versions = Vec::new();
    for row in result.by_ref() {
        match row.as_slice() {
            [
                Value::String(file_id),
                Value::String(version_id),
                Value::Int64(observed_at),
            ] => {
                current_versions.push((file_id.clone(), version_id.clone(), *observed_at));
            }
            values => {
                return Err(RepositoryError::Storage(format!(
                    "current-state backfill returned invalid values: {values:?}"
                )));
            }
        }
    }
    drop(result);

    for (sequence, (file_id, version_id, observed_at)) in current_versions.into_iter().enumerate() {
        let sequence = i64::try_from(sequence)
            .map_err(|_| RepositoryError::Storage("backfill sequence exceeds INT64".to_string()))?;
        let observation_id = format!("backfill:current:{file_id}:{version_id}");
        execute(
            connection,
            "MERGE (observation:FileObservation {id: $observation_id}) ON CREATE SET observation.file_id = $file_id, observation.file_version_id = $version_id, observation.observed_at_unix_ms = $observed_at;",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MERGE (observation_order:ObservationOrder {id: $observation_id}) ON CREATE SET observation_order.file_id = $file_id, observation_order.observed_at_unix_ms = $observed_at, observation_order.sequence = $sequence;",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MATCH (file:File {id: $file_id}), (observation:FileObservation {id: $observation_id}) MERGE (file)-[:HAS_OBSERVATION]->(observation);",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MATCH (observation:FileObservation {id: $observation_id}), (version:FileVersion {id: $version_id}) MERGE (observation)-[:OBSERVES]->(version);",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MATCH (order_node:ObservationOrder {id: $observation_id}), (observation:FileObservation {id: $observation_id}) MERGE (order_node)-[:ORDERS]->(observation);",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MATCH (file:File {id: $file_id})-[current:CURRENT_OBSERVATION]->(:FileObservation) DELETE current;",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
        execute(
            connection,
            "MATCH (file:File {id: $file_id}), (observation:FileObservation {id: $observation_id}) CREATE (file)-[:CURRENT_OBSERVATION]->(observation);",
            &file_id,
            &version_id,
            &observation_id,
            observed_at,
            sequence,
        )?;
    }
    Ok(())
}

/// The epoch continues from the highest run sequence recorded by the v6 model, so the
/// next run gets a fresh id and every location, all at epoch 0, ranks before it. A row
/// that already exists is left alone, which keeps a retried migration idempotent.
fn initialize_reconcile_epoch(connection: &Connection<'_>) -> Result<(), RepositoryError> {
    let mut lookup = connection
        .prepare("MATCH (run:ReconcileRun) RETURN MAX(run.sequence);")
        .map_err(|error| storage_error("prepare reconcile epoch initialization", error))?;
    let mut result = connection
        .execute(&mut lookup, vec![])
        .map_err(|error| storage_error("look up latest reconcile run", error))?;
    let latest_sequence = match result.next().as_deref() {
        None | Some([Value::Null(_)]) => 0,
        Some([Value::Int64(sequence)]) => *sequence,
        Some(values) => {
            return Err(RepositoryError::Storage(format!(
                "reconcile epoch initialization returned invalid values: {values:?}"
            )));
        }
    };
    drop(result);

    let mut initialize = connection
        .prepare(
            "MERGE (epoch:ReconcileEpoch {id: 'current'}) ON CREATE SET epoch.sequence = $sequence;",
        )
        .map_err(|error| storage_error("prepare reconcile epoch row", error))?;
    connection
        .execute(
            &mut initialize,
            vec![("sequence", Value::Int64(latest_sequence))],
        )
        .map_err(|error| storage_error("initialize reconcile epoch", error))?;
    Ok(())
}

fn execute(
    connection: &Connection<'_>,
    query: &str,
    file_id: &str,
    version_id: &str,
    observation_id: &str,
    observed_at: i64,
    sequence: i64,
) -> Result<(), RepositoryError> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| storage_error("prepare current-state backfill", error))?;
    connection
        .execute(
            &mut statement,
            vec![
                ("file_id", Value::String(file_id.to_string())),
                ("version_id", Value::String(version_id.to_string())),
                ("observation_id", Value::String(observation_id.to_string())),
                ("observed_at", Value::Int64(observed_at)),
                ("sequence", Value::Int64(sequence)),
            ],
        )
        .map_err(|error| storage_error("backfill current state", error))?;
    Ok(())
}

pub fn in_transaction<T>(
    connection: &Connection<'_>,
    operation: impl FnOnce(&Connection<'_>) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    in_transaction_committing(connection, operation, || ())
}

/// The same, with `commit_guard` taken for the commit and released after it: what keeps the
/// readers of this process out of the engine while a commit may move pages (ADR 0050).
pub fn in_transaction_committing<T, G>(
    connection: &Connection<'_>,
    operation: impl FnOnce(&Connection<'_>) -> Result<T, RepositoryError>,
    commit_guard: impl FnOnce() -> G,
) -> Result<T, RepositoryError> {
    connection
        .query("BEGIN TRANSACTION;")
        .map_err(|error| storage_error("begin transaction", error))?;

    match operation(connection) {
        Ok(value) => {
            let _guard = commit_guard();
            connection
                .query("COMMIT;")
                .map(|_| value)
                .map_err(|error| storage_error("commit transaction", error))
        }
        Err(error) => match connection.query("ROLLBACK;") {
            Ok(_) => Err(error),
            // Nothing left to undo: the engine already ended the transaction, which is
            // what it does when it runs out of memory mid-statement.
            Err(rollback_error) if no_active_transaction(&rollback_error) => Err(error),
            Err(_) => Err(RepositoryError::RollbackFailed(Box::new(error))),
        },
    }
}
