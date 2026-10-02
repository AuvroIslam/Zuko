// Tamper-evident audit log: %LOCALAPPDATA%\Zuko\audit\audit.jsonl, one sealed
// `zuko_core::audit::Receipt` per line (hash-chained). Also keeps the recent
// activity items in memory for `activity_recent`.
//
// * One writer: every append takes the log's mutex, assigns `seq`, seals the receipt
//   against the previous hash and writes the whole line with a single `write_all`.
// * Continuity across restarts: the first use reads the log's tail to learn the last
//   `seq` and hash, so the chain simply goes on. A last line left half-written by a
//   crash is trimmed (it never had a hash, so nothing verifiable is lost).
// * Rotation: once the file passes 20 MB it is renamed `audit-<ts>.jsonl` and a new
//   chain starts (its first receipt is an `AuditRotate` marker whose summary names the
//   old file and the hash it ended on, so the two logs stay linked; `seq` keeps
//   counting). `verify` checks every file, each as its own chain.
// * Receipts hold masked summaries and vault keys only, never values (the callers
//   mask; the log adds nothing of its own).
// * The ring of the last 500 activity items is seeded from the log's last receipts at
//   startup, so the island's feed is not empty after a restart.
//
// A log that cannot be written never fails the caller: the receipt is returned sealed
// as usual and the problem is reported once in zuko.log until a write succeeds again.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use zuko_core::audit::{self, Receipt, GENESIS};

use crate::events::ActivityItem;

/// Activity items kept for `activity_recent`.
const RING: usize = 500;
/// Rotate once the active file is at least this big.
const ROTATE_AT: u64 = 20 * 1024 * 1024;
/// How much of the log's end is read to resume the chain and seed the ring.
const TAIL_BYTES: u64 = 1 << 20;
/// Event name of the first receipt in a rotated-in log.
const ROTATE_EVENT: &str = "AuditRotate";

pub fn path() -> PathBuf {
    crate::settings::local_dir().join("audit").join("audit.jsonl")
}

struct State {
    next_seq: u64,
    last_hash: String,
    /// Bytes in the active file.
    size: u64,
    /// A write failure has been reported and not yet followed by a success.
    warned: bool,
    /// Renaming the full log failed once; do not retry on every append.
    rotate_failed: bool,
}

pub struct AuditLog {
    file: PathBuf,
    rotate_at: u64,
    state: Mutex<State>,
    ring: Mutex<VecDeque<ActivityItem>>,
    written: AtomicU64,
}

impl AuditLog {
    /// Opens (or starts) the log at `file`: resumes the chain from its tail and seeds
    /// the activity ring.
    pub fn open(file: PathBuf, rotate_at: u64) -> AuditLog {
        let mut state = State { next_seq: 1, last_hash: GENESIS.to_string(), size: 0, warned: false, rotate_failed: false };
        let mut ring = VecDeque::new();
        match read_tail(&file) {
            Ok(Some(tail)) => {
                let tail = repair_tail(&file, tail);
                state.size = tail.file_len;
                let receipts = parse_receipts(&tail.text);
                if let Some(last) = receipts.last() {
                    state.next_seq = last.seq + 1;
                    state.last_hash = last.hash.clone();
                }
                let from = receipts.len().saturating_sub(RING + 1);
                for r in &receipts[from..] {
                    if r.event != ROTATE_EVENT {
                        ring.push_back(item_from_receipt(r));
                    }
                }
                while ring.len() > RING {
                    ring.pop_front();
                }
            }
            Ok(None) => {}
            Err(e) => crate::log::line(format!("audit log unreadable at start ({}): {e}", file.display())),
        }
        AuditLog { file, rotate_at, state: Mutex::new(state), ring: Mutex::new(ring), written: AtomicU64::new(0) }
    }

