//! Static analysis of Bash and PowerShell commands, best effort.
//!
//! Splits a command line into simple commands (on `;`, `&&`, `||`, `|`, `&`, newlines,
//! and inside `$( … )`, backticks, `bash -c '…'`, `sh -c`, `cmd /c`, `powershell -Command`
//! / `pwsh -c`), tokenizes with shell quoting rules (single quotes, double quotes,
//! backslash in Bash, backtick in PowerShell), and recognises:
//! * **network egress**: curl, wget, Invoke-WebRequest/iwr, Invoke-RestMethod/irm,
//!   Start-BitsTransfer, nc/ncat/netcat, telnet, ssh, scp, sftp, rsync (remote), ftp,
//!   git push / git clone / git fetch / git remote add, npm publish, pip download,
//!   python -c "…requests…/urllib", node -e "…fetch…", certutil -urlcache, bitsadmin;
//!   plus every URL and host literal (`https://…`, `user@host:`, bare `host.tld/path`
//!   arguments of network tools);
//! * **destructive**: rm -r/-rf/-fr, rmdir /s, del /s /q, Remove-Item -Recurse/-Force,
//!   git reset --hard, git clean -f[dx], git push --force/-f/--force-with-lease,
//!   git branch -D, git checkout -- ., dd of=, mkfs, format, diskpart, shred, truncate,
//!   DROP TABLE/DATABASE, `> file` truncation of existing paths, find … -delete;
//! * **privilege**: sudo, su, doas, runas, Start-Process -Verb RunAs, Set-ExecutionPolicy,
//!   reg add/delete, schtasks /create, sc create/config, chmod 777 / chmod +s, chown,
//!   icacls … /grant Everyone, netsh advfirewall;
//! * **installs / supply chain**: npm/pnpm/yarn/bun install|add (with package names),
//!   pip install, cargo install, go install, gem install, choco/winget/scoop install,
//!   apt/dnf/brew install, npx/pnpx/bunx <remote pkg>;
//! * **pipe-to-shell**: `curl … | sh|bash|zsh|python|node`, `iwr … | iex`, `iex (iwr …)`;
//! * **obfuscation**: eval, iex/Invoke-Expression, `-EncodedCommand`/`-enc`, `base64 -d`
//!   piped to a shell, `$'\x..'`, `${!var}`, very long single tokens, command
//!   substitution building a program name. Obfuscation sets [`ShellAnalysis::obfuscated`];
//!   unbalanced quotes or parse failures set [`ShellAnalysis::unparseable`];
//! * **process control**: kill/taskkill/Stop-Process with names or PIDs
//!   ([`ShellAnalysis::killed_processes`]);
//! * **file targets**: path-like arguments (reads), redirection targets `>`/`>>`/`Out-File`/
//!   `Set-Content`/`tee` (writes), cp/mv/Copy-Item/Move-Item destinations (writes),
//!   rm/del/Remove-Item targets (deletes), cat/type/Get-Content/less/head/tail sources (reads).
//!
//! Paths are returned exactly as written; [`crate::action`] resolves them against cwd.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Bash,
    PowerShell,
}

/// One simple command after splitting.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Program name as written, lowercased, without directory or `.exe`
    /// (`/usr/bin/curl` → `curl`, `C:\x\git.exe` → `git`).
    pub program: String,
    pub args: Vec<String>,
}

/// A notable thing the command does, with a plain-English sentence for the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flag {
    /// `network`, `destructive`, `privilege`, `install`, `pipe_to_shell`,
    /// `obfuscated`, `unparseable`, `kill`, `write`, `delete`.
    pub kind: String,
    /// e.g. "Deletes build/ and everything inside it", "Sends data to webhook.site".
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellAnalysis {
    pub segments: Vec<Segment>,
    /// Lowercased hosts contacted (from URLs, `user@host`, git remotes, ssh targets).
    pub hosts: Vec<String>,
    pub urls: Vec<String>,
    /// Paths read (as written).
    pub reads: Vec<String>,
    /// Paths written or created (as written).
    pub writes: Vec<String>,
    /// Paths deleted (as written).
    pub deletes: Vec<String>,
    /// Process names or PIDs targeted by kill/taskkill/Stop-Process.
    pub killed_processes: Vec<String>,
    pub network: bool,
    pub destructive: bool,
    pub privilege: bool,
    pub installs: bool,
    pub pipe_to_shell: bool,
    pub obfuscated: bool,
    pub unparseable: bool,
    pub flags: Vec<Flag>,
}

/// Analyses `command`. Never panics; malformed input yields `unparseable = true` with
/// whatever could be recovered.
pub fn analyze(command: &str, dialect: Dialect) -> ShellAnalysis {
    let _ = (command, dialect);
    todo!()
}

/// Tokenizes one simple command with the dialect's quoting rules (exposed for tests and
/// policy matching).
pub fn tokenize(command: &str, dialect: Dialect) -> Result<Vec<String>, String> {
    let _ = (command, dialect);
    todo!()
}
