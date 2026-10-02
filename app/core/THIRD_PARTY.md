# Third-party notices for zuko-core

## gitleaks (MIT)

Many of the secret-detection regular expressions in `src/detect.rs` (`SECRET_SPECS`:
API keys, tokens, private keys, webhook URLs, the keyword prefilter idea and the
templated-value allowlist) are derived from the rule set of
[gitleaks](https://github.com/gitleaks/gitleaks) (`config/gitleaks.toml`), adapted to the
Rust `regex` crate and to Zuko's placeholder kinds.

```
MIT License

Copyright (c) 2019 Zachary Rice

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Ideas without copied code

- **Yelp detect-secrets** (Apache-2.0): the false-positive filters (templated values,
  indirect references, UUID/hash heuristics) inspired the allowlists in `src/detect.rs`.
  No code or patterns were copied.
- **Microsoft Presidio** (MIT): the context-word approach for national IDs, passports and
  dates of birth. No code was copied.

No code from AGPL-licensed projects (for example TruffleHog) is used.

## Test fixtures

All secrets, keys, tokens, card numbers, IBANs and personal data in `tests/fixtures/`
are fake: either generated for these tests (and marked `Fake`/`Zuko` where the format
allows) or well-known public test values (Stripe/Visa test card numbers, the IBAN
examples from the ISO 13616 registry).
