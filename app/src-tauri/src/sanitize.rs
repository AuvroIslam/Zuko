// Documents: scan a .txt/.md/.pdf/code file, write a masked copy
// (<inbox>/<stem>.zuko.md), and report what was found (CONTRACTS.md SanitizeResult).
//
// * Accepted: plain text (.txt .log .csv .tsv .json .yaml .yml .toml .ini .cfg .conf
//   .properties .env), Markdown (.md .markdown), common source code, and .pdf. Anything
//   else is refused with a message that says what is supported.
// * Files over 25 MB are refused. Text is read as UTF-8 (lossy, with a warning), or
//   as UTF-16 when it starts with a byte-order mark; a file full of NUL bytes is
//   refused as binary.
// * PDFs are reduced to their text layer, one "## Page N" section per page. A PDF
//   without a text layer (a scan) still gets an output file, with the warning
//   "No text layer found (scanned PDF?)" instead of a silent empty one.
// * Masking is the engine's (source "file"); new vault entries are persisted before
//   returning, because the output only makes sense while its placeholders can be
//   turned back into values.
// * The output starts with a short header: original name (masked like everything
//   else), date and the masked labels with their vault keys, never values. Code and
//   structured text (everything but .txt/.md) is fenced so it pastes cleanly.
// * Sanitizing the same file again overwrites its `.zuko.md`.
// * Local AI (optional, `policy.localAi.deepScanDocuments`): before masking, the
//   document (already deterministically masked, so known secrets stay hidden even
//   from the local model) is deep-scanned in chunks for names, addresses and other
//   values the patterns miss. Verified findings are interned into the vault, so the
//   normal masking pass below masks them like any other vault value. This waits, but
//   only within a bounded budget (localai::learn_document); on a timeout or a bad
//   answer the deterministic result stands and a warning says so. The result's
//   `aiDeepScan` reports "+N items". The AI can only add masks, never remove one.

use std::path::Path;

use serde::Serialize;
use zuko_core::mask::{self, MaskCtx, MaskReport};
use zuko_core::placeholder;

use crate::engine::Engine;

/// Largest file Zuko will read.
const MAX_BYTES: u64 = 25 * 1024 * 1024;
/// Length of `SanitizeResult::preview`, in characters.
const PREVIEW_CHARS: usize = 1500;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingCount {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizeResult {
    pub name: String,
    pub input_path: String,
    pub output_path: String,
    /// text | markdown | pdf | code
    pub kind: String,
    pub pages: Option<u32>,
    pub findings: Vec<FindingCount>,
    pub preview: String,
    pub warnings: Vec<String>,
    /// What the local AI deep scan added (None when it is off).
    pub ai_deep_scan: Option<AiDeepScan>,
}

/// The local AI's part in one sanitize (CONTRACTS.md `SanitizeResult.aiDeepScan`).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDeepScan {
    /// Values the model found (and Zuko verified) that the patterns had not masked.
    pub items: usize,
    /// Of those, values the vault had never seen.
    pub new_items: usize,
    pub model: String,
    pub ms: u64,
    /// Why the scan stopped early or failed, if it did.
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Text,
    Markdown,
    Code,
    Pdf,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Markdown => "markdown",
            Kind::Code => "code",
            Kind::Pdf => "pdf",
        }
    }
}

const TEXT_EXTS: &[&str] =
    &["txt", "log", "csv", "tsv", "json", "jsonl", "ndjson", "yaml", "yml", "toml", "ini", "cfg", "conf", "properties", "env"];
const MARKDOWN_EXTS: &[&str] = &["md", "markdown"];
const CODE_EXTS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "mjs", "cjs", "go", "java", "kt", "kts", "swift", "c", "h", "cpp", "cc", "hpp", "cs", "rb",
    "php", "sh", "bash", "zsh", "ps1", "psm1", "bat", "cmd", "sql", "html", "htm", "css", "scss", "xml", "vue", "svelte", "lua", "r",
    "dart", "scala", "pl", "ex", "exs", "tf", "gradle",
];
/// Extension-less files that are plain text.
const TEXT_NAMES: &[&str] = &["dockerfile", "makefile", "readme", "license", "procfile"];

