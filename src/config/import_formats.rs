//! Subscription documents reduced to the same entries as `[[sources]]`.

use anyhow::{Context, Result, bail};
use quick_xml::{Reader, events::Event};
use serde::Deserialize;
use url::Url;

use super::{SourceConfig, deserialize_sources, source_entries::split_lines};

pub(super) struct Document {
    pub sources: Vec<SourceConfig>,
    pub collection: bool,
}

#[derive(Debug)]
pub(super) struct InvalidCollection;

impl std::fmt::Display for InvalidCollection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid source collection")
    }
}

impl std::error::Error for InvalidCollection {}

pub(super) fn parse(text: &str, document_url: Option<&Url>) -> Result<Document> {
    let result = parse_document(text, document_url);
    let text = text.trim_start_matches('\u{feff}').trim();
    let collection = if text.starts_with('<') {
        xml_root(text).map_or_else(|_| text.contains("<opml"), |root| root == b"opml")
    } else {
        !text.starts_with('{')
    };
    if collection {
        result.context(InvalidCollection)
    } else {
        result
    }
}

pub(super) fn parse_bytes(bytes: &[u8], document_url: &Url) -> Result<Document> {
    let encoding = encoding_rs::Encoding::for_bom(bytes).map(|(encoding, _)| encoding);
    if encoding.is_none()
        && let Ok(text) = std::str::from_utf8(bytes)
    {
        return parse(text, Some(document_url));
    }
    let encoding = match encoding {
        Some(encoding) => encoding,
        None => {
            let mut reader = Reader::from_reader(bytes);
            let mut buffer = Vec::new();
            let label = match reader.read_event_into(&mut buffer)? {
                Event::Decl(declaration) => declaration
                    .encoding()
                    .transpose()?
                    .map(|value| value.into_owned()),
                _ => None,
            }
            .context("source document is not UTF-8 and has no XML encoding declaration")?;
            encoding_rs::Encoding::for_label(&label)
                .context("unsupported source document encoding")?
        }
    };
    let (text, _, malformed) = encoding.decode(bytes);
    if malformed {
        bail!("source document contains invalid encoded characters");
    }
    parse(&text, Some(document_url))
}

fn parse_document(text: &str, document_url: Option<&Url>) -> Result<Document> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        bail!("import contains no sources");
    }
    let mut collection = true;
    let sources = if text.starts_with('<') {
        if xml_root(text)? == b"opml" {
            opml_sources(text)?
        } else {
            collection = false;
            feed_source(text, document_url)?
        }
    } else if text.starts_with('{') {
        collection = false;
        feed_source(text, document_url)?
    } else if looks_like_list(text) {
        list_sources(text)?
    } else {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct ImportedConfig {
            #[serde(deserialize_with = "deserialize_sources")]
            sources: Vec<SourceConfig>,
        }
        toml::from_str::<ImportedConfig>(text)
            .context("parsing imported aggr TOML")?
            .sources
    };
    if sources.is_empty() {
        bail!("import contains no sources");
    }
    Ok(Document {
        sources,
        collection,
    })
}

fn looks_like_list(text: &str) -> bool {
    split_lines(text).next().is_some_and(|line| {
        line.starts_with("https://")
            || line.starts_with("http://")
            || (!line.starts_with(['[', '#']) && !line.contains('='))
    })
}

fn list_sources(text: &str) -> Result<Vec<SourceConfig>> {
    split_lines(text)
        .map(|line| {
            if line.starts_with('#') {
                bail!("source lists contain one resource per line; use [[sources]] tables for metadata");
            }
            if line.contains("://") {
                let url = Url::parse(line).context("invalid URL in source list")?;
                if !matches!(url.scheme(), "http" | "https") || line.chars().any(char::is_whitespace) {
                    bail!("remote sources must be HTTP(S) URLs without whitespace");
                }
            }
            Ok(SourceConfig { url: Some(line.to_string()), ..Default::default() })
        })
        .collect()
}

