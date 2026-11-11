// "Copy value" from the Vault table: the value goes from the vault straight to the
// OS clipboard on the Rust side (it never travels to the webview), and is cleared again
// after a delay, but only if the clipboard still holds that very value. Whatever the
// user copied in the meantime is never touched.
//
// The clipboard is behind a trait so the clearing logic is tested without an OS clipboard.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long a copied value stays on the clipboard.
pub const CLEAR_AFTER: Duration = Duration::from_secs(30);

pub trait Clip: Send + Sync + 'static {
    /// Current clipboard text; `None` when it is empty or holds something else (an image, files).
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str) -> Result<(), String>;
}

/// The real clipboard, through the Tauri plugin.
pub struct SystemClip(pub tauri::AppHandle);

impl Clip for SystemClip {
    fn read(&self) -> Option<String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.0.clipboard().read_text().ok()
    }
    fn write(&self, text: &str) -> Result<(), String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.0.clipboard().write_text(text.to_string()).map_err(|e| format!("Couldn't write the clipboard: {e}"))
    }
}

/// Counts copies, so a newer copy cancels an older copy's pending clear (copying the same
/// value twice must not let the first timer wipe the second copy early).
pub fn generation() -> &'static Arc<AtomicU64> {
    static GEN: std::sync::OnceLock<Arc<AtomicU64>> = std::sync::OnceLock::new();
    GEN.get_or_init(|| Arc::new(AtomicU64::new(0)))
}

/// Writes `value` to the clipboard and arranges for it to be cleared after `delay`.
pub fn copy_and_arm(clip: Arc<dyn Clip>, gen: Arc<AtomicU64>, value: String, delay: Duration) -> Result<(), String> {
    clip.write(&value)?;
    let mine = gen.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        clear_if_unchanged(clip.as_ref(), &gen, mine, &value);
    });
    Ok(())
}

/// Clears the clipboard if it still holds `value` and no newer copy was made. Returns
/// whether it cleared.
pub fn clear_if_unchanged(clip: &dyn Clip, gen: &AtomicU64, mine: u64, value: &str) -> bool {
    if gen.load(Ordering::SeqCst) != mine || clip.read().as_deref() != Some(value) {
        return false;
    }
    clip.write("").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake(Mutex<Option<String>>);
    impl Clip for Fake {
        fn read(&self) -> Option<String> {
            self.0.lock().unwrap().clone().filter(|s| !s.is_empty())
        }
        fn write(&self, text: &str) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn clears_the_value_it_copied() {
        let clip = Arc::new(Fake::default());
        let gen = Arc::new(AtomicU64::new(0));
        copy_and_arm(clip.clone(), gen, "s3cret-value".into(), Duration::from_millis(40)).unwrap();
        assert_eq!(clip.read().as_deref(), Some("s3cret-value"));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(clip.read(), None);
    }

    #[test]
    fn never_wipes_something_else_the_user_copied() {
        let clip = Arc::new(Fake::default());
        let gen = Arc::new(AtomicU64::new(0));
        copy_and_arm(clip.clone(), gen, "s3cret-value".into(), Duration::from_millis(60)).unwrap();
        clip.write("my shopping list").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(clip.read().as_deref(), Some("my shopping list"));
    }

    #[test]
    fn a_newer_copy_of_the_same_value_keeps_its_full_time() {
        let clip = Arc::new(Fake::default());
        let gen = Arc::new(AtomicU64::new(0));
        copy_and_arm(clip.clone(), gen.clone(), "s3cret-value".into(), Duration::from_millis(60)).unwrap();
        copy_and_arm(clip.clone(), gen, "s3cret-value".into(), Duration::from_millis(900)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(clip.read().as_deref(), Some("s3cret-value"), "the first timer must not clear the second copy");
        std::thread::sleep(Duration::from_millis(1000));
        assert_eq!(clip.read(), None);
    }

    #[test]
    fn clear_checks_the_value_directly() {
        let clip = Fake::default();
        let gen = AtomicU64::new(1);
        clip.write("a").unwrap();
        assert!(!clear_if_unchanged(&clip, &gen, 1, "b"));
        assert!(!clear_if_unchanged(&clip, &gen, 0, "a"));
        assert!(clear_if_unchanged(&clip, &gen, 1, "a"));
    }
}