fn classify(path: &Path) -> Result<(Kind, String), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // `.env`, `.env.local`, `.env.production`: Path::extension() sees none or "local".
    if name == ".env" || name.starts_with(".env.") {
        return Ok((Kind::Text, "env".into()));
    }
    if ext == "pdf" {
        return Ok((Kind::Pdf, ext));
    }
    if MARKDOWN_EXTS.contains(&ext.as_str()) {
        return Ok((Kind::Markdown, ext));
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        return Ok((Kind::Text, ext));
    }
    if CODE_EXTS.contains(&ext.as_str()) {
        return Ok((Kind::Code, ext));
    }
    if ext.is_empty() && TEXT_NAMES.contains(&name.as_str()) {
        return Ok((Kind::Text, "txt".into()));
    }
    let what = if ext.is_empty() { "files without an extension".to_string() } else { format!(".{ext} files") };
    Err(format!(
        "Zuko can't read {what} yet. It handles text, Markdown, JSON/CSV/YAML/TOML/INI/.env files, source code and PDF."
    ))
}

/// Sanitizes `path` into the inbox (see the module doc).
#[allow(dead_code)]
pub fn sanitize_file(engine: &Engine, path: &str) -> Result<SanitizeResult, String> {
    sanitize_file_report(engine, path).map(|(result, _)| result)
}

/// Like [`sanitize_file`], also returning the engine's mask report (new keys, total
/// count) for the privacy notice.
pub fn sanitize_file_report(engine: &Engine, path: &str) -> Result<(SanitizeResult, MaskReport), String> {
    sanitize_into(engine, path, &crate::files::inbox_dir())
}

/// A file reduced to the text Zuko works on.
pub struct Extracted {
    pub kind: Kind,
    /// Lowercase extension (or "env" / "txt" for the special names).
    pub ext: String,
    /// The file's own name.
    pub name: String,
    /// Text for text kinds; "## Page N" sections for a PDF.
    pub body: String,
    pub pages: Option<u32>,
    pub warnings: Vec<String>,
}

/// Reads `input` and reduces it to text (see the module doc for what is accepted and
/// refused). Shared by the sanitizer and the island chat.
pub fn extract(input: &Path) -> Result<Extracted, String> {
    let meta = std::fs::metadata(input).map_err(|e| format!("Can't open {}: {e}", input.display()))?;
    if meta.is_dir() {
        return Err("That is a folder. Drop a single file.".into());
    }
    let (kind, ext) = classify(input)?;
    let name = input.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "{name} is {} MB; Zuko reads files up to {} MB.",
            meta.len().div_ceil(1024 * 1024),
            MAX_BYTES / (1024 * 1024)
        ));
    }
    let bytes = read_capped(input)?;

    let mut warnings = Vec::new();
    let mut pages = None;
    let body = if kind == Kind::Pdf {
        let page_texts = pdf_pages(&bytes)?;
        pages = Some(page_texts.len() as u32);
        if page_texts.iter().all(|p| p.trim().is_empty()) {
            warnings.push("No text layer found (scanned PDF?)".to_string());
        }
        pdf_markdown(&page_texts)
    } else {
        let (text, lossy) = decode_text(&bytes)?;
        if lossy {
            warnings.push("The file is not valid UTF-8; unreadable bytes were replaced.".to_string());
        }
        text
    };
    Ok(Extracted { kind, ext, name, body, pages, warnings })
}

