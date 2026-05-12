//! Pure helpers — ports of `podimo/utils.py` and `main.py` small functions.
//!
//! Mirrors:
//!   - `randomHexId(length)`              → [`random_hex_id`]
//!   - `randomFlyerId()`                  → [`random_flyer_id`]
//!   - `token_key(username, password)`    → [`token_key`]
//!   - `is_correct_email_address(s)`      → [`is_correct_email`]
//!   - `generateHeaders(auth, locale)`    → [`generate_headers`]
//!   - `split_username_region_locale(s)`  → [`split_username_region_locale`]
//!   - `_arg(args, name)`                 → [`amp_arg`]

use std::collections::HashMap;

use once_cell::sync::Lazy;
use rand::distributions::{Distribution, Uniform};
use rand::seq::IteratorRandom;
use regex::Regex;
use sha2::{Digest, Sha256};

static EMAIL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("static regex compiles"));

pub static PODCAST_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9a-fA-F\-]+$").expect("static regex compiles"));

const HEX_CHARS: &[u8] = b"1234567890abcdef";

pub fn random_hex_id(length: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| {
            char::from(
                *HEX_CHARS
                    .iter()
                    .choose(&mut rng)
                    .expect("hex chars not empty"),
            )
        })
        .collect()
}

pub fn random_flyer_id() -> String {
    let mut rng = rand::thread_rng();
    let dist = Uniform::from(1_000_000_000_000_u64..=9_999_999_999_999_u64);
    let a = dist.sample(&mut rng);
    let b = dist.sample(&mut rng);
    format!("{a}-{b}")
}

pub fn token_key(username: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update(b"~");
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn is_correct_email(username: &str) -> bool {
    EMAIL_RE.is_match(username)
}

pub fn generate_headers(authorization: Option<&str>, locale: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("user-os".into(), "android".into());
    h.insert(
        "user-agent".into(),
        "Podimo/2.45.1 build 566/Android 33".into(),
    );
    h.insert("user-version".into(), "2.45.1".into());
    h.insert("user-locale".into(), locale.into());
    h.insert("user-unique-id".into(), random_hex_id(16));
    if let Some(auth) = authorization {
        h.insert("authorization".into(), auth.into());
    }
    h
}

/// Splits an HTTP-Basic username overloaded with `,region,locale`.
/// When fewer or more than three parts are present, defaults to `("nl", "nl-NL")`.
pub fn split_username_region_locale(s: &str) -> (String, String, String) {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 3 {
        (parts[0].into(), parts[1].into(), parts[2].into())
    } else {
        (parts[0].into(), "nl".into(), "nl-NL".into())
    }
}

/// Returns the query-arg value for `name`, falling back to `amp;<name>` for
/// consumers that don't decode `&amp;` (e.g. Audiobookshelf scraping the HTML).
pub fn amp_arg<'a, F>(get: F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<&'a str>,
{
    get(name)
        .or_else(|| get(&format!("amp;{name}")))
        .map(String::from)
}

/// Appends `#.jpg` to URLs that don't already end in `.jpg`/`.png`. Podimo image URLs
/// end in signed query strings; feedgen / Apple require a recognized extension.
/// Clients strip the fragment before issuing the GET, so the actual fetch is unaffected.
pub fn jpg_fragment(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".png") {
        url.to_string()
    } else {
        format!("{url}#.jpg")
    }
}

/// Returns `Some(true)` for any common positive coercion ("true", "1", "t", "y", "yes")
/// case-insensitive — matches the Python service's boolean handling.
pub fn parse_bool_loose(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "1" | "t" | "y" | "yes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation_matches_python() {
        assert!(is_correct_email("a@b.com"));
        assert!(is_correct_email("user+tag@example.co.uk"));
        assert!(!is_correct_email("not-an-email"));
        assert!(!is_correct_email("user @example.com"));
        assert!(!is_correct_email("user@@example.com"));
        assert!(!is_correct_email("user@example"));
    }

    #[test]
    fn token_key_is_stable() {
        // Same shape as the Python sha256(b"~".join([username, password])).
        let k = token_key("a@b.com", "secret");
        assert_eq!(k.len(), 64);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(token_key("a@b.com", "secret"), k);
    }

    #[test]
    fn split_three_parts() {
        assert_eq!(
            split_username_region_locale("a@b.com,nl,nl-NL"),
            ("a@b.com".into(), "nl".into(), "nl-NL".into())
        );
    }

    #[test]
    fn split_one_part_defaults() {
        assert_eq!(
            split_username_region_locale("a@b.com"),
            ("a@b.com".into(), "nl".into(), "nl-NL".into())
        );
    }

    #[test]
    fn split_four_parts_falls_back_to_defaults() {
        assert_eq!(
            split_username_region_locale("a@b.com,nl,nl-NL,extra"),
            ("a@b.com".into(), "nl".into(), "nl-NL".into())
        );
    }

    #[test]
    fn amp_arg_prefers_plain_then_amp_prefix() {
        let m: HashMap<&str, &str> = [("region", "nl"), ("amp;locale", "nl-NL")]
            .into_iter()
            .collect();
        assert_eq!(amp_arg(|k| m.get(k).copied(), "region"), Some("nl".into()));
        assert_eq!(
            amp_arg(|k| m.get(k).copied(), "locale"),
            Some("nl-NL".into())
        );
        assert_eq!(amp_arg(|k| m.get(k).copied(), "absent"), None);
    }

    #[test]
    fn amp_arg_plain_wins_over_amp_prefixed() {
        let m: HashMap<&str, &str> = [("region", "nl"), ("amp;region", "de")]
            .into_iter()
            .collect();
        assert_eq!(amp_arg(|k| m.get(k).copied(), "region"), Some("nl".into()));
    }

    #[test]
    fn jpg_fragment_appends_when_missing() {
        assert_eq!(
            jpg_fragment("https://x/y?signed=abc"),
            "https://x/y?signed=abc#.jpg"
        );
        assert_eq!(jpg_fragment("https://x/cover.jpg"), "https://x/cover.jpg");
        assert_eq!(jpg_fragment("https://x/cover.PNG"), "https://x/cover.PNG");
    }

    #[test]
    fn podcast_id_regex_matches_hex_and_hyphens() {
        assert!(PODCAST_ID_RE.is_match("de9b2081-9fc5-489f-b9d3-d744ed9cab20"));
        assert!(PODCAST_ID_RE.is_match("1234567890"));
        assert!(!PODCAST_ID_RE.is_match("not-a-valid-id!"));
        assert!(!PODCAST_ID_RE.is_match(""));
    }

    #[test]
    fn random_hex_id_length() {
        let s = random_hex_id(16);
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_flyer_id_format() {
        let s = random_flyer_id();
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 2);
        for p in parts {
            assert_eq!(p.len(), 13);
            assert!(p.chars().all(|c| c.is_ascii_digit()));
        }
    }
}
