// Per-site request body rewriters: where the user's prompt text lives in each chat's own
// JSON, so masking can run on exactly those strings (and not on ids or anti-bot tokens,
// which look high-entropy and must never be touched).
//
//   ChatGPT   POST /backend-api/(f/)?conversation   messages[].content.parts[] (strings, and
//             the text of multimodal parts)
//   claude.ai POST .../chat_conversations/<id>/completion   prompt, attachments[].extracted_content
//   DeepSeek  POST /api/v0/chat/completion | edit_message    prompt
//
// Pure functions over a parsed body: the caller supplies `mask`, which talks to the engine.

import { withNote } from "../shared/placeholders.ts";
import type { SiteId } from "../shared/sites.ts";

export interface BodyField {
  text: string;
  set(value: string): void;
}

export interface SiteRewriter {
  /** Every string the user wrote or attached, in body order. */
  fields(body: any): BodyField[];
  /** The text that should carry the placeholder note; null when there is none. */
  noteField(body: any, fields: BodyField[]): BodyField | null;
}

const isObj = (v: unknown): v is Record<string, any> => typeof v === "object" && v !== null && !Array.isArray(v);

function stringField(holder: Record<string, any> | any[], key: string | number): BodyField | null {
  const v = (holder as any)[key];
  if (typeof v !== "string") return null;
  const field: BodyField = {
    text: v,
    set(value) {
      (holder as any)[key] = value;
      field.text = value; // later steps (the note) must build on the masked text, not the original
    },
  };
  return field;
}

const chatgpt: SiteRewriter = {
  fields(body) {
    const out: BodyField[] = [];
    if (!isObj(body) || !Array.isArray(body.messages)) return out;
    for (const m of body.messages) {
      const content = isObj(m) ? m.content : null;
      if (!isObj(content)) continue;
      if (Array.isArray(content.parts)) {
        content.parts.forEach((part: unknown, i: number) => {
          if (typeof part === "string") {
            const f = stringField(content.parts, i);
            if (f) out.push(f);
          } else if (isObj(part)) {
            // multimodal_text: image/audio pointers have no text; transcriptions and text parts do.
            const f = stringField(part, "text");
            if (f) out.push(f);
          }
        });
      }
      const t = stringField(content, "text");
      if (t) out.push(t);
    }
    return out;
  },
  noteField(body, fields) {
    if (!isObj(body) || !Array.isArray(body.messages)) return null;
    // The last user message's first string part; fall back to any first field.
    for (let i = body.messages.length - 1; i >= 0; i--) {
      const m = body.messages[i];
      if (!isObj(m) || (m.author?.role ?? "user") !== "user" || !isObj(m.content)) continue;
      if (Array.isArray(m.content.parts)) {
        const idx = m.content.parts.findIndex((p: unknown) => typeof p === "string");
        if (idx >= 0) return stringField(m.content.parts, idx);
      }
      const t = stringField(m.content, "text");
      if (t) return t;
    }
    return fields[0] ?? null;
  },
};

const claude: SiteRewriter = {
  fields(body) {
    const out: BodyField[] = [];
    if (!isObj(body)) return out;
    const p = stringField(body, "prompt");
    if (p) out.push(p);
    if (Array.isArray(body.attachments)) {
      for (const a of body.attachments) {
        if (!isObj(a)) continue;
        const f = stringField(a, "extracted_content");
        if (f) out.push(f);
      }
    }
    return out;
  },
  noteField(body) {
    return isObj(body) ? stringField(body, "prompt") : null;
  },
};

const deepseek: SiteRewriter = {
  fields(body) {
    const f = isObj(body) ? stringField(body, "prompt") : null;
    return f ? [f] : [];
  },
  noteField(body) {
    return isObj(body) ? stringField(body, "prompt") : null;
  },
};

const REWRITERS: Record<SiteId, SiteRewriter> = { chatgpt, claude, deepseek };

export function rewriterFor(site: SiteId): SiteRewriter {
  return REWRITERS[site];
}

export interface MaskOutcome {
  texts: string[];
  /** Placeholders now present in `texts` that exist in the vault. */
  keys: string[];
  count: number;
}

export interface RewriteResult {
  /** The JSON to send. */
  body: string;
  /** The user-text fields, before and after (the fail-safe scan and tests use these). */
  count: number;
  keys: string[];
  changed: boolean;
}

/**
 * Masks the prompt fields of a known-endpoint JSON body. Returns null when the body is not
 * JSON or has none of the site's text fields (the caller then treats it as an ordinary body).
 */
export async function rewriteJson(
  site: SiteId,
  raw: string,
  mask: (texts: string[]) => Promise<MaskOutcome>,
): Promise<RewriteResult | null> {
  let body: any;
  try {
    body = JSON.parse(raw);
  } catch {
    return null;
  }
  const rw = rewriterFor(site);
  const fields = rw.fields(body);
  if (fields.length === 0) return null;

  const outcome = await mask(fields.map((f) => f.text));
  if (outcome.texts.length !== fields.length) throw new Error("mask returned the wrong number of texts");
  let changed = false;
  fields.forEach((f, i) => {
    const next = outcome.texts[i]!;
    if (next !== f.text) {
      f.set(next);
      changed = true;
    }
  });

  if (outcome.keys.length > 0) {
    const target = rw.noteField(body, fields);
    if (target) {
      const noted = withNote(target.text, outcome.keys);
      if (noted !== target.text) {
        target.set(noted);
        changed = true;
      }
    } else if (site !== "chatgpt" && isObj(body) && (body.prompt === undefined || body.prompt === "")) {
      // Only attachments carried placeholders: give the model the note anyway.
      body.prompt = withNote("", outcome.keys).trim();
      changed = true;
    }
  }
  return { body: changed ? JSON.stringify(body) : raw, count: outcome.count, keys: outcome.keys, changed };
}