fn xml_root(text: &str) -> Result<Vec<u8>> {
    let mut reader = Reader::from_str(text);
    loop {
        match reader.read_event().context("parsing imported XML")? {
            Event::Start(element) | Event::Empty(element) => {
                return Ok(element.local_name().as_ref().to_vec());
            }
            Event::Eof => bail!("imported XML has no root element"),
            _ => {}
        }
    }
}

fn opml_sources(text: &str) -> Result<Vec<SourceConfig>> {
    let mut reader = Reader::from_str(text);
    let mut categories: Vec<Option<String>> = Vec::new();
    let mut sources = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        let event = reader.read_event().context("parsing imported OPML")?;
        if depth == 0 && matches!(event, Event::Start(_) | Event::Empty(_)) {
            roots += 1;
            if roots > 1 {
                bail!("OPML must contain exactly one root element");
            }
        }
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"outline" =>
            {
                let mut url = None;
                let mut name = None;
                let mut title = None;
                let mut category = None;
                for attribute in element.attributes() {
                    let attribute = attribute.context("reading OPML outline attributes")?;
                    let value = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )?
                        .trim()
                        .to_string();
                    if value.is_empty() {
                        continue;
                    }
                    match attribute.key.as_ref() {
                        b"xmlUrl" => url = Some(value),
                        b"text" => name = Some(value),
                        b"title" => title = Some(value),
                        b"category" => category = Some(value),
                        _ => {}
                    }
                }
                let name = title.or(name);
                let folder = if let Some(url) = url {
                    let inherited = categories
                        .iter()
                        .filter_map(|part| part.as_deref())
                        .collect::<Vec<_>>()
                        .join("/");
                    sources.push(SourceConfig {
                        url: Some(url),
                        name,
                        category: category.or_else(|| (!inherited.is_empty()).then_some(inherited)),
                        ..Default::default()
                    });
                    None
                } else {
                    name
                };
                if !empty {
                    categories.push(folder);
                    depth += 1;
                }
            }
            Event::Start(_) => depth += 1,
            Event::End(element) => {
                depth = depth
                    .checked_sub(1)
                    .context("unexpected closing OPML element")?;
                if element.local_name().as_ref() == b"outline" {
                    categories.pop();
                }
            }
            Event::Eof => {
                if depth != 0 {
                    bail!("unclosed OPML element");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(sources)
}

fn feed_source(text: &str, document_url: Option<&Url>) -> Result<Vec<SourceConfig>> {
    feed_rs::parser::parse(text.as_bytes())
        .context("import is not a supported RSS, Atom, or JSON Feed document")?;
    let url = document_url.context("feed import requires a document location")?;
    let source = if url.scheme() == "file" {
        SourceConfig {
            local_feed: Some(
                url.to_file_path()
                    .map_err(|_| anyhow::anyhow!("invalid local feed path"))?,
            ),
            ..Default::default()
        }
    } else {
        SourceConfig {
            url: Some(url.to_string()),
            ..Default::default()
        }
    };
    Ok(vec![source])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_sources(text: &str, url: Option<&Url>) -> Result<Vec<SourceConfig>> {
        parse(text, url).map(|document| document.sources)
    }

    #[test]
    fn declared_legacy_xml_encodings_and_utf16_are_supported() {
        let url = Url::parse("file:///tmp/feed.xml").unwrap();
        let bytes = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss version=\"2.0\"><channel><title>Caf\xe9</title><link>https://example.org/</link><description>News</description></channel></rss>";
        assert!(!parse_bytes(bytes, &url).unwrap().collection);
        let text =
            r#"<opml><body><outline text="Café" xmlUrl="https://example.org/feed"/></body></opml>"#;
        let bytes: Vec<_> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(
            parse_bytes(&bytes, &url).unwrap().sources[0]
                .name
                .as_deref(),
            Some("Café")
        );
    }

    #[test]
    fn malformed_opml_is_a_collection_error_even_with_an_unclosed_root() {
        assert!(
            parse("<opml", None)
                .err()
                .unwrap()
                .is::<InvalidCollection>()
        );
        assert!(
            parse(
                "<opml><body><outline xmlUrl='https://example.org/feed'/></body></opml><extra/>",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn opml_preserves_outline_order_metadata_and_entities() {
        let sources = parse_sources(
            r#"<?xml version="1.0"?><opml version="2.0"><body>
              <outline text="Science"><outline text="Space &amp; time" xmlUrl="https://example.org/rss?a=1&amp;b=2"/>
                <outline text="Physics"><outline title="Particles" xmlUrl="https://example.net/atom"/></outline>
              </outline><outline text="Other" xmlUrl="https://other.example/feed"/>
            </body></opml>"#,
            None,
        ).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(
            sources[0].url.as_deref(),
            Some("https://example.org/rss?a=1&b=2")
        );
        assert_eq!(sources[0].name.as_deref(), Some("Space & time"));
        assert_eq!(sources[0].category.as_deref(), Some("Science"));
        assert_eq!(sources[1].category.as_deref(), Some("Science/Physics"));
        assert_eq!(sources[2].category, None);
    }

    #[test]
    fn plain_lists_share_line_normalization() {
        let sources = parse_sources(
            "  https://a.example/feed\r\n\n\thttps://b.example/rss  \n",
            None,
        )
        .unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].url.as_deref(), Some("https://a.example/feed"));
        assert_eq!(sources[1].url.as_deref(), Some("https://b.example/rss"));
        assert!(parse_sources("https://a.example/feed\n#[category='science']", None).is_err());
        let sources = parse_sources(
            " nested.toml\n./local feeds.opml\nhttps://a.example/feed\n",
            None,
        )
        .unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|source| source.url.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "nested.toml",
                "./local feeds.opml",
                "https://a.example/feed"
            ]
        );
    }

    #[test]
    fn toml_uses_the_shared_source_deserializer() {
        let sources = parse_sources(
            r#"[[sources]]
url = [" https://a.example/\nhttps://b.example/ ", "https://c.example/"]
"#,
            None,
        )
        .unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[1].url.as_deref(), Some("https://b.example/"));
    }

    #[test]
    fn remote_feed_formats_register_the_document_url() {
        let url = Url::parse("https://example.org/feed").unwrap();
        for document in [
            r#"<rss version="2.0"><channel><title>RSS</title><link>https://example.org/</link><description>News</description></channel></rss>"#,
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><id>urn:example</id><title>Atom</title><updated>2026-01-01T00:00:00Z</updated></feed>"#,
            r#"{"version":"https://jsonfeed.org/version/1.1","title":"JSON","items":[]}"#,
        ] {
            let sources = parse_sources(document, Some(&url)).unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(sources[0].url.as_deref(), Some(url.as_str()));
        }
    }

    #[test]
    fn local_feeds_register_the_document_path() {
        let url = Url::parse("file:///tmp/news.json").unwrap();
        let sources = parse_sources(
            r#"{"version":"https://jsonfeed.org/version/1.1","title":"JSON","items":[]}"#,
            Some(&url),
        )
        .unwrap();
        assert_eq!(
            sources[0].local_feed.as_deref(),
            Some(std::path::Path::new("/tmp/news.json"))
        );
        assert_eq!(sources[0].url, None);
    }

    #[test]
    fn malformed_and_unrecognized_documents_are_rejected() {
        for text in [
            "",
            "<html><body>Not a feed</body></html>",
            "<opml><body><outline",
            "[site]\ntitle='No sources'",
            "sources_urls = ['https://example.org/']",
        ] {
            assert!(parse_sources(text, None).is_err(), "accepted {text:?}");
        }
    }
}