fn sanitize_into(engine: &Engine, path: &str, inbox: &Path) -> Result<(SanitizeResult, MaskReport), String> {
    let input = Path::new(path);
    let Extracted { kind, ext, name, body, pages, mut warnings } = extract(input)?;

    // Optional local-AI deep scan first: it can only add vault entries, which the
    // masking pass below then applies.
    let ai_cfg = engine.policy().local_ai.clone();
    let ai_deep_scan = ai_cfg.scans_documents().then(|| {
        let (learned, note) = crate::localai::block_on(crate::localai::learn_document(engine, &format!("{name}
{body}")));
        if let Some(n) = &note {
            warnings.push(n.clone());
        }
        AiDeepScan { items: learned.found, new_items: learned.new_keys.len(), model: ai_cfg.model.trim().to_string(), ms: learned.ms, error: note }
    });

    // Mask the content and the original name in one pass over the vault.
    let det = engine.detector();
    let ctx = MaskCtx { source: "file".into(), now: crate::engine::now() };
    let (masked_body, masked_name, report) = engine.with_vault(|vault| {
        let (b, mut report) = mask::mask_text(&det, vault, &body, &ctx);
        // The stem is scanned on its own: the detector rightly ignores things that
        // look like file names (`icon@2x.png`), which would otherwise let
        // `invoice bob@acme.io.txt` through as one harmless-looking token.
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i..]),
            _ => (name.as_str(), ""),
        };
        let (n, name_report) = mask::mask_text(&det, vault, stem, &ctx);
        report.absorb(name_report);
        (b, format!("{n}{ext}"), report)
    });
    if report.count > 0 {
        engine.persist_vault();
        engine.flush_vault();
    }

    let findings = findings(engine, &body, &masked_body, &name, &masked_name, &report);
    let fenced = if matches!(kind, Kind::Markdown | Kind::Pdf) || ext == "txt" {
        masked_body
    } else {
        fence(&masked_body, &ext)
    };
    let output = format!("{}\n{}", header(&masked_name, &findings), fenced);

    std::fs::create_dir_all(inbox).map_err(|e| format!("Can't create the inbox: {e}"))?;
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let stem: String = stem.chars().take(100).collect();
    let out_path = inbox.join(format!("{stem}.zuko.md"));
    crate::files::write_atomic(&out_path, output.as_bytes()).map_err(|e| format!("Can't write {}: {e}", out_path.display()))?;

    let result = SanitizeResult {
        name,
        input_path: path.to_string(),
        output_path: out_path.to_string_lossy().to_string(),
        kind: kind.name().to_string(),
        pages,
        findings,
        preview: preview(&output),
        warnings,
        ai_deep_scan,
    };
    Ok((result, report))
}

/// Reads the file, never more than the cap (the file may have grown since `metadata`).
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("Can't open {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("The file is larger than {} MB.", MAX_BYTES / (1024 * 1024)));
    }
    Ok(bytes)
}

/// Text from raw bytes: UTF-16 when a BOM says so, else UTF-8 (lossy). The flag is
/// true when bytes had to be replaced.
fn decode_text(bytes: &[u8]) -> Result<(String, bool), String> {
    let utf16 = |rest: &[u8], le: bool| {
        let units = rest.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
        let mut lossy = rest.len() % 2 == 1;
        let text: String = char::decode_utf16(units)
            .map(|r| {
                r.unwrap_or_else(|_| {
                    lossy = true;
                    char::REPLACEMENT_CHARACTER
                })
            })
            .collect();
        (text, lossy)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Ok(utf16(rest, true));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Ok(utf16(rest, false));
    }
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Err("That file looks binary, not text.".into());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_string(), false)),
        Err(_) => Ok((String::from_utf8_lossy(bytes).into_owned(), true)),
    }
}

/// The text of each page. The PDF parser can panic on malformed files, so a panic is
/// turned into an ordinary error.
fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>, String> {
    match std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes)) {
        Ok(Ok(pages)) => Ok(pages),
        Ok(Err(e)) => {
            let why = e.to_string();
            if why.to_lowercase().contains("encrypt") || why.to_lowercase().contains("password") {
                Err("This PDF is password-protected. Remove the password and try again.".into())
            } else {
                Err(format!("Zuko couldn't read this PDF: {why}"))
            }
        }
        Err(_) => Err("Zuko couldn't read this PDF: it uses something the reader can't parse.".into()),
    }
}

fn pdf_markdown(pages: &[String]) -> String {
    let mut out = String::new();
    for (i, page) in pages.iter().enumerate() {
        let text = tidy_page(page);
        out.push_str(&format!("## Page {}\n\n", i + 1));
        if text.is_empty() {
            out.push_str("(no text on this page)\n\n");
        } else {
            out.push_str(&text);
            out.push_str("\n\n");
        }
    }
    out.trim_end().to_string() + "\n"
}

