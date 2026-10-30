// `crosskit migrate --from-crosscode`: the data directory leaves the product's earlier name.
//
// Asked for, never done on the way to something else: the directory is the owner's whole
// index, and a program that moved it on its own first run would be the program that lost it
// if anything went wrong halfway. Every refusal names what to do, and nothing inside the
// directory — the policy file's paths included — is read or rewritten.
use crate::infrastructure::{EarlierNameMove, MoveRefusal, move_from_earlier_name};

pub(super) const FLAG: &str = "--from-crosscode";

pub(super) fn run() -> Result<(), String> {
    match move_from_earlier_name() {
        Ok(EarlierNameMove::Moved(rename)) => {
            println!("Moved: {}", rename.from.display());
            println!("   to: {}", rename.to.display());
            println!(
                "Every context, graph and index is where it was inside it; nothing was rebuilt. \
                 `crosskit status` shows them."
            );
            Ok(())
        }
        Ok(EarlierNameMove::AlreadyMoved(rename)) => {
            println!(
                "Nothing to move: {} is already the data directory and {} does not exist.",
                rename.to.display(),
                rename.from.display()
            );
            Ok(())
        }
        Ok(EarlierNameMove::NothingToMove(rename)) => {
            println!(
                "Nothing to move: neither {} nor {} exists.",
                rename.from.display(),
                rename.to.display()
            );
            Ok(())
        }
        Err(refusal) => Err(explain(refusal)),
    }
}

fn explain(refusal: MoveRefusal) -> String {
    match refusal {
        MoveRefusal::Directory(error) => error.to_string(),
        MoveRefusal::BothExist(rename) => format!(
            "both {} and {} exist, and which of them holds the data is not this program's \
             guess. Nothing was moved; remove or rename the one that is not wanted and ask again",
            rename.from.display(),
            rename.to.display()
        ),
        MoveRefusal::HeldByWriter { database, pid } => format!(
            "process {pid} holds {} for writing — an `index watch` or a pass of the earlier \
             build. Nothing was moved; stop it and ask again",
            database.display()
        ),
        MoveRefusal::Undetermined { database, reason } => format!(
            "whether anybody is writing {} could not be asked ({reason}), so nothing was moved",
            database.display()
        ),
        MoveRefusal::DifferentVolumes(rename) => format!(
            "{} is on another volume than {}, so moving it would be a copy of everything in it. \
             Nothing was moved or copied; set CROSSKIT_DATA_DIR={} to keep using it where it is",
            rename.from.display(),
            rename.to.display(),
            rename.from.display()
        ),
        MoveRefusal::Failed { rename, error } => format!(
            "{} was not moved to {}: {error}",
            rename.from.display(),
            rename.to.display()
        ),
    }
}