    /// Seals `receipt` (seq, prev_hash, hash) and appends it to the log. The caller
    /// gets the sealed receipt back even if the write failed.
    pub fn append(&self, mut receipt: Receipt) -> Receipt {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.size > 0 && st.size >= self.rotate_at && !st.rotate_failed {
            self.rotate(&mut st);
        }
        if receipt.ts == 0 {
            receipt.ts = crate::engine::now();
        }
        receipt.seq = st.next_seq;
        receipt.seal(&st.last_hash);
        match self.write(&receipt) {
            Ok(bytes) => {
                st.next_seq += 1;
                st.last_hash = receipt.hash.clone();
                st.size += bytes;
                st.warned = false;
                self.written.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                // The chain state stays where the file is, so the next success still links.
                if !st.warned {
                    crate::log::line(format!("audit log not written ({}): {e}", self.file.display()));
                    st.warned = true;
                }
            }
        }
        receipt
    }

    fn write(&self, receipt: &Receipt) -> std::io::Result<u64> {
        if let Some(dir) = self.file.parent() {
            crate::platform::ensure_private_dir(dir)?;
        }
        let mut line = serde_json::to_string(receipt).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        // One write_all of the whole line: a crash leaves at most one partial last line.
        options.open(&self.file)?.write_all(line.as_bytes())?;
        Ok(line.len() as u64)
    }

    /// Moves the full log aside and begins a new chain with a marker receipt.
    fn rotate(&self, st: &mut State) {
        let ts = crate::engine::now();
        let dir = self.file.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut name = format!("audit-{ts}.jsonl");
        let mut n = 1;
        while dir.join(&name).exists() {
            name = format!("audit-{ts}-{n}.jsonl");
            n += 1;
        }
        if let Err(e) = std::fs::rename(&self.file, dir.join(&name)) {
            crate::log::line(format!("audit log rotation failed: {e}"));
            st.rotate_failed = true;
            return;
        }
        let mut marker = Receipt {
            seq: st.next_seq,
            ts,
            event: ROTATE_EVENT.into(),
            summary: format!(
                "Log rotated: the previous log {name} ended at receipt {} with hash {}",
                st.next_seq.saturating_sub(1),
                st.last_hash
            ),
            verdict: "info".into(),
            tier: "low".into(),
            ..Default::default()
        };
        marker.seal(GENESIS);
        match self.write(&marker) {
            Ok(bytes) => {
                st.next_seq += 1;
                st.last_hash = marker.hash;
                st.size = bytes;
            }
            Err(e) => {
                // The new chain starts with the next real receipt instead.
                crate::log::line(format!("audit rotation marker not written: {e}"));
                st.last_hash = GENESIS.to_string();
                st.size = 0;
            }
        }
    }

    pub fn remember_activity(&self, item: &ActivityItem) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.push_back(item.clone());
        while ring.len() > RING {
            ring.pop_front();
        }
    }

    /// Attaches the local model's explanation to a remembered item (if still in the ring).
    pub fn annotate_activity(&self, id: &str, text: &str) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = ring.iter_mut().rev().find(|i| i.id == id) {
            item.ai_explanation = Some(text.to_string());
        }
    }

    /// Newest first.
    pub fn recent(&self, limit: usize) -> Vec<ActivityItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().rev().take(limit).cloned().collect()
    }

    /// Verifies every log file (rotated ones first, then the active one), each as its
    /// own chain: `Ok(total receipts)` or `Err(file, line and reason of the first break)`.
    pub fn verify(&self) -> Result<u64, String> {
        // Writers wait, so a line in the middle of being written is never seen.
        let _st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut files = rotated_files(&self.file);
        files.push(self.file.clone());
        let mut total = 0;
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let bytes = match std::fs::read(&f) {
                Ok(b) => b,
                Err(_) if !f.exists() => continue,
                Err(e) => return Err(format!("{name}: cannot be read: {e}")),
            };
            let text = String::from_utf8(bytes).map_err(|_| format!("{name}: not valid UTF-8"))?;
            total += audit::verify_chain(text.lines()).map_err(|(line, why)| format!("{name}, line {line}: {why}"))?;
        }
        Ok(total)
    }

    /// Receipts written since launch.
    pub fn count(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
}

// ── Reading the tail ──────────────────────────────────────────────────────────

struct Tail {
    /// Complete lines only, as text.
    text: String,
    file_len: u64,
}

