// Which file an observation is about. The kernel names an open file by device and inode
// number, but it recycles inode numbers: ext4 and overlayfs hand a freed number to the
// next file created, and APFS does not. An identity built from that pair alone therefore
// lets a newly created file inherit the history of a deleted one — the one mistake this
// layer must never make, because splitting one file's history in two is repairable and
// merging two files into one is not.
//
// What tells them apart is the birth time the filesystem records, judged against the
// graph's own earliest sighting of that identity: a file created after we first saw the
// identity cannot be the file we saw. Nothing else is needed, and in particular no stored
// birth time, so no already written identity changes and no migration is required.

/// One file as the filesystem reports it, before the graph has a say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedFileIdentity {
    device: u64,
    inode: u64,
    /// Whole nanoseconds since the Unix epoch. Absent where the filesystem records no
    /// birth time, and then nothing distinguishes a recycled number from the file that
    /// held it.
    born_at_unix_ns: Option<i64>,
}

/// What the graph already holds under one base identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownIdentity {
    /// No file has ever carried it, so nothing can be inherited.
    Unseen,
    /// The earliest moment a location carrying it was first seen.
    FirstSeenAt(i64),
    /// Carried by a file the graph knows, but no location dates it — a database written
    /// before locations existed. The observed file cannot be placed against it at all.
    Undated,
}

impl ObservedFileIdentity {
    pub fn new(device: u64, inode: u64, born_at_unix_ns: Option<i64>) -> Self {
        Self {
            device,
            inode,
            born_at_unix_ns,
        }
    }

    /// Whether the filesystem recorded when this file was created. Without it a file that
    /// took a deleted file's inode number cannot be told from that file, so a scan reports
    /// how many such sources it met rather than leaving the gap unsaid.
    pub fn has_birth_time(&self) -> bool {
        self.born_at_unix_ns.is_some()
    }

    /// A distinct identity per name, for the fakes that stand in for a filesystem. One
    /// helper rather than one per test module, so every fake tree behaves like the same one
    /// device with a file per path, each with a creation time the way a real one has.
    #[cfg(test)]
    pub fn distinct_for_test(name: &str) -> Self {
        let inode = name.bytes().fold(1u64, |seed, byte| {
            seed.wrapping_mul(31).wrapping_add(u64::from(byte))
        });
        Self::new(1, inode, Some(1_000_000_000))
    }

    /// The identity the kernel's pair alone gives. It is what the graph is asked about,
    /// and what an observation keeps when nothing contradicts it.
    pub fn base_id(&self) -> String {
        format!("file:unix:{}:{}", self.device, self.inode)
    }

    /// The identity this observation is published under: a pure function of what the
    /// filesystem reported and what the graph holds, so the same file resolves to the same
    /// identity on every pass, and two files that took the same inode number never to the
    /// same one.
    pub fn resolve(&self, known: KnownIdentity) -> String {
        let base = self.base_id();
        let Some(born_at_unix_ns) = self.born_at_unix_ns else {
            return base;
        };
        if !contradicts(born_at_unix_ns, known) {
            return base;
        }
        // The discriminator is the birth time itself, so the branch is reached again by
        // every later pass instead of being allocated anew each time.
        format!("{base}:born:{born_at_unix_ns}")
    }
}

