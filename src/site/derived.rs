//! Disposable build-only recovery of old code blocks; archived Markdown stays authoritative.

use std::io::Read as _;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use url::Url;

const NAMESPACE: &str = "derived-markdown-v1";
const MAX_CACHE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Entry {
    input: String,
    markdown: String,
    digest: String,
}

pub(super) fn markdown(
    stored: &str,
    retained_html: Option<&str>,
    base: Option<&Url>,
    cache_root: Option<&Path>,
) -> String {
    let Some(html) = retained_html else {
        return stored.to_string();
    };
    let Some(cache_root) = cache_root else {
        return crate::content::effective_markdown(stored, Some(html), base);
    };
    let input = key(stored, html, base);
    let path = cache_root.join(NAMESPACE).join(format!("{input}.json"));
    if let Some(markdown) = read(&path, &input) {
        return markdown;
    }
    let markdown = crate::content::effective_markdown(stored, Some(html), base);
    let entry = Entry {
        input,
        digest: crate::model::sha1_hex(&markdown),
        markdown,
    };
    if let Ok(bytes) = serde_json::to_vec(&entry)
        && bytes.len() as u64 <= MAX_CACHE_BYTES
    {
        let _ = crate::cache::write(&path, &bytes);
    }
    entry.markdown
}

fn read(path: &Path, input: &str) -> Option<String> {
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_CACHE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    let entry: Entry = serde_json::from_slice(&bytes).ok()?;
    (entry.input == input && entry.digest == crate::model::sha1_hex(&entry.markdown))
        .then_some(entry.markdown)
}

fn key(stored: &str, html: &str, base: Option<&Url>) -> String {
    static IMPLEMENTATION: OnceLock<String> = OnceLock::new();
    let implementation =
        IMPLEMENTATION.get_or_init(|| crate::model::sha1_hex(include_bytes!("../content.rs")));
    let mut hash = Sha1::new();
    for value in [
        NAMESPACE.as_bytes(),
        implementation.as_bytes(),
        stored.as_bytes(),
        html.as_bytes(),
        base.map(Url::as_str).unwrap_or_default().as_bytes(),
    ] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    hex::encode(hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORED: &str = "Keep this prose.\n\n```\n$ z dotfiles$ pwd/private/dotfiles\n```\n";
    const HTML: &str = "<pre><code data-lang=\"bash\"><span><span>$ z dotfiles\n</span></span><span><span>$ <span>pwd</span>\n</span></span><span><span>/private/dotfiles\n</span></span></code></pre>";

    #[test]
    fn derivation_key_tracks_every_pure_input() {
        let base = Url::parse("https://example.com/one/").unwrap();
        let original = key(STORED, HTML, Some(&base));
        assert_ne!(original, key("edited", HTML, Some(&base)));
        assert_ne!(original, key(STORED, "changed html", Some(&base)));
        assert_ne!(original, key(STORED, HTML, None));
        assert_ne!(
            original,
            key(
                STORED,
                HTML,
                Some(&Url::parse("https://example.com/two/").unwrap())
            )
        );
        assert_eq!(original, key(STORED, HTML, Some(&base)));
    }

    #[test]
    fn cache_recovers_corruption_without_modifying_archive() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("archive");
        std::fs::create_dir(&archive).unwrap();
        std::fs::write(archive.join("item.md"), STORED).unwrap();
        std::fs::write(archive.join("item.html"), HTML).unwrap();
        let cache = root.path().join("cache");
        let expected = crate::content::effective_markdown(STORED, Some(HTML), None);
        assert!(expected.contains("$ z dotfiles\n$ pwd\n/private/dotfiles\n"));
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&cache)), expected);
        let path = cache
            .join(NAMESPACE)
            .join(format!("{}.json", key(STORED, HTML, None)));
        assert!(path.is_file());
        let mut entry: Entry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(entry.markdown, expected);
        let unchanged = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(unchanged)
            .unwrap();
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&cache)), expected);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            unchanged
        );

        entry.markdown = "changed".into();
        std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&cache)), expected);
        let repaired: Entry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(repaired.markdown, expected);
        std::fs::write(&path, "broken JSON").unwrap();
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&cache)), expected);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_CACHE_BYTES + 1)
            .unwrap();
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&cache)), expected);
        assert!(std::fs::metadata(&path).unwrap().len() < MAX_CACHE_BYTES);
        assert_eq!(
            std::fs::read_to_string(archive.join("item.md")).unwrap(),
            STORED
        );
        assert_eq!(
            std::fs::read_to_string(archive.join("item.html")).unwrap(),
            HTML
        );
    }

    #[test]
    fn absent_html_and_unavailable_cache_are_harmless() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("absent");
        assert_eq!(markdown(STORED, None, None, Some(&absent)), STORED);
        assert!(!absent.exists());
        let file = root.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        let expected = crate::content::effective_markdown(STORED, Some(HTML), None);
        assert_eq!(markdown(STORED, Some(HTML), None, Some(&file)), expected);
        assert_eq!(markdown(STORED, Some(HTML), None, None), expected);
    }
}
