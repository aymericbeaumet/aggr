//! BCP 47 language tags: well-formedness per RFC 5646 syntax, without registry validation.

use std::collections::BTreeSet;

/// Whether `tag` is a well-formed BCP 47 language tag such as `en`, `fr-FR`, or `zh-Hant-TW`.
pub(crate) fn is_well_formed(tag: &str) -> bool {
    const GRANDFATHERED: &[&str] = &[
        "art-lojban",
        "cel-gaulish",
        "en-gb-oed",
        "i-ami",
        "i-bnn",
        "i-default",
        "i-enochian",
        "i-hak",
        "i-klingon",
        "i-lux",
        "i-mingo",
        "i-navajo",
        "i-pwn",
        "i-tao",
        "i-tay",
        "i-tsu",
        "no-bok",
        "no-nyn",
        "sgn-be-fr",
        "sgn-be-nl",
        "sgn-ch-de",
        "zh-guoyu",
        "zh-hakka",
        "zh-min",
        "zh-min-nan",
        "zh-xiang",
    ];

    if !tag.is_ascii() {
        return false;
    }
    if GRANDFATHERED
        .iter()
        .any(|known| tag.eq_ignore_ascii_case(known))
    {
        return true;
    }

    let subtags = tag.split('-').collect::<Vec<_>>();
    if subtags.iter().any(|part| {
        part.is_empty() || part.len() > 8 || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return false;
    }
    let Some(language) = subtags.first().copied() else {
        return false;
    };
    if language.eq_ignore_ascii_case("x") {
        return subtags.len() > 1;
    }
    if !(2..=8).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }

    let mut index = 1;
    if language.len() <= 3 {
        for _ in 0..3 {
            if subtags.get(index).is_some_and(|part| {
                part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_alphabetic())
            }) {
                index += 1;
            } else {
                break;
            }
        }
    }
    if subtags
        .get(index)
        .is_some_and(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        index += 1;
    }
    if subtags.get(index).is_some_and(|part| {
        (part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
            || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) {
        index += 1;
    }

    let mut variants = BTreeSet::new();
    while let Some(part) = subtags.get(index).copied()
        && ((5..=8).contains(&part.len())
            || (part.len() == 4 && part.as_bytes()[0].is_ascii_digit()))
    {
        if !variants.insert(part.to_ascii_lowercase()) {
            return false;
        }
        index += 1;
    }

    let mut singletons = BTreeSet::new();
    while let Some(singleton) = subtags
        .get(index)
        .copied()
        .filter(|part| part.len() == 1 && !part.eq_ignore_ascii_case("x"))
    {
        if !singletons.insert(singleton.to_ascii_lowercase()) {
            return false;
        }
        index += 1;
        let start = index;
        while subtags
            .get(index)
            .is_some_and(|part| (2..=8).contains(&part.len()))
        {
            index += 1;
        }
        if index == start {
            return false;
        }
    }

    if subtags
        .get(index)
        .is_some_and(|part| part.eq_ignore_ascii_case("x"))
    {
        index += 1;
        if index == subtags.len() {
            return false;
        }
        index = subtags.len();
    }
    index == subtags.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_tags_follow_the_bcp_47_grammar() {
        for tag in [
            "en",
            "fr-FR",
            "zh-Hant-TW",
            "zh-cmn-Hans-CN",
            "de-CH-1901",
            "en-US-u-ca-gregory",
            "x-reader-local",
            "i-klingon",
        ] {
            assert!(is_well_formed(tag), "rejected {tag:?}");
        }

        for tag in [
            "",
            "e",
            "en_US",
            "en--US",
            "en-",
            "en-abcdefghi",
            "en-u",
            "en-u-ca-u-nu",
            "de-1901-1901",
            "x",
            "123",
            "fr-É",
        ] {
            assert!(!is_well_formed(tag), "accepted {tag:?}");
        }
    }
}
