//! Detector performance. Targets (release): `Detector::new` < 20 ms, scanning 1 MB of
//! mixed code/prose/secrets < 100 ms. Asserts are generous (machines vary, debug builds
//! are ~10–30× slower); run `cargo test --release -p zuko-core --test privacy_perf --
//! --nocapture` to see the real numbers.

use std::time::{Duration, Instant};
use zuko_core::detect::{Detector, DetectorConfig};
use zuko_core::mask::{mask_text, MaskCtx};
use zuko_core::vault::Vault;

const CLEAN_CODE: &str = include_str!("fixtures/clean_code.txt");
const POSITIVE: &str = include_str!("fixtures/detect_positive.txt");

fn corpus(min_len: usize) -> String {
    let secrets: String = POSITIVE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('@'))
        .map(|l| {
            // Drop the «KIND/rule:value» markers, keep the value.
            let mut out = String::new();
            let mut rest = l;
            while let Some(i) = rest.find('«') {
                out.push_str(&rest[..i]);
                let after = &rest[i + '«'.len_utf8()..];
                let colon = after.find(':').unwrap();
                let close = after.find('»').unwrap();
                out.push_str(&after[colon + 1..close]);
                rest = &after[close + '»'.len_utf8()..];
            }
            out.push_str(rest);
            out.replace('␤', "\n") + "\n"
        })
        .collect();
    let prose = "আমার নাম রহিম, আমি ঢাকায় থাকি। The quick brown fox jumps over the lazy dog 42 times; see https://docs.acme.io/v2/guide#setup for details.\n";
    let mut s = String::with_capacity(min_len + 64 * 1024);
    let mut i = 0;
    while s.len() < min_len {
        s.push_str(CLEAN_CODE);
        s.push_str(prose);
        if i % 4 == 0 {
            s.push_str(&secrets);
        }
        i += 1;
    }
    s
}

fn limits() -> (Duration, Duration) {
    if cfg!(debug_assertions) {
        (Duration::from_millis(3000), Duration::from_secs(30))
    } else {
        (Duration::from_millis(150), Duration::from_millis(600))
    }
}

#[test]
fn detector_new_and_scan_1mb_are_fast() {
    let (new_limit, scan_limit) = limits();
    let cfg = DetectorConfig { ips: true, custom_terms: vec!["Project Falcon".into()], ..DetectorConfig::default() };
    // Warm up allocators/page cache, then take the best of 5 compilations.
    let _ = Detector::new(&cfg);
    let mut best_new = Duration::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let d = Detector::new(&cfg);
        best_new = best_new.min(t.elapsed());
        drop(d);
    }
    let det = Detector::new(&cfg);
    let text = corpus(1 << 20);
    let _ = det.scan(&text[..64 * 1024]);
    let mut best_scan = Duration::MAX;
    let mut n = 0;
    for _ in 0..3 {
        let t = Instant::now();
        n = det.scan(&text).len();
        best_scan = best_scan.min(t.elapsed());
    }
    println!(
        "Detector::new: {:.2} ms; scan {} KiB: {:.2} ms ({} findings)",
        best_new.as_secs_f64() * 1e3,
        text.len() / 1024,
        best_scan.as_secs_f64() * 1e3,
        n
    );
    assert!(n > 100, "corpus should contain findings, got {n}");
    assert!(best_new < new_limit, "Detector::new took {best_new:?}");
    assert!(best_scan < scan_limit, "scan of 1 MiB took {best_scan:?}");
}

#[test]
fn mask_1mb_is_fast_and_idempotent() {
    let (_, scan_limit) = limits();
    let det = Detector::new(&DetectorConfig::default());
    let mut vault = Vault::new();
    let ctx = MaskCtx { source: "test".into(), now: 1 };
    let text = corpus(1 << 20);
    let t = Instant::now();
    let (masked, report) = mask_text(&det, &mut vault, &text, &ctx);
    let first = t.elapsed();
    let t = Instant::now();
    let (again, report2) = mask_text(&det, &mut vault, &masked, &ctx);
    let second = t.elapsed();
    println!(
        "mask 1 MiB: {:.2} ms ({} replacements, {} keys); re-mask: {:.2} ms",
        first.as_secs_f64() * 1e3,
        report.count,
        report.keys.len(),
        second.as_secs_f64() * 1e3
    );
    assert!(report.count > 100);
    assert_eq!(again, masked, "masking must be idempotent");
    assert_eq!(report2.count, 0);
    assert!(first < scan_limit * 2, "mask took {first:?}");
}
