// Placeholder helpers shared by the net guard (note) and the content script (matching).
//
// Format (zuko-core `placeholder.rs`): `{{KIND_N}}`, KIND = [A-Z][A-Z0-9_]*, N >= 1.
//
// Models sometimes mangle placeholders on the way back: dropped or single brackets, lower
// case, a space or nothing instead of `_`. `VariantMatcher` finds those too, but only for
// keys in the known set, so `HTTP_2`, `SHA_256` or a user's own `{{ name }}` template are
// never touched. The idea (not the code) follows Redacto's placeholder-variants (Apache-2.0).

export const NOTE_PREFIX = "[Zuko privacy note:";

const KEY_RE = /^([A-Z][A-Z0-9_]*[A-Z0-9])_([1-9]\d*)$|^([A-Z])_([1-9]\d*)$/;

export function parseKey(key: string): { kind: string; n: number } | null {
  const m = KEY_RE.exec(key);
  if (!m) return null;
  return { kind: m[1] ?? m[3]!, n: Number(m[2] ?? m[4]) };
}

export function wrap(key: string): string {
  return `{{${key}}}`;
}

/** Keys of every well-formed `{{KEY}}` in the text (no vault check), in order of appearance. */
export function canonicalKeys(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/\{\{ ?([A-Z][A-Z0-9_]*_[1-9]\d*) ?\}\}/g)) out.push(m[1]!);
  return out;
}

/** One-line note that tells the model what the placeholders mean. No `]` inside. */
export function buildNote(keys: string[]): string {
  const uniq = [...new Set(keys)];
  const shown = uniq.slice(0, 6).map(wrap).join(", ") + (uniq.length > 6 ? ", ..." : "");
  const verb = uniq.length === 1 ? "is a placeholder" : "are placeholders";
  return (
    `${NOTE_PREFIX} ${shown} ${verb} for private values kept on the user's machine. ` +
    `Use ${uniq.length === 1 ? "it" : "them"} verbatim, exactly as written, and never ask for or guess the real values.]`
  );
}

const NOTE_AT_START = /^\s*\[Zuko privacy note:[^\]]*\]\s*/;

export function hasNote(text: string): boolean {
  return NOTE_AT_START.test(text);
}

/** Removes a leading Zuko note (used to hide it in the displayed user message). */
export function stripNote(text: string): string {
  return text.replace(NOTE_AT_START, "");
}

/** Prepends the note (replacing a stale one) when `keys` is not empty. */
export function withNote(text: string, keys: string[]): string {
  if (keys.length === 0) return text;
  return `${buildNote(keys)}\n\n${stripNote(text)}`;
}

// ---------------------------------------------------------------------------------------
// Tolerant matching

export interface VariantMatch {
  start: number;
  end: number;
  /** Canonical key, e.g. `API_KEY_1`. */
  key: string;
  /** The text as it appeared. */
  text: string;
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const WORD = /[\p{L}\p{N}_]/u;
const isWord = (ch: string | undefined) => ch !== undefined && ch !== "" && WORD.test(ch);

export class VariantMatcher {
  private readonly keys: Set<string>;
  private readonly flatKinds = new Map<string, string>();
  private readonly re: RegExp | null;

  constructor(keys: Iterable<string>) {
    this.keys = new Set();
    const kinds = new Set<string>();
    for (const k of keys) {
      const p = parseKey(k);
      if (!p) continue;
      this.keys.add(k);
      kinds.add(p.kind);
    }
    for (const kind of kinds) {
      const flat = kind.replace(/_/g, "");
      if (!this.flatKinds.has(flat)) this.flatKinds.set(flat, kind);
    }
    if (kinds.size === 0) {
      this.re = null;
      return;
    }
    const kindAlt = [...kinds]
      .sort((a, b) => b.length - a.length)
      .map((k) => k.split("_").map(escapeRe).join("[_ ]?"))
      .join("|");
    this.re = new RegExp(
      `(\\{\\{[ \\t]*|\\[\\[[ \\t]*|\\{[ \\t]*|\\[[ \\t]*)?(${kindAlt})[_ ]?(\\d{1,9})(?!\\d)([ \\t]*\\}\\}|[ \\t]*\\]\\]|[ \\t]*\\}|[ \\t]*\\])?`,
      "gi",
    );
  }

  get size(): number {
    return this.keys.size;
  }

  has(key: string): boolean {
    return this.keys.has(key);
  }

  /** Cheap pre-check so most text nodes are skipped without running the regex. */
  mightContain(text: string): boolean {
    return this.re !== null && /\d/.test(text);
  }

  find(text: string): VariantMatch[] {
    const re = this.re;
    if (!re || !text) return [];
    re.lastIndex = 0;
    const out: VariantMatch[] = [];
    let m: RegExpExecArray | null;
    while ((m = re.exec(text)) !== null) {
      const whole = m[0];
      const open: string | undefined = m[1];
      const kindText = m[2]!;
      const digits = m[3]!;
      const close: string | undefined = m[4];
      if (digits.startsWith("0")) continue;
      const kind = this.flatKinds.get(kindText.replace(/[_ ]/g, "").toUpperCase());
      if (!kind) continue;
      const key = `${kind}_${Number(digits)}`;
      if (!this.keys.has(key)) continue;
      const start = m.index;
      const end = start + whole.length;
      if (!open && !close) {
        // Bare form (`API_KEY_1`): uppercase only, and on word boundaries, so code that
        // happens to contain `api_key_1` or `MY_API_KEY_1` is left alone.
        if (kindText !== kindText.toUpperCase()) continue;
        if (isWord(text[start - 1]) || isWord(text[end])) continue;
      } else {
        if (!open && isWord(text[start - 1])) continue;
        if (!close && isWord(text[end])) continue;
      }
      out.push({ start, end, key, text: whole });
    }
    return out;
  }

  /** The distinct keys that appear (in any accepted form). */
  keysIn(text: string): string[] {
    return [...new Set(this.find(text).map((v) => v.key))];
  }
}

export interface Replaced {
  text: string;
  /** Ranges of the inserted values in `text`. */
  ranges: Array<{ start: number; end: number; key: string }>;
  count: number;
}

/**
 * Replaces every variant whose value `lookup` knows. Placeholders without a value are left
 * exactly as they were.
 */
export function replaceVariants(
  text: string,
  matcher: VariantMatcher,
  lookup: (key: string) => string | undefined,
): Replaced {
  const matches = matcher.find(text);
  if (matches.length === 0) return { text, ranges: [], count: 0 };
  let out = "";
  let pos = 0;
  const ranges: Replaced["ranges"] = [];
  for (const v of matches) {
    const value = lookup(v.key);
    if (value === undefined) continue;
    out += text.slice(pos, v.start);
    ranges.push({ start: out.length, end: out.length + value.length, key: v.key });
    out += value;
    pos = v.end;
  }
  out += text.slice(pos);
  return { text: out, ranges, count: ranges.length };
}
