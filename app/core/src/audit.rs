//! Tamper-evident audit receipts: append-only JSONL, each line sealed with
//! `hash = sha256(prev_hash || canonical_json(receipt without hash))`. The app writes
//! them; [`verify_chain`] checks a whole log. Receipts never contain secret values: the
//! summary is masked by the caller and tool input is recorded only as a SHA-256.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Hash of the empty chain (prev_hash of the first receipt).
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Receipt {
    pub seq: u64,
    pub ts: u64,
    pub session_id: String,
    /// Hook event (`PreToolUse`, `PermissionRequest`, `UserPromptSubmit`, `Gateway`…).
    pub event: String,
    pub tool: String,
    /// Masked human summary.
    pub summary: String,
    /// SHA-256 hex of the canonical tool input (or request body for gateway receipts).
    pub input_sha256: String,
    /// `allow`, `ask`, `deny`, `defer`, `approved`, `denied_by_user`, `masked`.
    pub verdict: String,
    pub tier: String,
    pub score: u8,
    /// Policy rules and invariant ids that fired.
    pub rules: Vec<String>,
    /// Vault keys involved (masked or rehydrated) — keys only, never values.
    pub keys: Vec<String>,
    pub policy_digest: String,
    pub prev_hash: String,
    pub hash: String,
}

impl Receipt {
    /// Sets `prev_hash` and computes `hash`.
    pub fn seal(&mut self, prev_hash: &str) {
        self.prev_hash = prev_hash.to_string();
        self.hash = self.compute_hash();
    }

    /// The hash this receipt should have given its `prev_hash`.
    pub fn compute_hash(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut v {
            o.remove("hash");
        }
        let mut h = Sha256::new();
        h.update(self.prev_hash.as_bytes());
        h.update(canonical_json(&v).as_bytes());
        hex_lower(&h.finalize())
    }

    /// True if `hash` matches the content and `prev_hash`.
    pub fn is_sealed(&self) -> bool {
        !self.hash.is_empty() && self.hash == self.compute_hash()
    }
}

/// Verifies a JSONL log. `Ok(n)` with the number of receipts, or `Err((line_no, why))`
/// at the first broken link (bad JSON, wrong prev_hash, wrong hash, non-increasing seq).
///
/// Line numbers are 1-based and count every line; blank lines (a trailing newline, a
/// line left by an interrupted write being trimmed) are skipped.
pub fn verify_chain<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<u64, (u64, String)> {
    let mut prev = GENESIS.to_string();
    let mut last_seq: Option<u64> = None;
    let mut count = 0u64;
    for (i, raw) in lines.into_iter().enumerate() {
        let line_no = i as u64 + 1;
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        let r: Receipt = serde_json::from_str(line).map_err(|e| (line_no, format!("not a valid receipt: {e}")))?;
        if r.prev_hash != prev {
            return Err((line_no, format!("broken link: prevHash is {} but the previous receipt's hash is {}", short(&r.prev_hash), short(&prev))));
        }
        let want = r.compute_hash();
        if r.hash != want {
            return Err((line_no, format!("receipt was modified: hash is {} but its content hashes to {}", short(&r.hash), short(&want))));
        }
        if let Some(s) = last_seq {
            if r.seq <= s {
                return Err((line_no, format!("sequence went from {s} to {} (must increase)", r.seq)));
            }
        }
        last_seq = Some(r.seq);
        prev = r.hash;
        count += 1;
    }
    Ok(count)
}

