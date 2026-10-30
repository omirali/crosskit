// The service's own copy of the program.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Copies `source` to `target` so that `target` is never half written: the copy goes to a
/// file beside it, and a rename puts it in place.
///
/// A rename and not a write over the old file, because the old file may be the program of the
/// running service: replacing its name leaves the process on the inode it started from, while
/// writing into it would change the code under a process that is executing it.
///
/// The copy keeps the time of the build it came from, which is what lets `status` say which
/// build the service runs.
pub fn copy_program(source: &Path, target: &Path) -> io::Result<()> {
    let directory = target
        .parent()
        .ok_or_else(|| io::Error::other("the program's place has no directory"))?;
    fs::create_dir_all(directory)?;
    let staged = directory.join(format!(".crosskit-{}.partial", std::process::id()));
    let copied = copy_staged(source, &staged).and_then(|()| fs::rename(&staged, target));
    if copied.is_err() && staged.exists() {
        // What failed is reported; a partial file left behind would be all that remains of it.
        let _ = fs::remove_file(&staged);
    }
    copied
}

fn copy_staged(source: &Path, staged: &Path) -> io::Result<()> {
    fs::copy(source, staged)?;
    fs::set_permissions(staged, fs::Permissions::from_mode(0o755))?;
    let built = fs::metadata(source)?.modified()?;
    fs::File::options()
        .write(true)
        .open(staged)?
        .set_modified(built)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::copy_program;
    use crate::infrastructure::service::file_stamp;

    #[test]
    fn the_copy_replaces_the_old_one_and_keeps_the_time_of_its_build() {
        let directory = std::env::temp_dir().join(format!("crosskit-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("built");
        fs::write(&source, "new build").unwrap();
        let target = directory.join("bin").join("crosskit");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "old build").unwrap();

        copy_program(&source, &target).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new build");
        assert_eq!(
            file_stamp(&target).unwrap().modified_ms / 1000,
            file_stamp(&source).unwrap().modified_ms / 1000
        );
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(&directory).unwrap();
    }
}
