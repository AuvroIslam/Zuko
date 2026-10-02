// What counts as a text-like file or body (scannable as UTF-8), shared by the net guard and the
// upload interceptor.

/** Extensions the upload interceptor sanitizes as text. */
export const TEXT_EXT =
  /\.(txt|md|markdown|json|jsonl|ndjson|csv|tsv|log|xml|ya?ml|toml|ini|env|cfg|conf|properties|html?|css|scss|less|svg|[cm]?[jt]sx?|vue|svelte|py|rb|go|rs|java|kt|kts|scala|c|h|cc|cpp|hpp|cs|php|sh|bash|zsh|ps1|psm1|bat|cmd|sql|swift|dart|lua|r|pl|ex|exs|tf|gradle)$/i;

export const PDF_EXT = /\.pdf$/i;

const TEXT_TYPE = /^text\/|json|xml|javascript|ecmascript|csv|markdown|yaml|toml|x-www-form-urlencoded|x-sh\b|x-python|x-ruby|x-rust|x-go|x-java/i;

export function isTextLikeName(name: string): boolean {
  return TEXT_EXT.test(name);
}

export function isTextLikeType(type: string): boolean {
  return TEXT_TYPE.test(type);
}

export function isPdf(file: { name: string; type: string }): boolean {
  return file.type === "application/pdf" || PDF_EXT.test(file.name);
}

/** Text files and code Zuko can scan directly (PDFs are handled separately). */
export function isScannableText(file: { name: string; type: string }): boolean {
  return isTextLikeName(file.name) || (isTextLikeType(file.type) && !isPdf(file));
}

/** Largest body or file the net guard reads into memory to scan. */
export const MAX_SCAN_BYTES = 16 * 1024 * 1024;