/// The last `TAIL_BYTES` of the file; `None` when there is no file.
fn read_tail(file: &Path) -> std::io::Result<Option<(Vec<u8>, u64, u64)>> {
    let mut f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    Ok(Some((buf, start, len)))
}

/// Makes sure the file ends on a line boundary and returns its complete lines. A last
/// line without a newline is either a sealed receipt that only lost its `\n` (the
/// newline is added) or a half-written one (trimmed away).
fn repair_tail(file: &Path, raw: (Vec<u8>, u64, u64)) -> Tail {
    let (mut buf, start, mut file_len) = raw;
    if start > 0 {
        // The window began mid-line: drop the fragment before the first newline.
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => buf.drain(..=i).for_each(drop),
            None => buf.clear(),
        }
    }
    if !buf.is_empty() && buf.last() != Some(&b'\n') {
        let cut = buf.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
        let partial = String::from_utf8_lossy(&buf[cut..]).to_string();
        let sealed = serde_json::from_str::<Receipt>(&partial).map(|r| r.is_sealed()).unwrap_or(false);
        let fixed = if sealed {
            std::fs::OpenOptions::new().append(true).open(file).and_then(|mut f| f.write_all(b"\n")).is_ok()
        } else {
            let keep = file_len - (buf.len() - cut) as u64;
            std::fs::OpenOptions::new().write(true).open(file).and_then(|f| f.set_len(keep)).map(|_| file_len = keep).is_ok()
        };
        if fixed && sealed {
            file_len += 1;
            buf.push(b'\n');
        } else if fixed {
            buf.truncate(cut);
        }
        crate::log::line(if sealed {
            "audit log: added the missing newline after the last receipt".to_string()
        } else {
            "audit log: trimmed a half-written last line".to_string()
        });
    }
    Tail { text: String::from_utf8_lossy(&buf).to_string(), file_len }
}

fn parse_receipts(text: &str) -> Vec<Receipt> {
    text.lines().filter_map(|l| serde_json::from_str::<Receipt>(l).ok()).collect()
}

/// `audit-<ts>.jsonl` files next to the active log, oldest first.
fn rotated_files(active: &Path) -> Vec<PathBuf> {
    let Some(dir) = active.parent() else { return Vec::new() };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n.starts_with("audit-") && n.ends_with(".jsonl")
        })
        .collect();
    // `audit-<ts>.jsonl`, or `audit-<ts>-<n>.jsonl` for a second rotation in the same second.
    files.sort_by_key(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stem = name.trim_start_matches("audit-").trim_end_matches(".jsonl");
        let (ts, n) = stem.split_once('-').unwrap_or((stem, "0"));
        (ts.parse::<u64>().unwrap_or(0), n.parse::<u64>().unwrap_or(0))
    });
    files
}

fn item_from_receipt(r: &Receipt) -> ActivityItem {
    ActivityItem {
        id: format!("audit-{}", r.seq),
        // Receipt timestamps are unix seconds; tolerate a writer that used milliseconds.
        ts: if r.ts > 100_000_000_000 { r.ts } else { r.ts.saturating_mul(1000) },
        session_id: r.session_id.clone(),
        project: String::new(),
        event: r.event.clone(),
        tool: r.tool.clone(),
        summary: r.summary.clone(),
        verdict: r.verdict.clone(),
        tier: if r.tier.is_empty() { "low".into() } else { r.tier.clone() },
        score: r.score,
        headline: r.summary.clone(),
        rules: r.rules.clone(),
        keys: r.keys.clone(),
        ai_explanation: None,
    }
}

// ── The app's log ─────────────────────────────────────────────────────────────

fn global() -> &'static AuditLog {
    static LOG: OnceLock<AuditLog> = OnceLock::new();
    LOG.get_or_init(|| AuditLog::open(path(), ROTATE_AT))
}

/// Opens the log now (resumes the chain, seeds the activity ring) instead of on the
/// first event.
pub fn init() {
    let _ = global();
}

/// Seals `receipt` (seq, prev_hash, hash) and appends it to the log.
/// Never fails loudly: an unwritable log is reported once in zuko.log.
pub fn append(receipt: Receipt) -> Receipt {
    global().append(receipt)
}