fn short(h: &str) -> String {
    if h.len() > 12 {
        format!("{}…", &h[..12])
    } else if h.is_empty() {
        "(empty)".into()
    } else {
        h.to_string()
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// SHA-256 hex of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// Canonical JSON (object keys sorted recursively, no whitespace).
pub fn canonical_json(v: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                write_canonical(&o[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        // Scalars: serde_json's compact form is already canonical (strings escaped the
        // same way every time, numbers as parsed).
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(seq: u64) -> Receipt {
        Receipt {
            seq,
            ts: 1_700_000_000 + seq,
            session_id: "s1".into(),
            event: "PreToolUse".into(),
            tool: "Bash".into(),
            summary: format!("Run step {seq}"),
            input_sha256: sha256_hex(format!("input {seq}").as_bytes()),
            verdict: "allow".into(),
            tier: "low".into(),
            score: 3,
            rules: vec![],
            keys: vec!["API_KEY_1".into()],
            policy_digest: "d".into(),
            ..Default::default()
        }
    }

    fn chain(n: u64) -> Vec<String> {
        let mut prev = GENESIS.to_string();
        (1..=n)
            .map(|i| {
                let mut r = receipt(i);
                r.seal(&prev);
                prev = r.hash.clone();
                serde_json::to_string(&r).unwrap()
            })
            .collect()
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let a = json!({"b": 1, "a": {"z": [3, {"y": true, "x": null}], "c": "é\"q"}});
        assert_eq!(canonical_json(&a), r#"{"a":{"c":"é\"q","z":[3,{"x":null,"y":true}]},"b":1}"#);
        let b: Value = serde_json::from_str(r#"{ "a" : {"z":[3,{"x":null,"y":true}],"c":"é\"q"}, "b":1 }"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn seal_and_verify() {
        let lines = chain(5);
        assert_eq!(verify_chain(lines.iter().map(|s| s.as_str())), Ok(5));
        // Trailing blank line is fine.
        let mut with_blank = lines.clone();
        with_blank.push(String::new());
        assert_eq!(verify_chain(with_blank.iter().map(|s| s.as_str())), Ok(5));
        assert_eq!(verify_chain(std::iter::empty()), Ok(0));
        let r: Receipt = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(r.prev_hash, GENESIS);
        assert!(r.is_sealed());
    }

    #[test]
    fn detects_edited_field() {
        let mut lines = chain(4);
        let mut r: Receipt = serde_json::from_str(&lines[2]).unwrap();
        r.verdict = "deny".into();
        lines[2] = serde_json::to_string(&r).unwrap();
        let err = verify_chain(lines.iter().map(|s| s.as_str())).unwrap_err();
        assert_eq!(err.0, 3);
        assert!(err.1.contains("modified"), "{}", err.1);
    }

    #[test]
    fn detects_resealed_edit_by_broken_next_link() {
        // An attacker who edits a receipt and recomputes its hash breaks the next link.
        let mut lines = chain(4);
        let mut r: Receipt = serde_json::from_str(&lines[1]).unwrap();
        r.summary = "nothing to see".into();
        let prev = r.prev_hash.clone();
        r.seal(&prev);
        lines[1] = serde_json::to_string(&r).unwrap();
        let err = verify_chain(lines.iter().map(|s| s.as_str())).unwrap_err();
        assert_eq!(err.0, 3);
        assert!(err.1.contains("broken link"), "{}", err.1);
    }

    #[test]
    fn detects_deleted_reordered_and_bad_lines() {
        let lines = chain(4);
        let mut deleted = lines.clone();
        deleted.remove(1);
        assert_eq!(verify_chain(deleted.iter().map(|s| s.as_str())).unwrap_err().0, 2);

        let mut swapped = lines.clone();
        swapped.swap(1, 2);
        assert_eq!(verify_chain(swapped.iter().map(|s| s.as_str())).unwrap_err().0, 2);

        let mut garbage = lines.clone();
        garbage.insert(2, "{not json".into());
        let err = verify_chain(garbage.iter().map(|s| s.as_str())).unwrap_err();
        assert_eq!(err.0, 3);
        assert!(err.1.contains("valid receipt"));

        // Non-increasing seq, even with valid hashes.
        let mut a = receipt(5);
        a.seal(GENESIS);
        let mut b = receipt(5);
        b.seal(&a.hash);
        let l = [serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap()];
        let err = verify_chain(l.iter().map(|s| s.as_str())).unwrap_err();
        assert_eq!(err.0, 2);
        assert!(err.1.contains("sequence"));
    }

    #[test]
    fn hash_ignores_key_order_and_whitespace() {
        let lines = chain(2);
        // Re-serialize line 1 with a different key order and spacing.
        let v: Value = serde_json::from_str(&lines[0]).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap().replace('\n', " ");
        let l = [pretty, lines[1].clone()];
        assert_eq!(verify_chain(l.iter().map(|s| s.as_str())), Ok(2));
    }
}
