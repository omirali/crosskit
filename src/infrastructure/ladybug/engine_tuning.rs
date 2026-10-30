// How much memory and how many threads LadybugDB is allowed to use.
//
// The thread pool is left to the engine and always has been. The buffer pool is left to it
// too, with one exception measured in Stage 17: the engine sizes the pool from
// `sysconf(_SC_PHYS_PAGES)`, which is the machine's memory, and a process under a cgroup
// limit may have far less than the machine. The pool then aims at a ceiling it can never
// reach, never evicts a page, and the kernel ends the process. Where such a limit exists,
// Crosskit names a ceiling that fits inside it; where it does not, nothing changes.
//
// A binary built for the engine tuning benchmark compiles in explicit values instead, so a
// measured run can be labelled with what actually ran. Nothing here is read from the
// environment of a running Crosskit: the limit comes from the kernel, and a released
// binary offers no way to set any of this.
use lbug::SystemConfig;

use super::process_memory_limit;

/// What part of an enforced limit the buffer pool may hold.
///
/// The engine keeps 80% of the machine's memory, which is the right share when the machine
/// is the limit: little else of consequence runs on it. Under a container limit the same
/// process must also hold Crosskit's own side, the search index writer, the allocator's
/// slack and every page the kernel charges to it, so the pool takes half and leaves the
/// other half to all of that.
///
/// What this share buys, measured in Stage 17: the scale corpus of ten thousand files, which
/// the kernel killed under a 640 MiB limit, indexes under it whole, and its peak under 1 GiB
/// falls from 653 MiB to 547 MiB. What it does not buy: it is not why a worker of twenty
/// thousand jobs survives that limit — the checkpoint in `insight_worker` is, and the run
/// passes with this ceiling neutralised.
const POOL_SHARE_OF_LIMIT: u64 = 2;

/// Below this a limit is not turned into a pool ceiling at all.
///
/// Stage 2P found that a pool too small for one transaction stops being a memory setting
/// and becomes an undeclared limit on file size: the largest source the policy allows is
/// refused instead of indexed. Re-measured in Stage 17, after the page-wise chunk writing
/// of 2R changed what that transaction costs: the 16 MiB ceiling source is refused at a
/// 64 MiB pool and indexed at 128 MiB. This floor is twice the smallest pool that worked,
/// so a ceiling is only ever named where there is room to spare. Under a limit tighter than
/// twice this, Crosskit asks for nothing and behaves exactly as it did before: a cap that
/// traded a kill for a refusal would be the worse of the two, because a refusal is a file
/// size limit nobody declared.
const SMALLEST_USEFUL_POOL_BYTES: u64 = 256 * 1024 * 1024;

/// The engine limits one process runs with.
pub struct EngineTuning {
    buffer_pool: BufferPool,
    max_threads: Option<u64>,
}

/// How the size of the buffer pool was arrived at. Three ways, because a report that could
/// not tell them apart would let a measured run be labelled with a number it did not use.
enum BufferPool {
    /// The engine sizes it from the machine's memory, which is right when the machine is
    /// the limit.
    Engine,
    /// The engine would size it from memory this process may not have, so a ceiling that
    /// fits the enforced limit is named instead.
    UnderLimit { bytes: u64, limit_bytes: u64 },
    /// Compiled in by the engine tuning benchmark.
    #[cfg(feature = "engine-tuning-benchmark")]
    Compiled(u64),
}

impl BufferPool {
    /// What this process should ask for, from what the kernel says it is allowed.
    fn for_this_process() -> Self {
        let Some(limit_bytes) = process_memory_limit::enforced_bytes() else {
            return Self::Engine;
        };
        let bytes = limit_bytes / POOL_SHARE_OF_LIMIT;
        if bytes < SMALLEST_USEFUL_POOL_BYTES {
            return Self::Engine;
        }
        Self::UnderLimit { bytes, limit_bytes }
    }

    /// The ceiling to name, or nothing when the engine's own sizing stands.
    fn bytes(&self) -> Option<u64> {
        match self {
            Self::Engine => None,
            Self::UnderLimit { bytes, .. } => Some(*bytes),
            #[cfg(feature = "engine-tuning-benchmark")]
            Self::Compiled(bytes) => Some(*bytes),
        }
    }
}

