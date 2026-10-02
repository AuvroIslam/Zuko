// Documents: scan a .txt/.md/.pdf/code file, write a masked copy
// (<inbox>/<stem>.zuko.md), and report what was found (CONTRACTS.md SanitizeResult).
// PDFs are reduced to their text layer ("## Page N" sections); a PDF without one
// gets a warning instead of a silent empty file.
//
// OWNER: state & features (wave 2). Stub until then.

use serde::Serialize;

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
}

pub fn sanitize_file(engine: &crate::engine::Engine, path: &str) -> Result<SanitizeResult, String> {
    let _ = (engine, path);
    Err("not implemented yet".into())
}
