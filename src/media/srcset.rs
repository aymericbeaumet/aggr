//! Tokenize source sets without treating commas inside a URL as separators.

#[derive(Debug, PartialEq)]
pub struct Candidate<'a> {
    pub url: &'a str,
    pub score: f64,
}

/// HTML source-set URLs extend to ASCII whitespace. Only a trailing comma, or a comma
/// following the descriptors, separates candidates; CDN transformation commas stay in URLs.
pub fn candidates(input: &str) -> Vec<Candidate<'_>> {
    let bytes = input.as_bytes();
    let mut position = 0;
    let mut candidates = Vec::new();
    while position < bytes.len() {
        while position < bytes.len()
            && (bytes[position].is_ascii_whitespace() || bytes[position] == b',')
        {
            position += 1;
        }
        let start = position;
        while position < bytes.len() && !bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        let raw_url = &input[start..position];
        if raw_url.is_empty() {
            break;
        }
        if raw_url.ends_with(',') {
            let url = raw_url.trim_end_matches(',');
            if !url.is_empty() {
                candidates.push(Candidate { url, score: 1.0 });
            }
            continue;
        }
        let mut descriptors = Vec::new();
        while position < bytes.len() {
            while position < bytes.len() && bytes[position].is_ascii_whitespace() {
                position += 1;
            }
            if position == bytes.len() {
                break;
            }
            if bytes[position] == b',' {
                position += 1;
                break;
            }
            let start = position;
            let mut parentheses = 0usize;
            while position < bytes.len() {
                let byte = bytes[position];
                if parentheses == 0 && (byte.is_ascii_whitespace() || byte == b',') {
                    break;
                }
                if byte == b'(' {
                    parentheses += 1;
                } else if byte == b')' {
                    parentheses = parentheses.saturating_sub(1);
                }
                position += 1;
            }
            descriptors.push(&input[start..position]);
        }
        if let Some(score) = descriptor_score(&descriptors) {
            candidates.push(Candidate {
                url: raw_url,
                score,
            });
        }
    }
    candidates
}

fn descriptor_score(descriptors: &[&str]) -> Option<f64> {
    let mut width = None;
    let mut density = None;
    let mut height = None;
    for descriptor in descriptors {
        if let Some(number) = descriptor.strip_suffix('w') {
            if width.is_some() || density.is_some() {
                return None;
            }
            width = Some(positive_integer(number)?);
        } else if let Some(number) = descriptor.strip_suffix('x') {
            if width.is_some() || density.is_some() || height.is_some() || number.starts_with('+') {
                return None;
            }
            let parsed = number.parse::<f64>().ok()?;
            if !parsed.is_finite() || parsed < 0.0 {
                return None;
            }
            density = Some(parsed);
        } else if let Some(number) = descriptor.strip_suffix('h') {
            if height.is_some() || density.is_some() {
                return None;
            }
            height = Some(positive_integer(number)?);
        } else {
            return None;
        }
    }
    if height.is_some() && width.is_none() {
        return None;
    }
    Some(width.map(|width| width as f64).or(density).unwrap_or(1.0))
}

fn positive_integer(input: &str) -> Option<u64> {
    if input.is_empty() || !input.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    input.parse().ok().filter(|number| *number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdn_commas_are_preserved_in_complete_urls() {
        let first = "https://substackcdn.com/image/fetch/w_424,c_limit,f_webp,q_auto:good,fl_progressive:steep/https%3A%2F%2Fexample.com%2Fphoto.png";
        let second = first.replace("w_424", "w_1456");
        let input = format!("{first} 424w, {second} 1456w");
        assert_eq!(
            candidates(&input),
            [
                Candidate {
                    url: first,
                    score: 424.0
                },
                Candidate {
                    url: &second,
                    score: 1456.0
                },
            ]
        );
    }

    #[test]
    fn descriptors_separate_candidates_and_invalid_entries_are_discarded() {
        let input = "one.png, two.png 2x, bad.png 1x 2x, bad2.png 0w, bad3.png NaNx, bad4.png calc(1, 2), final.png 800w 400h";
        assert_eq!(
            candidates(input),
            [
                Candidate {
                    url: "one.png",
                    score: 1.0
                },
                Candidate {
                    url: "two.png",
                    score: 2.0
                },
                Candidate {
                    url: "final.png",
                    score: 800.0
                },
            ]
        );
        assert_eq!(candidates("one.png,two.png")[0].url, "one.png,two.png");
    }
}
