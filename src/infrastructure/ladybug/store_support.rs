// Small helpers shared by every Ladybug store: error context, safe integer conversion and
// the prepare-then-execute pattern of a parameterised Cypher statement.
//
// # How a batch reaches what it writes
//
// One rule, and every store here follows it. Stage 2R established it for chunks and Stage
// 18 finished carrying it through the rest, after measuring what the places that had not
// adopted it cost.
//
// `MERGE` is never used to write a node or a link a batch already knows to be new. The
// planner reads it as a question — does this already exist? — and answers it by scanning:
// `MERGE (file:File {id: row.id})` scans the node table although `id` is the primary key,
// and `MATCH (a {id: ...}), (b {id: ...}) MERGE (a)-[:R]->(b)` scans three node tables and
// the relationship table although both ends were handed to it. Each of those scans grows
// with everything the graph has ever held, so a batch built on `MERGE` costs the whole
// graph rather than the batch, and a pass over an unchanged tree pays it again every time.
//
// So a store first asks what it already has, by primary key, in one set-based lookup. Then
// it writes only what is missing: `CREATE` for the node, and `CREATE` for its links with
// each endpoint reached through a `MATCH` of its own. A row that was already stored gets no
// link statement at all — a node and its links are written by one transaction, and every
// link table here puts the "one" end on the child, so a stored node cannot be missing a
// link or acquire a second one.
//
// The fast path is chosen by reading, never by attempting a write and catching a refusal.
use lbug::{Connection, LogicalType, Value};

use crate::application::RepositoryError;

pub(super) fn storage_error(
    context: &'static str,
    error: impl std::fmt::Display,
) -> RepositoryError {
    let reported = error.to_string();
    if is_out_of_memory(&reported) {
        return RepositoryError::StorageExhausted(context);
    }
    RepositoryError::Storage(format!("{context}: {reported}"))
}

/// The same for the statement that could not even be prepared. The operation is the same
/// one, so an exhausted page cache is still named by it; only the wording of the ordinary
/// storage failure says which half failed.
pub(super) fn prepare_error(
    context: &'static str,
    error: impl std::fmt::Display,
) -> RepositoryError {
    let reported = error.to_string();
    if is_out_of_memory(&reported) {
        return RepositoryError::StorageExhausted(context);
    }
    RepositoryError::Storage(format!("prepare {context}: {reported}"))
}

/// LadybugDB reports an exhausted page cache as a buffer manager exception, and its text
/// is the only signal it gives. The text itself never leaves this module: it names engine
/// internals, so the category is what crosses the boundary.
fn is_out_of_memory(reported: &str) -> bool {
    reported.contains("Buffer manager exception") || reported.contains("buffer pool is full")
}

/// Whether the engine has already ended the transaction on its own. It does that when it
/// runs out of memory, and a `ROLLBACK` afterwards then finds nothing to undo: that is the
/// transaction having been rolled back, not a failure to roll it back.
pub(super) fn no_active_transaction(error: &impl std::fmt::Display) -> bool {
    error.to_string().contains("No active transaction")
}

pub(super) fn current_unix_ms() -> Result<i64, RepositoryError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            RepositoryError::Storage(format!("system clock precedes UNIX epoch: {error}"))
        })?;
    i64::try_from(duration.as_millis())
        .map_err(|_| RepositoryError::Storage("current timestamp exceeds INT64".to_string()))
}

pub(super) fn u64_to_i64(value: u64, field: &str) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| {
        RepositoryError::InvariantViolation(format!("{field} {value} exceeds Ladybug INT64"))
    })
}

/// Runs a parameterised statement whose result set is not needed (MERGE, CREATE, DELETE).
pub(super) fn execute_statement(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| prepare_error(context, error))?;
    connection
        .execute(&mut statement, parameters)
        .map_err(|error| storage_error(context, error))?;
    Ok(())
}

/// Runs a parameterised lookup and returns its first row, if the lookup matched anything.
pub(super) fn fetch_first_row(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<Option<Vec<Value>>, RepositoryError> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| prepare_error(context, error))?;
    let mut result = connection
        .execute(&mut statement, parameters)
        .map_err(|error| storage_error(context, error))?;
    Ok(result.next())
}

/// Runs a parameterised lookup and materializes every row it returned. Callers bound the
/// result with `LIMIT`: a query result borrows the connection, so a page must be read in
/// full before the caller writes through the same connection.
pub(super) fn fetch_rows(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<Vec<Vec<Value>>, RepositoryError> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| prepare_error(context, error))?;
    let result = connection
        .execute(&mut statement, parameters)
        .map_err(|error| storage_error(context, error))?;
    Ok(result.collect())
}