/// Line endings normalized, trailing spaces dropped, runs of blank lines collapsed.
fn tidy_page(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank > 0 {
                out.push('\n');
            }
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

/// Wraps `body` in a code fence longer than any backtick run inside it.
fn fence(body: &str, lang: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let ticks = "`".repeat((longest + 1).max(3));
    let nl = if body.ends_with('\n') { "" } else { "\n" };
    format!("{ticks}{lang}\n{body}{nl}{ticks}\n")
}

/// Per-key counts: placeholders in the masked texts minus the ones the input already
/// had, in the report's key order.
fn findings(engine: &Engine, body: &str, masked_body: &str, name: &str, masked_name: &str, report: &MaskReport) -> Vec<FindingCount> {
    let count = |text: &str, key: &str| placeholder::find_all(text).iter().filter(|(_, _, k)| k == key).count();
    engine.with_vault(|vault| {
        report
            .keys
            .iter()
            .filter_map(|key| {
                let entry = vault.get(key)?;
                let seen = count(masked_body, key) + count(masked_name, key);
                let before = count(body, key) + count(name, key);
                Some(FindingCount {
                    key: key.clone(),
                    kind: entry.kind.clone(),
                    label: entry.label.clone(),
                    count: seen.saturating_sub(before).max(1),
                })
            })
            .collect()
    })
}

/// The short banner at the top of the output. Labels and vault keys only.
fn header(masked_name: &str, findings: &[FindingCount]) -> String {
    let t = crate::platform::local_time();
    let mut h = format!("> Sanitized by Zuko · original: {masked_name} · {:04}-{:02}-{:02}\n", t.year, t.month, t.day);
    if findings.is_empty() {
        h.push_str("> Nothing sensitive found.\n");
    } else {
        let list: Vec<String> = findings
            .iter()
            .map(|f| format!("{} {}{}", f.label, placeholder::wrap(&f.key), if f.count > 1 { format!(" ×{}", f.count) } else { String::new() }))
            .collect();
        h.push_str(&format!("> Masked: {}. Paste an answer back into Zuko to restore the values.\n", list.join(", ")));
    }
    h
}

fn preview(text: &str) -> String {
    if text.chars().count() <= PREVIEW_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(PREVIEW_CHARS).collect();
    format!("{cut}…")
}

/// Test fixtures shared with other modules' tests.
#[cfg(test)]
pub(crate) mod tests_support {
    /// A small valid PDF: one page per entry, each with a Helvetica text object.
    pub fn tiny_pdf(pages: &[&str]) -> Vec<u8> {
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..pages.len()).map(|i| format!("{} 0 R", 4 + i * 2)).collect::<Vec<_>>().join(" "),
                pages.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        for (i, text) in pages.iter().enumerate() {
            let content_id = 5 + i * 2;
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_id} 0 R /Resources << /Font << /F1 3 0 R >> >> >>"
            ));
            let stream = if text.is_empty() {
                String::new()
            } else {
                let esc = text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
                format!("BT /F1 12 Tf 72 720 Td ({esc}) Tj ET")
            };
            objs.push(format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::tiny_pdf;
    use super::*;
    use crate::engine::CtxBase;
    use zuko_core::policy::Policy;
    use zuko_core::vault::Vault;

    const KEY: &str = "sk-proj-abcdefghijklmnopqrstuvwx1234";

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-san-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn engine() -> Engine {
        Engine::with_parts(Policy::default(), Vault::new(), CtxBase::default())
    }

    fn run(e: &Engine, dir: &Path, file: &str, content: &[u8]) -> Result<(SanitizeResult, MaskReport), String> {
        let p = dir.join(file);
        std::fs::write(&p, content).unwrap();
        sanitize_into(e, &p.to_string_lossy(), &dir.join("inbox"))
    }

    #[test]
    fn the_local_ai_deep_scan_adds_names_and_addresses() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ollama = rt.block_on(crate::localai::mock::start(Default::default()));
        let mut p = Policy::default();
        p.local_ai.enabled = true;
        p.local_ai.endpoint = ollama.url.clone();
        let e = Engine::with_parts(p, Vault::new(), CtxBase::default());
        let dir = tmp("ai");
        let src = format!("Lease\nTenant: Rahim Uddin, House 12, Road 5, Dhanmondi, Dhaka\nOPENAI_API_KEY={KEY}\n");
        let (r, report) = run(&e, &dir, "lease.md", src.as_bytes()).unwrap();
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(!out.contains("Rahim Uddin") && !out.contains("Dhanmondi") && !out.contains(KEY), "{out}");
        assert!(out.contains("{{NAME_1}}") && out.contains("{{ADDRESS_1}}") && out.contains("{{API_KEY_1}}"));
        let ai = r.ai_deep_scan.as_ref().expect("the deep scan ran");
        assert_eq!((ai.items, ai.new_items, ai.error.as_deref()), (2, 2, None));
        assert_eq!(report.count, 3);
        assert!(r.findings.iter().any(|f| f.label == "Person name"));
        // The model never saw the API key.
        assert!(!ollama.chat_bodies().concat().contains(KEY));

        // Off: no deep scan, no field.
        let (r, _) = run(&engine(), &dir, "lease2.md", src.as_bytes()).unwrap();
        assert!(r.ai_deep_scan.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn masks_text_and_writes_the_copy() {
        let dir = tmp("text");
        let e = engine();
        let src = format!("deploy notes\nOPENAI_API_KEY={KEY}\nping alice@acme.io twice: alice@acme.io\n");
        let (r, report) = run(&e, &dir, "notes.txt", src.as_bytes()).unwrap();
        assert_eq!(r.kind, "text");
        assert_eq!(r.name, "notes.txt");
        assert!(r.output_path.ends_with("notes.zuko.md"));
        assert!(r.warnings.is_empty());
        assert_eq!(report.new_keys.len(), 2);

        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(!out.contains(KEY) && !out.contains("alice@acme.io"), "values leaked into the copy:\n{out}");
        assert!(out.contains("OPENAI_API_KEY={{API_KEY_1}}"));
        assert!(out.contains("{{EMAIL_1}}"));
        // Header: original name, labels with keys, no values.
        assert!(out.starts_with("> Sanitized by Zuko · original: notes.txt"));
        assert!(out.contains("{{EMAIL_1}} ×2"), "{out}");

        // Findings are aggregated per key.
        let email = r.findings.iter().find(|f| f.key == "EMAIL_1").unwrap();
        assert_eq!(email.count, 2);
        assert_eq!(email.kind, "EMAIL");
        assert!(!email.label.is_empty());
        let api = r.findings.iter().find(|f| f.key == "API_KEY_1").unwrap();
        assert_eq!(api.count, 1);
        assert_eq!(r.findings.len(), 2);

        // The vault can turn the copy back into the original text.
        let (back, _) = e.with_vault(|v| mask::rehydrate_text(v, &out));
        assert!(back.contains(KEY) && back.contains("alice@acme.io"));
        assert!(r.preview.starts_with("> Sanitized by Zuko"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn placeholders_already_in_the_file_are_not_counted_as_findings() {
        let dir = tmp("pre");
        let e = engine();
        let src = format!("first {KEY}\nagain {{{{API_KEY_1}}}} and {KEY}\n");
        let (r, _) = run(&e, &dir, "a.md", src.as_bytes()).unwrap();
        assert_eq!(r.kind, "markdown");
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].count, 2, "two real values, the pre-existing placeholder is not a finding");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_and_structured_files_are_fenced_and_names_are_masked() {
        let dir = tmp("code");
        let e = engine();
        let (r, _) = run(&e, &dir, "config.json", format!("{{\"key\": \"{KEY}\"}}").as_bytes()).unwrap();
        assert_eq!(r.kind, "text");
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(out.contains("```json\n{\"key\": \"{{API_KEY_1}}\"}\n```"), "{out}");

        let (r, _) = run(&e, &dir, "main.rs", b"fn main() { println!(\"```\"); }").unwrap();
        assert_eq!(r.kind, "code");
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(out.contains("````rs\n"), "the fence must be longer than the backticks inside:\n{out}");

        // A file name that contains a value does not leak through the header.
        let (r, _) = run(&e, &dir, "invoice bob@acme.io.txt", b"total 5").unwrap();
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(!out.contains("bob@acme.io"), "{out}");
        assert!(r.findings.iter().any(|f| f.kind == "EMAIL"));

        // .env files by name.
        let (r, _) = run(&e, &dir, ".env", format!("TOKEN={KEY}").as_bytes()).unwrap();
        assert_eq!(r.kind, "text");
        assert!(!std::fs::read_to_string(&r.output_path).unwrap().contains(KEY));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_unsupported_binary_huge_and_missing_files() {
        let dir = tmp("refuse");
        let e = engine();
        let err = run(&e, &dir, "slides.pptx", b"PK").unwrap_err();
        assert!(err.contains(".pptx") && err.contains("PDF"), "{err}");
        assert!(run(&e, &dir, "photo.png", b"\x89PNG").unwrap_err().contains(".png"));
        assert!(run(&e, &dir, "noext", b"hello").unwrap_err().contains("without an extension"));
        assert!(run(&e, &dir, "blob.txt", b"abc\0def\0\0\0").unwrap_err().contains("binary"));
        assert!(sanitize_into(&e, &dir.join("missing.txt").to_string_lossy(), &dir.join("inbox")).is_err());
        assert!(sanitize_into(&e, &dir.to_string_lossy(), &dir.join("inbox")).unwrap_err().contains("folder"));
        // Nothing was written for a refused file.
        assert!(!dir.join("inbox").exists() || std::fs::read_dir(dir.join("inbox")).unwrap().count() == 0);

        // Over the cap.
        let big = dir.join("big.txt");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();
        let err = sanitize_into(&e, &big.to_string_lossy(), &dir.join("inbox")).unwrap_err();
        assert!(err.contains("25 MB"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decodes_utf16_and_lossy_utf8() {
        let dir = tmp("enc");
        let e = engine();
        let text = format!("secret {KEY} café");
        let mut utf16 = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let (r, _) = run(&e, &dir, "ps.txt", &utf16).unwrap();
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(out.contains("{{API_KEY_1}} café"), "{out}");
        assert!(r.warnings.is_empty());

        let (r, _) = run(&e, &dir, "latin.txt", b"caf\xe9 ok").unwrap();
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("UTF-8"));

        // BOM-prefixed UTF-8 loses the BOM.
        let (r, _) = run(&e, &dir, "bom.txt", b"\xef\xbb\xbfhello").unwrap();
        assert!(std::fs::read_to_string(&r.output_path).unwrap().ends_with("\nhello"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pdf_text_is_extracted_per_page_and_masked() {
        let dir = tmp("pdf");
        let e = engine();
        let pdf = tiny_pdf(&["Invoice for alice@acme.io", &format!("Key {KEY} (do not share)")]);
        let (r, report) = run(&e, &dir, "invoice.pdf", &pdf).unwrap();
        assert_eq!(r.kind, "pdf");
        assert_eq!(r.pages, Some(2));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(report.count, 2);
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(r.output_path.ends_with("invoice.zuko.md"));
        assert!(out.contains("## Page 1") && out.contains("## Page 2"), "{out}");
        assert!(out.contains("Invoice for {{EMAIL_1}}"), "{out}");
        assert!(out.contains("Key {{API_KEY_1}} (do not share)"), "{out}");
        assert!(!out.contains("alice@acme.io") && !out.contains(KEY));
        assert_eq!(r.findings.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pdf_without_a_text_layer_warns_and_still_writes_a_file() {
        let dir = tmp("scan");
        let e = engine();
        let (r, report) = run(&e, &dir, "scan.pdf", &tiny_pdf(&["", ""])).unwrap();
        assert_eq!(r.pages, Some(2));
        assert_eq!(r.warnings, vec!["No text layer found (scanned PDF?)".to_string()]);
        assert_eq!(report.count, 0);
        let out = std::fs::read_to_string(&r.output_path).unwrap();
        assert!(out.contains("## Page 1") && out.contains("(no text on this page)"));
        assert!(out.contains("Nothing sensitive found."));

        // Garbage that claims to be a PDF is an error, not a panic.
        let err = run(&e, &dir, "broken.pdf", b"%PDF-1.4 this is not really a pdf").unwrap_err();
        assert!(err.contains("PDF"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_is_capped() {
        let dir = tmp("preview");
        let e = engine();
        let (r, _) = run(&e, &dir, "long.txt", "word ".repeat(2000).as_bytes()).unwrap();
        assert_eq!(r.preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(r.preview.ends_with('…'));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
