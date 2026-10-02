// MAIN-world entry (manifest: world MAIN, run_at document_start). Runs before the page's own
// scripts so the site picks up the wrapped fetch/XHR. Keep this file small: it runs under the
// page's CSP and has no access to chrome.* APIs. Detection, the vault and the WASM engine
// live elsewhere; this script only intercepts and asks over a MessageChannel.

import { siteForHost } from "../shared/sites.ts";
import { installNetGuard } from "./guard-core.ts";
import type { GuardWindow } from "./pipeline.ts";

const w = window as GuardWindow;
const site = siteForHost(w.location.hostname);
if (site && !(w as any).__zukoNetGuard) {
  Object.defineProperty(w, "__zukoNetGuard", { value: true, enumerable: false });
  installNetGuard(w, site);
}
