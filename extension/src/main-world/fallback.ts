// Last-resort detector for the page world. It is used only when the engine cannot be
// reached (WASM missing, service worker unresponsive, extension reloaded under an open tab):
// a request that carries one of these high-confidence secrets is blocked instead of sent.
//
// Deliberately tiny and conservative (no entropy rules, no PII): every pattern is a
// well-known token shape or a private key header, so a false positive means "block one
// prompt with something that looks exactly like a live credential".

const RULES: Array<[label: string, re: RegExp]> = [
  ["private key", /-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----/],
  ["Anthropic API key", /\bsk-ant-[A-Za-z0-9_-]{20,}/],
  ["OpenAI API key", /\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}/],
  ["Stripe key", /\b[rs]k_(?:live|test)_[0-9A-Za-z]{20,}/],
  ["AWS access key", /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/],
  ["GitHub token", /\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{50,})/],
  ["GitLab token", /\bglpat-[A-Za-z0-9_-]{20,}/],
  ["Slack token", /\bxox[abprs]-[A-Za-z0-9-]{10,}/],
  ["Google API key", /\bAIza[0-9A-Za-z_-]{35}\b/],
  ["npm token", /\bnpm_[A-Za-z0-9]{36}\b/],
  ["Hugging Face token", /\bhf_[A-Za-z0-9]{34,}\b/],
  ["JSON web token", /\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/],
];

/** The label of the first high-confidence secret in `text`, or null. */
export function findHighConfidenceSecret(text: string): string | null {
  for (const [label, re] of RULES) if (re.test(text)) return label;
  return null;
}