/// Runs an aggregate lookup that returns one non-negative INT64 count.
pub(super) fn fetch_count(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<usize, RepositoryError> {
    let row = fetch_first_row(connection, query, parameters, context)?;
    match row.as_deref() {
        Some([Value::Int64(count)]) => usize::try_from(*count).map_err(|_| {
            RepositoryError::InvariantViolation(format!("{context} is negative: {count}"))
        }),
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "{context} returned invalid values: {values:?}"
        ))),
        None => Err(RepositoryError::InvariantViolation(format!(
            "{context} returned no row"
        ))),
    }
}

/// Wraps struct rows as the list parameter an `UNWIND $rows` statement expects. The child
/// type is taken from the first row, so every row must carry the same fields in the same
/// order. An empty batch is never sent to the database, so the caller checks for it.
pub(super) fn struct_list(rows: Vec<Value>) -> Value {
    let child = rows.first().map_or(LogicalType::Any, LogicalType::from);
    Value::List(child, rows)
}

/// Runs a set-based statement over `rows` and checks that the database processed exactly
/// the rows that were sent. The query text is static and every value travels as a
/// parameter; `query` must end in a `RETURN COUNT(...)` so the check has something to
/// compare. An empty batch is not sent at all.
pub(super) fn execute_rows(
    connection: &Connection<'_>,
    query: &str,
    rows: Vec<Value>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    let expected = rows.len();
    if expected == 0 {
        return Ok(());
    }
    let processed = fetch_count(
        connection,
        query,
        vec![("rows", struct_list(rows))],
        context,
    )?;
    if processed != expected {
        return Err(RepositoryError::InvariantViolation(format!(
            "{context} processed {processed} of {expected} rows"
        )));
    }
    Ok(())
}

/// Runs a set-based statement over `rows` whose effect on other rows cannot be counted,
/// such as retiring whatever else sits at a path.
pub(super) fn execute_rows_unchecked(
    connection: &Connection<'_>,
    query: &str,
    rows: Vec<Value>,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut all = vec![("rows", struct_list(rows))];
    all.extend(parameters);
    execute_statement(connection, query, all, context)
}

/// Reads whatever the database already stores for the given ids, in one statement.
pub(super) fn fetch_rows_for_ids(
    connection: &Connection<'_>,
    query: &str,
    ids: Vec<String>,
    context: &'static str,
) -> Result<Vec<Vec<Value>>, RepositoryError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = Value::List(
        LogicalType::String,
        ids.into_iter().map(Value::String).collect(),
    );
    fetch_rows(connection, query, vec![("ids", ids)], context)
}

/// A string field of a row the database returned.
pub(super) fn text(
    values: &[Value],
    index: usize,
    context: &'static str,
) -> Result<String, RepositoryError> {
    match values.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(RepositoryError::InvariantViolation(format!(
            "{context} returned a row without a string at position {index}"
        ))),
    }
}

/// An integer field of a row the database returned.
pub(super) fn number(
    values: &[Value],
    index: usize,
    context: &'static str,
) -> Result<i64, RepositoryError> {
    match values.get(index) {
        Some(Value::Int64(value)) => Ok(*value),
        _ => Err(RepositoryError::InvariantViolation(format!(
            "{context} returned a row without an integer at position {index}"
        ))),
    }
}

/// Runs a set-based statement over a plain list of identifiers and checks that the
/// database processed exactly the identifiers that were sent. The query text is static and
/// the identifiers travel as one parameter, alongside whatever `parameters` the statement
/// applies to all of them; `query` must end in a `RETURN COUNT(...)`.
pub(super) fn execute_ids(
    connection: &Connection<'_>,
    query: &str,
    ids: Vec<String>,
    parameters: Vec<(&str, Value)>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    let expected = ids.len();
    if expected == 0 {
        return Ok(());
    }
    let ids = Value::List(
        LogicalType::String,
        ids.into_iter().map(Value::String).collect(),
    );
    let mut all = vec![("ids", ids)];
    all.extend(parameters);
    let processed = fetch_count(connection, query, all, context)?;
    if processed != expected {
        return Err(RepositoryError::InvariantViolation(format!(
            "{context} processed {processed} of {expected} rows"
        )));
    }
    Ok(())
}
