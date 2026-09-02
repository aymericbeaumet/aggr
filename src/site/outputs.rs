//! Non-template outputs: syndicated feeds and redirect stubs.

use anyhow::Result;

use super::context::{BuildCtx, ItemCtx, SiteCtx};

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
    atom_collection(site, build, &site.title, "", items)
}

pub fn atom_collection(
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> String {
    let root = site
        .base_url
        .clone()
        .unwrap_or_else(|| site.base_path.clone());
    let collection = format!("{root}{path}");
    let mut out = String::with_capacity(4096);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!("  <title>{}</title>\n", escape(title)));
    if !site.description.is_empty() {
        out.push_str(&format!(
            "  <subtitle>{}</subtitle>\n",
            escape(&site.description)
        ));
    }
    out.push_str(&format!("  <id>{}</id>\n", escape(&collection)));
    out.push_str(&format!(
        "  <link rel=\"alternate\" href=\"{}\"/>\n",
        escape(&collection)
    ));
    out.push_str(&format!(
        "  <link rel=\"self\" href=\"{}atom.xml\"/>\n",
        escape(&collection)
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

pub fn rss_collection(
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> String {
    let root = site
        .base_url
        .clone()
        .unwrap_or_else(|| site.base_path.clone());
    let collection = format!("{root}{path}");
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\"><channel>\n",
    );
    out.push_str(&format!("  <title>{}</title>\n", escape(title)));
    out.push_str(&format!("  <link>{}</link>\n", escape(&collection)));
    out.push_str(&format!(
        "  <description>{}</description>\n",
        escape(&site.description)
    ));
    out.push_str(&format!(
        "  <lastBuildDate>{}</lastBuildDate>\n",
        build.time.to_rfc2822()
    ));
    out.push_str("  <generator>aggr</generator>\n");
    for item in items {
        out.push_str("  <item>\n");
        out.push_str(&format!("    <title>{}</title>\n", escape(&item.title)));
        out.push_str(&format!("    <link>{}</link>\n", escape(&item.link)));
        out.push_str(&format!(
            "    <guid isPermaLink=\"false\">{}{}</guid>\n",
            escape(&root),
            escape(&item.url)
        ));
        out.push_str(&format!(
            "    <pubDate>{}</pubDate>\n",
            item.date.to_rfc2822()
        ));
        if !item.excerpt.is_empty() {
            out.push_str(&format!(
                "    <description>{}</description>\n",
                escape(&item.excerpt)
            ));
        }
        for label in &item.labels {
            out.push_str(&format!("    <category>{}</category>\n", escape(label)));
        }
        out.push_str("  </item>\n");
    }
    out.push_str("</channel></rss>\n");
    out
}

pub fn json_collection(
    site: &SiteCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> Result<String> {
    let root = site
        .base_url
        .clone()
        .unwrap_or_else(|| site.base_path.clone());
    let collection = format!("{root}{path}");
    let entries: Vec<_> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": format!("{root}{}", item.url),
                "url": item.link,
                "external_url": item.link,
                "title": item.title,
                "summary": item.excerpt,
                "date_published": item.date.to_rfc3339(),
                "date_modified": item.updated.map(|date| date.to_rfc3339()),
                "authors": item.authors.iter().map(|name| serde_json::json!({"name": name})).collect::<Vec<_>>(),
                "tags": item.labels,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "version": "https://jsonfeed.org/version/1.1",
        "title": title,
        "home_page_url": collection,
        "feed_url": format!("{root}{path}feed.json"),
        "description": site.description,
        "items": entries,
    }))?)
}

pub fn item_json(site: &SiteCtx, item: &ItemCtx, markdown: &str) -> Result<String> {
    let root = site
        .base_url
        .clone()
        .unwrap_or_else(|| site.base_path.clone());
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "id": item.path,
        "url": format!("{root}{}", item.url),
        "external_url": item.link,
        "title": item.title,
        "source": item.source,
        "source_name": item.source_name,
        "category": item.category,
        "tags": item.labels,
        "date_created": item.date.to_rfc3339(),
        "date_published": item.published.map(|date| date.to_rfc3339()),
        "date_modified": item.updated.map(|date| date.to_rfc3339()),
        "authors": item.authors,
        "summary": item.summary,
        "content_markdown": markdown,
    }))?)
}

pub fn text_item(item: &ItemCtx) -> String {
    let body = item
        .body_html
        .as_deref()
        .map(crate::content::html_to_text)
        .unwrap_or_default();
    format!("{}\n\n{}\n", item.title, body.trim())
}

pub fn rst_item(item: &ItemCtx) -> String {
    let title = &item.title;
    let underline = "=".repeat(title.chars().count().max(1));
    format!(
        "{title}\n{underline}\n\n{}",
        text_item(item)
            .split_once("\n\n")
            .map_or("", |(_, body)| body)
    )
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
