use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Serialize)]
pub struct DocumentCtx {
    pub url: String,
    pub local_url: Option<String>,
}

impl DocumentCtx {
    pub fn from_item(item: &crate::model::Item) -> Option<Self> {
        Self::from_url(&item.front.link)
            .or_else(|| Self::from_url(item.front.extra.get("document_url")?.as_str()?))
    }

    pub fn from_url(link: &str) -> Option<Self> {
        let url = Url::parse(link).ok()?;
        crate::preview::is_pdf_url(&url).then(|| Self {
            url: url.into(),
            local_url: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_pdf_urls_and_preserves_parameters() {
        for link in [
            "https://cims.nyu.edu/~tristanb/statement.pdf",
            "https://example.com/paper.PDF?download=1#page=3",
            "https://example.com/paper%2Epdf",
            "https://example.com/download?filename=Paper%20One.PDF&token=secret",
            "http://localhost:3000/paper.pdf",
        ] {
            assert_eq!(DocumentCtx::from_url(link).unwrap().url, link);
        }
    }

    #[test]
    fn rejects_unsafe_urls_and_unrelated_pdf_mentions() {
        for link in [
            "javascript:alert('paper.pdf')",
            "data:application/pdf;base64,AAAA",
            "file:///paper.pdf",
            "//example.com/paper.pdf",
            "/paper.pdf",
            "https://user:password@example.com/paper.pdf",
            "https://example.com/paper.pdf.html",
            "https://example.com/paper.pdf/notes",
            "https://example.com/article?url=https://other.example/paper.pdf",
            "https://example.com/article#paper.pdf",
            "https://example.com/download?filename=paper.pdf.exe",
        ] {
            assert!(DocumentCtx::from_url(link).is_none(), "{link}");
        }
    }
}
