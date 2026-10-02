// Tamper-evident audit log: %LOCALAPPDATA%\Zuko\audit\audit.jsonl, one sealed
// `zuko_core::audit::Receipt` per line (hash-chained). Also keeps the recent
// activity items in memory for `activity_recent`.
//
// OWNER: state & features (wave 2). Stub until then: nothing is written.

use std::path::PathBuf;

use zuko_core::audit::Receipt;

use crate::events::ActivityItem;

pub fn path() -> PathBuf {
    crate::platform::local_dir().join("audit").join("audit.jsonl")
}

/// Seals `receipt` (seq, prev_hash, hash) and appends it to the log.
/// Never fails loudly: an unwritable log is reported once in zuko.log.
pub fn append(receipt: Receipt) -> Receipt {
    receipt
}

/// Keeps `item` in the in-memory ring used by `activity_recent`.
pub fn remember_activity(item: &ActivityItem) {
    let _ = item;
}

/// Newest first.
pub fn recent(limit: usize) -> Vec<ActivityItem> {
    let _ = limit;
    Vec::new()
}

/// Verifies the whole log: Ok(count) or Err(why).
pub fn verify() -> Result<u64, String> {
    Ok(0)
}

/// Number of receipts written since launch.
pub fn count() -> u64 {
    0
}