/// Whether a file born at `born_at_unix_ns` can be the file the graph knows under this
/// identity. Milliseconds are the resolution the graph stores sightings in, so a file born
/// within the same millisecond as the first sighting is accepted: rejecting it would
/// branch every file that a watcher indexes at the moment it is created.
fn contradicts(born_at_unix_ns: i64, known: KnownIdentity) -> bool {
    match known {
        KnownIdentity::Unseen => false,
        KnownIdentity::Undated => true,
        KnownIdentity::FirstSeenAt(first_seen_at_unix_ms) => {
            born_at_unix_ns.div_euclid(1_000_000) > first_seen_at_unix_ms
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KnownIdentity, ObservedFileIdentity};

    const DEVICE: u64 = 16_777_232;
    const INODE: u64 = 4_242;

    fn born(nanoseconds: i64) -> ObservedFileIdentity {
        ObservedFileIdentity::new(DEVICE, INODE, Some(nanoseconds))
    }

    #[test]
    fn an_identity_no_file_ever_carried_is_published_under_the_kernel_pair() {
        let observed = born(1_000_000_000);
        assert_eq!(observed.base_id(), "file:unix:16777232:4242");
        assert_eq!(observed.resolve(KnownIdentity::Unseen), observed.base_id());
    }

    #[test]
    fn a_file_older_than_the_first_sighting_keeps_the_identity_it_had() {
        // Born at 1 000 ms, first seen at 5 000 ms: the file the graph knows.
        let observed = born(1_000_000_000);
        assert_eq!(
            observed.resolve(KnownIdentity::FirstSeenAt(5_000)),
            observed.base_id()
        );
    }

    #[test]
    fn a_file_born_after_the_first_sighting_cannot_be_that_file() {
        // The inode number was freed and handed to a file created at 9 000 ms.
        let observed = born(9_000_000_000);
        assert_eq!(
            observed.resolve(KnownIdentity::FirstSeenAt(5_000)),
            "file:unix:16777232:4242:born:9000000000"
        );
    }

    /// Every pass must reach the same branch, or a file would collect a new identity each
    /// time it is seen.
    #[test]
    fn the_branch_is_the_same_on_every_pass() {
        let observed = born(9_000_000_000);
        let first = observed.resolve(KnownIdentity::FirstSeenAt(5_000));
        let second = observed.resolve(KnownIdentity::FirstSeenAt(5_000));
        assert_eq!(first, second);
        assert_ne!(first, observed.base_id());
    }

    #[test]
    fn two_files_that_took_the_same_inode_number_never_share_an_identity() {
        let first = born(9_000_000_000).resolve(KnownIdentity::FirstSeenAt(5_000));
        let second = born(11_000_000_000).resolve(KnownIdentity::FirstSeenAt(5_000));
        assert_ne!(first, second);
    }

    /// The same pair at several paths, or at a new path after a rename, is one file: the
    /// birth time did not move, so the identity does not either.
    #[test]
    fn a_hard_link_and_a_rename_resolve_to_the_identity_they_already_had() {
        let observed = born(1_000_000_000);
        let known = KnownIdentity::FirstSeenAt(5_000);
        assert_eq!(observed.resolve(known), observed.base_id());
        assert_eq!(observed.resolve(known), observed.base_id());
    }

    /// A file indexed the moment it is created shares the millisecond of its first
    /// sighting; branching there would split the history of every file a watcher sees.
    #[test]
    fn a_file_born_within_the_millisecond_of_its_first_sighting_is_the_same_file() {
        let observed = born(5_000_999_999);
        assert_eq!(
            observed.resolve(KnownIdentity::FirstSeenAt(5_000)),
            observed.base_id()
        );
    }

    #[test]
    fn an_identity_no_sighting_dates_is_never_inherited() {
        let observed = born(9_000_000_000);
        assert_eq!(
            observed.resolve(KnownIdentity::Undated),
            "file:unix:16777232:4242:born:9000000000"
        );
    }

    /// Without a birth time the filesystem offers nothing to tell the two apart, and an
    /// identity invented per pass would be worse than the one that is merely inherited.
    #[test]
    fn an_identity_says_whether_the_filesystem_dated_it() {
        assert!(born(1).has_birth_time());
        assert!(!ObservedFileIdentity::new(DEVICE, INODE, None).has_birth_time());
    }

    #[test]
    fn a_filesystem_without_birth_times_keeps_the_kernel_pair() {
        let observed = ObservedFileIdentity::new(DEVICE, INODE, None);
        for known in [
            KnownIdentity::Unseen,
            KnownIdentity::FirstSeenAt(5_000),
            KnownIdentity::Undated,
        ] {
            assert_eq!(observed.resolve(known), observed.base_id());
        }
    }
}