/// Keeps `item` in the in-memory ring used by `activity_recent`.
pub fn remember_activity(item: &ActivityItem) {
    global().remember_activity(item);
}

/// See [`AuditLog::annotate_activity`].
pub fn annotate_activity(id: &str, text: &str) {
    global().annotate_activity(id, text);
}

/// Newest first.
pub fn recent(limit: usize) -> Vec<ActivityItem> {
    global().recent(limit)
}

/// Verifies the whole log: Ok(count) or Err(why).
pub fn verify() -> Result<u64, String> {
    global().verify()
}

/// Number of receipts written since launch.
pub fn count() -> u64 {
    global().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-al-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn receipt(n: u64) -> Receipt {
        Receipt {
            ts: 1_700_000_000 + n,
            session_id: "s1".into(),
            event: "PreToolUse".into(),
            tool: "Bash".into(),
            summary: format!("Run step {n}"),
            verdict: "allow".into(),
            tier: "low".into(),
            keys: vec!["API_KEY_1".into()],
            ..Default::default()
        }
    }

    fn lines(file: &Path) -> Vec<String> {
        std::fs::read_to_string(file).unwrap().lines().map(String::from).collect()
    }

    #[test]
    fn chain_continues_across_restarts() {
        let dir = tmp("restart");
        let file = dir.join("audit").join("audit.jsonl");
        let first = AuditLog::open(file.clone(), ROTATE_AT);
        assert_eq!(first.verify(), Ok(0), "no file yet is an empty, valid log");
        let a = first.append(receipt(1));
        let b = first.append(receipt(2));
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(a.prev_hash, GENESIS);
        assert_eq!(b.prev_hash, a.hash);
        assert_eq!(first.count(), 2);

        // A new process: same file, new object.
        let second = AuditLog::open(file.clone(), ROTATE_AT);
        assert_eq!(second.count(), 0, "count is per launch");
        let c = second.append(receipt(3));
        assert_eq!(c.seq, 3);
        assert_eq!(c.prev_hash, b.hash);
        assert_eq!(second.verify(), Ok(3));
        assert_eq!(audit::verify_chain(lines(&file).iter().map(|s| s.as_str())), Ok(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_reports_the_edited_line() {
        let dir = tmp("tamper");
        let file = dir.join("audit.jsonl");
        let log = AuditLog::open(file.clone(), ROTATE_AT);
        for n in 1..=4 {
            log.append(receipt(n));
        }
        let mut all = lines(&file);
        all[1] = all[1].replace("\"allow\"", "\"deny\"");
        std::fs::write(&file, all.join("\n") + "\n").unwrap();
        let err = log.verify().unwrap_err();
        assert!(err.contains("audit.jsonl, line 2") && err.contains("modified"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_written_last_line_is_trimmed() {
        let dir = tmp("partial");
        let file = dir.join("audit.jsonl");
        let log = AuditLog::open(file.clone(), ROTATE_AT);
        let a = log.append(receipt(1));
        let b = log.append(receipt(2));
        // A crash in the middle of the third line.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("{\"seq\":3,\"ts\":17000000");
        std::fs::write(&file, text).unwrap();

        let log = AuditLog::open(file.clone(), ROTATE_AT);
        let c = log.append(receipt(3));
        assert_eq!((c.seq, c.prev_hash.as_str()), (3, b.hash.as_str()));
        assert_eq!(log.verify(), Ok(3));
        assert_eq!(lines(&file).len(), 3);
        assert_eq!(a.seq, 1);

        // A sealed last receipt that only lost its newline is kept.
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.trim_end_matches('\n')).unwrap();
        let log = AuditLog::open(file.clone(), ROTATE_AT);
        let d = log.append(receipt(4));
        assert_eq!(d.seq, 4);
        assert_eq!(log.verify(), Ok(4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_starts_a_linked_chain() {
        let dir = tmp("rotate");
        let file = dir.join("audit.jsonl");
        // Tiny limit: roughly three receipts per file.
        let log = AuditLog::open(file.clone(), 1500);
        let mut hashes = Vec::new();
        for n in 1..=10 {
            hashes.push(log.append(receipt(n)));
        }
        let rotated = rotated_files(&file);
        assert!(!rotated.is_empty(), "the log should have rotated");
        // Every file verifies on its own and together.
        let active = lines(&file);
        assert!(audit::verify_chain(active.iter().map(|s| s.as_str())).is_ok());
        let first: Receipt = serde_json::from_str(&active[0]).unwrap();
        assert_eq!(first.event, ROTATE_EVENT);
        assert_eq!(first.prev_hash, GENESIS);
        // The marker names the hash the previous file ended on.
        let prev_text = std::fs::read_to_string(rotated.last().unwrap()).unwrap();
        let prev_last: Receipt = serde_json::from_str(prev_text.lines().last().unwrap()).unwrap();
        assert!(first.summary.contains(&prev_last.hash), "{}", first.summary);
        assert!(first.summary.contains(rotated.last().unwrap().file_name().unwrap().to_str().unwrap()));
        // seq keeps counting through the markers, so it never repeats.
        let seqs: Vec<u64> = hashes.iter().map(|r| r.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
        let total = log.verify().unwrap();
        assert_eq!(total, 10 + rotated.len() as u64, "10 receipts plus one marker per rotation");
        // A restart after rotation resumes from the active file.
        let again = AuditLog::open(file.clone(), 1500);
        let r = again.append(receipt(11));
        assert!(r.seq > *seqs.last().unwrap());
        assert!(again.verify().is_ok());
        // Tampering with a rotated file is caught and named.
        let victim = rotated.first().unwrap();
        let text = std::fs::read_to_string(victim).unwrap().replace("Run step", "Run STEP");
        std::fs::write(victim, text).unwrap();
        let err = again.verify().unwrap_err();
        assert!(err.contains(victim.file_name().unwrap().to_str().unwrap()), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ring_is_seeded_from_the_log_and_capped() {
        let dir = tmp("ring");
        let file = dir.join("audit.jsonl");
        let log = AuditLog::open(file.clone(), ROTATE_AT);
        for n in 1..=(RING as u64 + 20) {
            log.append(receipt(n));
        }
        assert!(log.recent(10).is_empty(), "the ring holds activity items, not raw receipts");

        let log = AuditLog::open(file, ROTATE_AT);
        let recent = log.recent(1000);
        assert_eq!(recent.len(), RING);
        assert_eq!(recent[0].summary, format!("Run step {}", RING + 20), "newest first");
        assert_eq!(recent[0].ts, (1_700_000_000 + RING as u64 + 20) * 1000);
        assert_eq!(recent[0].keys, vec!["API_KEY_1".to_string()]);
        assert_eq!(log.recent(3).len(), 3);
        // New items go on top, and the ring stays capped.
        log.remember_activity(&ActivityItem { id: "live-1".into(), summary: "fresh".into(), ..Default::default() });
        assert_eq!(log.recent(1)[0].id, "live-1");
        assert_eq!(log.recent(1000).len(), RING);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_appends_keep_one_chain() {
        let dir = tmp("threads");
        let file = dir.join("audit.jsonl");
        let log = std::sync::Arc::new(AuditLog::open(file, ROTATE_AT));
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let log = log.clone();
                std::thread::spawn(move || {
                    for n in 0..25 {
                        log.append(receipt(t * 100 + n));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(log.count(), 200);
        assert_eq!(log.verify(), Ok(200));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_log_does_not_fail_the_caller() {
        let dir = tmp("unwritable");
        // The "audit directory" is a regular file, so nothing can be created under it.
        let blocker = dir.join("audit");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let log = AuditLog::open(blocker.join("audit.jsonl"), ROTATE_AT);
        let r = log.append(receipt(1));
        assert!(r.is_sealed());
        assert_eq!(log.count(), 0);
        assert_eq!(log.verify(), Ok(0));
        // Once the path works again, the chain starts where the file is.
        std::fs::remove_file(&blocker).unwrap();
        let r2 = log.append(receipt(2));
        assert_eq!((r2.seq, r2.prev_hash.as_str()), (1, GENESIS));
        assert_eq!(log.verify(), Ok(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
