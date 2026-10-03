// Opening things in VS Code: the argument lists for `code`, kept pure so they can be
// tested without launching an editor.
//
// "Open file" on an activity row must land in the VS Code window that is already running
// the task, not in a fresh one. `code --goto <file>` on its own picks whichever window
// was active last, or opens a new one, so the session's working folder goes first:
// `code <folder> --goto <file>`. VS Code focuses the window that already has that folder
// open (or opens one for it) and opens the file there. A file outside that folder, or no
// usable folder at all, falls back to `code --reuse-window --goto <file>`.
//
// Every value is its own argument and no shell is involved (see `open_in_vscode` in
// lib.rs for why). On Windows `code` is `code.cmd`: Rust (1.77.2 and later) quotes
// arguments for batch files itself, including spaces, `&`, `^`, `%` and parentheses, and
// refuses an argument it cannot pass safely; `platform::no_console` keeps the cmd.exe
// that runs the batch file from flashing a console window.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The folder to hand VS Code before `file`: `cwd` when it is an absolute, existing
/// directory that contains `file`; otherwise None.
pub fn project_folder(file: &Path, cwd: Option<&str>) -> Option<PathBuf> {
    let cwd = cwd.map(str::trim).filter(|c| !c.is_empty())?;
    let dir = Path::new(cwd);
    if !(dir.is_absolute() && dir.is_dir()) {
        return None;
    }
    // Compared in canonical form, so `C:/a/b`, `C:\a\b` and a symlinked spelling agree.
    // The canonical (`\\?\`) forms are only used for the test, never passed to VS Code.
    let (file_c, dir_c) = (std::fs::canonicalize(file).ok()?, std::fs::canonicalize(dir).ok()?);
    contains(&dir_c, &file_c).then(|| dir.to_path_buf())
}

/// True when `path` lies under `dir` (component by component; case-insensitive on
/// Windows, where the file system is).
fn contains(dir: &Path, path: &Path) -> bool {
    let same = |a: Component, b: Component| {
        if cfg!(windows) {
            a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        } else {
            a == b
        }
    };
    let mut inner = path.components();
    let outer: Vec<Component> = dir.components().collect();
    outer.iter().all(|d| inner.next().is_some_and(|p| same(*d, p))) && path.components().count() > outer.len()
}

/// `code` arguments that open `file` in the window holding `folder`, or, without a
/// folder, in the last active window.
pub fn open_file_args(file: &Path, folder: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::with_capacity(3);
    match folder {
        // Never together with --reuse-window: that would swap the folder of whichever
        // window happens to be active for this one.
        Some(dir) => args.push(dir.as_os_str().to_owned()),
        None => args.push("--reuse-window".into()),
    }
    args.push("--goto".into());
    args.push(file.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-vscode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn the_session_folder_goes_first_when_it_holds_the_file() {
        let dir = scratch("inside");
        let file = dir.join("src").join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();
        let cwd = dir.to_string_lossy().to_string();

        let folder = project_folder(&file, Some(&cwd));
        assert_eq!(folder.as_deref(), Some(dir.as_path()));
        assert_eq!(
            open_file_args(&file, folder.as_deref()),
            vec![dir.as_os_str().to_owned(), OsString::from("--goto"), file.as_os_str().to_owned()],
        );
        // Another spelling of the same folder (separators, case on Windows) still matches.
        let other = if cfg!(windows) { cwd.replace('\\', "/").to_uppercase() } else { format!("{cwd}/") };
        assert!(project_folder(&file, Some(&other)).is_some(), "{other}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anything_else_falls_back_to_the_last_window() {
        let dir = scratch("outside");
        let file = dir.join("src").join("lib.rs");
        std::fs::write(&file, "").unwrap();
        let elsewhere = scratch("elsewhere");
        let sibling = format!("{}-sibling", dir.to_string_lossy());
        std::fs::create_dir_all(&sibling).unwrap();

        for cwd in [
            None,
            Some(""),
            Some("relative/folder"),
            Some(elsewhere.to_str().unwrap()),
            // A folder whose name merely starts like the real one is not its parent.
            Some(sibling.as_str()),
            // A file is not a folder.
            Some(file.to_str().unwrap()),
            Some(dir.join("missing").to_str().unwrap()),
        ] {
            assert_eq!(project_folder(&file, cwd), None, "{cwd:?}");
        }
        assert_eq!(
            open_file_args(&file, None),
            vec![OsString::from("--reuse-window"), OsString::from("--goto"), file.as_os_str().to_owned()],
        );
        for d in [dir, elsewhere, PathBuf::from(sibling)] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// The real launch path on Windows: a batch file standing in for `code.cmd`, started
    /// the way `open_file` starts it. Every argument must arrive whole and literal, even
    /// with spaces and the characters cmd.exe would otherwise treat as syntax.
    #[cfg(windows)]
    #[test]
    fn a_batch_file_launcher_gets_each_argument_intact() {
        use std::process::Command;
        let dir = scratch("cmd");
        let project = dir.join("my project & co (v2) 100%^");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let file = project.join("src").join("cart page.ts");
        std::fs::write(&file, "").unwrap();
        let fake = dir.join("code.cmd");
        // One line per argument, quotes stripped, written without re-parsing the value.
        std::fs::write(
            &fake,
            "@echo off\r\nsetlocal DisableDelayedExpansion\r\n>\"%~dp0args.txt\" (\r\n  for %%A in (%*) do echo(%%~A\r\n)\r\n",
        )
        .unwrap();

        let folder = project_folder(&file, project.to_str()).expect("the project folder");
        let mut cmd = Command::new(&fake);
        cmd.args(open_file_args(&file, Some(&folder)));
        let status = crate::platform::no_console(&mut cmd).status().expect("the batch file runs");
        assert!(status.success());
        let got = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        let lines: Vec<&str> = got.lines().map(str::trim_end).collect();
        assert_eq!(lines, vec![project.to_str().unwrap(), "--goto", file.to_str().unwrap()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
