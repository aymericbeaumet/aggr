//! Bounded PDF companions. These are opaque publisher bytes, never stored HTML.

use anyhow::{Result, bail};
use url::Url;

use crate::model::{Document, sha1_hex};

pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub source_url: String,
    pub bytes: Vec<u8>,
}

impl Asset {
    pub fn metadata(&self, stem: &str) -> Document {
        Document {
            file: format!("{stem}.document-{}.pdf", &sha1_hex(&self.bytes)[..12]),
            source: self.source_url.clone(),
        }
    }
}

pub fn validate_stored(bytes: &[u8], metadata: &Document, stem: &str) -> Result<()> {
    if !metadata.is_valid_for(stem) {
        bail!("invalid document companion metadata");
    }
    if bytes.len() > MAX_DOCUMENT_BYTES || !bytes.starts_with(b"%PDF-") {
        bail!("document companion is not a bounded PDF");
    }
    let expected = format!("{stem}.document-{}.pdf", &sha1_hex(bytes)[..12]);
    if metadata.file != expected {
        bail!("document companion hash does not match its bytes");
    }
    Ok(())
}

pub fn url(
    link: &str,
    extra: &std::collections::BTreeMap<String, serde_yaml_ng::Value>,
) -> Option<Url> {
    let candidate = |link: &str| Url::parse(link).ok().filter(crate::preview::is_pdf_url);
    candidate(link)
        .or_else(|| candidate(extra.get("document_url")?.as_str()?))
        .map(|mut url| {
            url.set_fragment(None);
            url
        })
}

pub fn matches_item(asset: &Asset, item: &crate::model::Item) -> bool {
    Url::parse(&asset.source_url).ok() == url(&item.front.link, &item.front.extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_validation_rejects_html_mismatched_hashes_and_unsafe_metadata() {
        let asset = Asset {
            source_url: "https://publisher.example/paper.pdf".into(),
            bytes: b"%PDF-1.7\nfixture".to_vec(),
        };
        let metadata = asset.metadata("paper");
        validate_stored(&asset.bytes, &metadata, "paper").unwrap();
        assert!(validate_stored(b"<html>challenge</html>", &metadata, "paper").is_err());
        assert!(validate_stored(b"%PDF-other", &metadata, "paper").is_err());
        assert!(!metadata.is_valid_for("other"));
        for file in [
            "../paper.document-0123456789ab.pdf",
            "paper.document-0123456789ab.html",
            "paper.document-0123456789ab.pdf/other",
        ] {
            assert!(
                !Document {
                    file: file.into(),
                    ..metadata.clone()
                }
                .is_valid_for("paper")
            );
        }
        for source in [
            "javascript:alert(1)",
            "https://user:pass@example.com/a.pdf",
            "https://example.com/a.pdf#fragment",
        ] {
            assert!(
                !Document {
                    source: source.into(),
                    ..metadata.clone()
                }
                .is_valid_for("paper")
            );
        }
        let mut oversized = vec![b' '; MAX_DOCUMENT_BYTES + 1];
        oversized[..5].copy_from_slice(b"%PDF-");
        assert!(validate_stored(&oversized, &metadata, "paper").is_err());
    }
}
