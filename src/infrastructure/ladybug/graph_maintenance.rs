// The two things a recovery may do to a context's graph: ask the engine whether it opens,
// and move the logs the engine refuses out of the way.
//
// Neither goes through the database manager. A probe must not migrate, cache a manager or
// create anything: it asks one question and lets go. And a graph that does not open cannot
// be reached through a manager at all, which is the whole situation this exists for.
use std::fs;
use std::path::{Path, PathBuf};

use lbug::{Database, SystemConfig};

use crate::application::{GraphMaintenance, RepositoryError};
use crate::domain::WorkspaceName;

use super::super::utc_stamp::utc_stamp;
use super::engine_tuning::EngineTuning;
use super::manager::{
    checkpoint_log_of, open_failure, refused_log_of, shadow_pages_of, workspace_database_path,
    write_ahead_log_of,
};
use super::store_support::current_unix_ms;

pub struct LadybugGraphMaintenance {
    database_path: PathBuf,
}

impl LadybugGraphMaintenance {
    pub fn for_workspace(workspace: &WorkspaceName) -> Result<Self, RepositoryError> {
        Ok(Self {
            database_path: workspace_database_path(workspace)?,
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Whether a write-ahead log is lying beside the database right now.
    ///
    /// A cheap question with a narrow meaning: a log exists while a writer is working and
    /// after a process that stopped without closing the database, and a clean close leaves
    /// none. It says nothing by itself about whether the database opens — that is what the
    /// engine answers, sometimes by dying — but it is the only condition under which the
    /// engine can do so, which is what makes it worth asking before the risky open.
    ///
    /// An interrupted checkpoint's log counts: the engine replays it on open as it does the
    /// ordinary one, and can die on it the same way.
    pub fn log_is_present(&self) -> bool {
        write_ahead_log_of(&self.database_path).exists()
            || checkpoint_log_of(&self.database_path).exists()
    }

    /// The log beside this context's database that a refusal is about, and the one every
    /// message about this failure names.
    pub fn write_ahead_log(&self) -> PathBuf {
        refused_log_of(&self.database_path)
    }
}

impl GraphMaintenance for LadybugGraphMaintenance {
    /// Opens the database for writing and closes it again.
    ///
    /// A context nobody has indexed yet has no database, and asking the engine about it
    /// would create an empty one: a question that answers itself by changing the answer.
    /// So the file is looked for first, and its absence is the graph opening as much as it
    /// ever will.
    fn probe(&self) -> Result<(), RepositoryError> {
        if !self.database_path.exists() {
            return Ok(());
        }
        Database::new(
            &self.database_path,
            EngineTuning::production().apply(SystemConfig::default().enable_multi_writes(false)),
        )
        .map(|_| ())
        .map_err(|error| open_failure(&self.database_path, &error))
    }

    /// One step at a time: the write-ahead log while there is one, then an interrupted
    /// checkpoint's log together with its shadow pages. The shadow pages go with their log
    /// rather than stay: without it the engine deletes them on the next open, and this
    /// repair deletes nothing.
    fn move_log_aside(&self) -> Result<Vec<PathBuf>, RepositoryError> {
        let ordinary = write_ahead_log_of(&self.database_path);
        let checkpoint = checkpoint_log_of(&self.database_path);
        let shadow = shadow_pages_of(&self.database_path);
        let step = if ordinary.exists() {
            vec![ordinary]
        } else if checkpoint.exists() {
            std::iter::once(checkpoint)
                .chain(Some(shadow).filter(|pages| pages.exists()))
                .collect()
        } else {
            return Ok(Vec::new());
        };
        let at = current_unix_ms()?;
        step.into_iter()
            .map(|file| {
                let aside = aside_name(&file, at);
                fs::rename(&file, &aside).map_err(|error| {
                    RepositoryError::Storage(format!(
                        "move the write-ahead log {} aside: {error}",
                        file.display()
                    ))
                })?;
                Ok(aside)
            })
            .collect()
    }
}

/// Where a log goes when it is moved aside: beside the database it belonged to, under a
/// name carrying the moment of the move.
///
/// The same directory, because a rename within one directory is one atomic step and needs
/// no second filesystem to be writable. The time, because a context can be recovered more
/// than once and the older file must not be overwritten — a recovery that overwrote the
/// previous log would delete data while claiming it never deletes any.
fn aside_name(log: &Path, unix_ms: i64) -> PathBuf {
    let mut name = log.as_os_str().to_os_string();
    name.push(format!(".aside-{}", utc_stamp(unix_ms)));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{LadybugGraphMaintenance, aside_name};
    use crate::application::GraphMaintenance;

    /// The files a writer stopped inside a checkpoint left, as the incident of 2026-10-30
    /// left them (ADR 0060): the ordinary log, the checkpoint's log and its shadow pages.
    #[test]
    fn logs_are_moved_ordinary_first_then_the_checkpoint_with_its_shadow_pages() {
        let directory = std::env::temp_dir().join(format!(
            "crosskit-maintenance-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir_all(&directory).unwrap();
        let database = directory.join("graph-work.lbug");
        for suffix in ["", ".wal", ".wal.checkpoint", ".shadow"] {
            fs::write(directory.join(format!("graph-work.lbug{suffix}")), suffix).unwrap();
        }
        let maintenance = LadybugGraphMaintenance {
            database_path: database.clone(),
        };
        assert!(maintenance.log_is_present());
        assert!(
            maintenance
                .write_ahead_log()
                .ends_with("graph-work.lbug.wal")
        );

        let first = maintenance.move_log_aside().unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].to_string_lossy().contains(".wal.aside-"));
        assert!(
            maintenance
                .write_ahead_log()
                .ends_with("graph-work.lbug.wal.checkpoint"),
            "with the ordinary log gone, the checkpoint's is the one refused"
        );

        let second = maintenance.move_log_aside().unwrap();
        let names: Vec<String> = second
            .iter()
            .map(|moved| moved.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].starts_with("graph-work.lbug.wal.checkpoint.aside-"));
        assert!(names[1].starts_with("graph-work.lbug.shadow.aside-"));
        assert_eq!(
            fs::read_to_string(&second[1]).unwrap(),
            ".shadow",
            "kept, not deleted"
        );

        assert!(!maintenance.log_is_present());
        assert!(maintenance.move_log_aside().unwrap().is_empty());
        assert!(database.exists(), "the database itself never moves");
        fs::remove_dir_all(directory).unwrap();
    }

    /// The moved log keeps the whole of its old name, so what it is and what it belonged
    /// to are readable from the directory listing alone.
    #[test]
    fn a_moved_log_is_named_after_the_one_it_was() {
        assert_eq!(
            aside_name(Path::new("/data/graph-work.lbug.wal"), 1_790_087_400_000),
            PathBuf::from("/data/graph-work.lbug.wal.aside-20260922T143000Z")
        );
    }

    /// Two recoveries in the same second would collide; a second apart do not. The point
    /// of the test is the rule, not the resolution: a name that repeats would overwrite a
    /// log, and this is where that would show.
    #[test]
    fn two_moves_a_second_apart_are_two_names() {
        let log = Path::new("/data/graph-work.lbug.wal");
        assert_ne!(
            aside_name(log, 1_790_087_400_000),
            aside_name(log, 1_790_087_401_000)
        );
    }
}
