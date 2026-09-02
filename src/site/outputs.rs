//! Non-template outputs: the search index, the Atom re-syndication feed, redirect stubs.

use anyhow::Result;
use serde::Serialize;

use super::context::{BuildCtx, ItemCtx, SiteCtx};

#[derive(Serialize)]
struct SearchEntry<'a> {
    path: &'a str,
    url: &'a str,
    title: &'a str,
    source: &'a str,
    source_name: &'a str,
    category: Option<&'a str>,
    date: &'a chrono::DateTime<chrono::Utc>,
    link: &'a str,
    domain: &'a str,
    excerpt: &'a str,
}

pub fn search_json(items: &[ItemCtx]) -> Result<String> {
    let entries: Vec<SearchEntry<'_>> = items
        .iter()
        .map(|item| SearchEntry {
            path: &item.path,
            url: &item.url,
            title: &item.title,
            source: &item.source,
            source_name: &item.source_name,
            category: item.category.as_deref(),
            date: &item.date,
            link: &item.link,
            domain: &item.domain,
            excerpt: &item.excerpt,
        })
        .collect();
    Ok(serde_json::to_string(&entries)?)
}

/// A ~200 byte page that sends old URLs to the item's GitHub permalink.
pub fn redirect_stub(target: &str) -> String {
    let target = escape(target);
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={target}\">\
         <link rel=\"canonical\" href=\"{target}\"><title>Moved</title><a href=\"{target}\">Moved</a>\n"
    )
}

/// Atom feed of the river's first page, so the site itself can be followed.
pub fn atom_feed(site: &SiteCtx, build: &BuildCtx, items: &[ItemCtx]) -> String {
    let root = site
        .base_url
        .clone()
        .unwrap_or_else(|| site.base_path.clone());
    let mut out = String::with_capacity(4096);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!("  <title>{}</title>\n", escape(&site.title)));
    if !site.description.is_empty() {
        out.push_str(&format!(
            "  <subtitle>{}</subtitle>\n",
            escape(&site.description)
        ));
    }
    out.push_str(&format!("  <id>{}</id>\n", escape(&root)));
    out.push_str(&format!(
        "  <link rel=\"alternate\" href=\"{}\"/>\n",
        escape(&root)
    ));
    out.push_str(&format!(
        "  <link rel=\"self\" href=\"{}feed.xml\"/>\n",
        escape(&root)
    ));
    out.push_str(&format!(
        "  <updated>{}</updated>\n",
        build.time.to_rfc3339()
    ));
    out.push_str(&format!(
        "  <generator version=\"{}\">aggr</generator>\n",
        build.version
    ));
    for item in items {
        let page = format!("{root}{}", item.url);
        out.push_str("  <entry>\n");
        out.push_str(&format!("    <id>{}</id>\n", escape(&page)));
        out.push_str(&format!("    <title>{}</title>\n", escape(&item.title)));
        out.push_str(&format!(
            "    <link rel=\"alternate\" href=\"{}\"/>\n",
            escape(&item.link)
        ));
        out.push_str(&format!(
            "    <link rel=\"related\" href=\"{}\"/>\n",
            escape(&page)
        ));
        out.push_str(&format!(
            "    <updated>{}</updated>\n",
            item.date.to_rfc3339()
        ));
        if let Some(published) = item.published {
            out.push_str(&format!(
                "    <published>{}</published>\n",
                published.to_rfc3339()
            ));
        }
        for author in &item.authors {
            out.push_str(&format!(
                "    <author><name>{}</name></author>\n",
                escape(author)
            ));
        }
        out.push_str(&format!(
            "    <source><title>{}</title></source>\n",
            escape(&item.source_name)
        ));
        if !item.excerpt.is_empty() {
            out.push_str(&format!(
                "    <summary>{}</summary>\n",
                escape(&item.excerpt)
            ));
        }
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_escapes_and_redirects() {
        let stub = redirect_stub("https://github.com/o/r/blob/abc/items/x/a.md?a=1&b=\"2\"");
        assert!(stub.contains("http-equiv=\"refresh\""));
        assert!(stub.contains("&amp;b=&quot;2&quot;"));
        assert!(!stub.contains("&b=\"2\""));
        assert!(stub.len() < 400);
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(
            escape("a < b & c > \"d\""),
            "a &lt; b &amp; c &gt; &quot;d&quot;"
        );
    }
}