impl EngineTuning {
    /// What every released Crosskit uses: the engine sizes its own thread pool, and its
    /// own buffer pool unless this process runs under a limit the engine cannot see.
    #[cfg(not(feature = "engine-tuning-benchmark"))]
    pub fn production() -> Self {
        Self {
            buffer_pool: BufferPool::for_this_process(),
            max_threads: None,
        }
    }

    /// The same, for a binary built by the engine tuning benchmark: both values come from
    /// the environment of the compiler. A value that is missing leaves that half to what
    /// production does, so an unlabelled run measures production rather than something
    /// else; a value that cannot be used is refused at compile time rather than silently
    /// replaced, because a benchmark that measured something other than it printed would be
    /// worse than no measurement.
    #[cfg(feature = "engine-tuning-benchmark")]
    pub fn production() -> Self {
        Self {
            buffer_pool: match BUFFER_POOL_MIB {
                Some(mib) => BufferPool::Compiled(
                    mib.checked_mul(1024 * 1024)
                        .expect("the buffer pool size in MiB overflows a byte count"),
                ),
                None => BufferPool::for_this_process(),
            },
            max_threads: MAX_THREADS,
        }
    }

    /// Applies whichever halves were named. A half that was not named is left at the
    /// engine's own default, which is what `SystemConfig` already carries.
    pub(super) fn apply(&self, config: SystemConfig) -> SystemConfig {
        let config = match self.buffer_pool.bytes() {
            Some(bytes) => config.buffer_pool_size(bytes),
            None => config,
        };
        match self.max_threads {
            Some(threads) => config.max_num_threads(threads),
            None => config,
        }
    }

    /// What `doctor` prints, in the shape the benchmark parses. `auto` is the truth about a
    /// released binary on an unconstrained machine: the engine sizes the pool and exposes
    /// no way to read back what it chose, so this states what Crosskit asked for. Under an
    /// enforced limit it states the ceiling it asked for and the limit it derived it from,
    /// which is what somebody looking into a killed process needs to see.
    pub fn describe(&self) -> String {
        format!(
            "buffer pool {}, max threads {}",
            match &self.buffer_pool {
                BufferPool::Engine => "auto".to_string(),
                BufferPool::UnderLimit { bytes, limit_bytes } => format!(
                    "auto, capped to {} MiB by the {} MiB limit of this process",
                    mebibytes(*bytes),
                    mebibytes(*limit_bytes)
                ),
                #[cfg(feature = "engine-tuning-benchmark")]
                BufferPool::Compiled(bytes) => format!("{} MiB", mebibytes(*bytes)),
            },
            match self.max_threads {
                Some(threads) => threads.to_string(),
                None => "auto".to_string(),
            }
        )
    }
}

fn mebibytes(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// One half of the configuration as the compiler saw it. Absent means the binary does what
/// a released one does; anything that is not a whole number above zero fails the build, so a
/// binary that could be labelled with a value it did not use never exists. Zero in
/// particular is refused because the engine reads it as "choose for me", which is what
/// absence already says more clearly.
#[cfg(feature = "engine-tuning-benchmark")]
macro_rules! compiled_in {
    ($name:literal) => {
        match option_env!($name) {
            None => None,
            Some(value) => match positive(value) {
                Some(parsed) => Some(parsed),
                None => panic!(concat!(
                    $name,
                    " must be a whole number above zero, or absent to leave the choice to the engine"
                )),
            },
        }
    };
}

#[cfg(feature = "engine-tuning-benchmark")]
const BUFFER_POOL_MIB: Option<u64> = compiled_in!("CROSSKIT_BUFFER_POOL_MIB");

#[cfg(feature = "engine-tuning-benchmark")]
const MAX_THREADS: Option<u64> = compiled_in!("CROSSKIT_MAX_THREADS");

/// A decimal number above zero, or `None` when the text is not one. Evaluated at compile
/// time, where an overflow is a build error rather than a wrapped value.
#[cfg(feature = "engine-tuning-benchmark")]
const fn positive(value: &str) -> Option<u64> {
    let digits = value.as_bytes();
    if digits.is_empty() {
        return None;
    }
    let mut parsed: u64 = 0;
    let mut index = 0;
    while index < digits.len() {
        let digit = digits[index];
        if !digit.is_ascii_digit() {
            return None;
        }
        parsed = parsed * 10 + (digit - b'0') as u64;
        index += 1;
    }
    if parsed == 0 { None } else { Some(parsed) }
}
