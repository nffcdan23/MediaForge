//! Gatekeeper for everything the user can type or paste into the link field.
//!
//! Only `http` and `https` links with a real host get through, which rules out `file:`, `javascript:`,
//! option-looking strings such as `--exec`, and anything else that could be reinterpreted downstream.

use serde::Serialize;
use url::Url;

pub const MAX_LENGTH: usize = 2048;
pub const MAX_BATCH_SIZE: usize = 200;

/// A link that has passed validation and is safe to hand to yt-dlp as a positional argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedUrl {
    url: Url,
}

impl ValidatedUrl {
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or("")
    }

    /// A compact label for use before the real title is known, e.g. `example.com/media/clip`.
    pub fn display_name(&self) -> String {
        let host = self.host();
        let host = host.strip_prefix("www.").unwrap_or(host);
        let path = percent_decode(self.url.path());
        if path.chars().count() > 1 { format!("{host}{path}") } else { host.to_string() }
    }
}

impl Serialize for ValidatedUrl {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlValidationError {
    Empty,
    TooLong,
    ContainsWhitespaceOrControlCharacters,
    Malformed,
    UnsupportedScheme(String),
    MissingHost,
    ContainsCredentials,
    InvalidPort,
}

impl UrlValidationError {
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "Paste a link to get started.".into(),
            Self::TooLong => "That link is too long to be valid.".into(),
            Self::ContainsWhitespaceOrControlCharacters => "Links can't contain spaces or control characters.".into(),
            Self::Malformed => "That doesn't look like a valid link.".into(),
            Self::UnsupportedScheme(scheme) => {
                format!("“{scheme}” links aren't supported. Use an http or https address.")
            }
            Self::MissingHost => "The link is missing a website address.".into(),
            Self::ContainsCredentials => "Links with an embedded username or password aren't accepted.".into(),
            Self::InvalidPort => "The link has an invalid port number.".into(),
        }
    }
}

pub fn validate(input: &str) -> Result<ValidatedUrl, UrlValidationError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(UrlValidationError::Empty);
    }
    if trimmed.len() > MAX_LENGTH {
        return Err(UrlValidationError::TooLong);
    }
    if trimmed.chars().any(is_forbidden_char) {
        return Err(UrlValidationError::ContainsWhitespaceOrControlCharacters);
    }

    let mut candidate = trimmed.to_string();
    match trimmed.find(':') {
        Some(colon) if is_scheme_like(&trimmed[..colon]) && !looks_like_host_with_port(trimmed) => {
            let scheme = trimmed[..colon].to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(UrlValidationError::UnsupportedScheme(scheme));
            }
        }
        _ => {
            // A bare "example.com/clip" is unambiguous enough to upgrade; anything else is rejected.
            if !looks_like_bare_host(trimmed) {
                return Err(UrlValidationError::Malformed);
            }
            candidate = format!("https://{trimmed}");
        }
    }

    // "https://" with nothing after it is a missing host, not a parse failure.
    let after_scheme = candidate.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(UrlValidationError::MissingHost);
    }
    if authority.contains('@') {
        return Err(UrlValidationError::ContainsCredentials);
    }

    let url = Url::parse(&candidate).map_err(|error| match error {
        url::ParseError::InvalidPort => UrlValidationError::InvalidPort,
        url::ParseError::EmptyHost => UrlValidationError::MissingHost,
        url::ParseError::InvalidDomainCharacter | url::ParseError::IdnaError => UrlValidationError::MissingHost,
        _ => UrlValidationError::Malformed,
    })?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(UrlValidationError::UnsupportedScheme(scheme.to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlValidationError::ContainsCredentials);
    }
    match url.host_str() {
        Some(host) if is_plausible_host(host) => {}
        _ => return Err(UrlValidationError::MissingHost),
    }
    if url.port() == Some(0) {
        return Err(UrlValidationError::InvalidPort);
    }
    if url.as_str().len() > MAX_LENGTH {
        return Err(UrlValidationError::Malformed);
    }
    Ok(ValidatedUrl { url })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Extraction {
    pub urls: Vec<ValidatedUrl>,
    /// Tokens that were present but rejected.
    pub rejected: Vec<String>,
    /// True when more than `MAX_BATCH_SIZE` links were supplied and the tail was dropped.
    pub truncated: bool,
}

/// Splits pasted text on whitespace and validates each token, dropping duplicates.
pub fn extract(text: &str) -> Extraction {
    let mut result = Extraction::default();
    let mut seen = std::collections::HashSet::new();
    for token in text.split(char::is_whitespace) {
        let piece = token.trim_matches(|c| ",;<>\"'".contains(c));
        if piece.is_empty() {
            continue;
        }
        match validate(piece) {
            Ok(url) => {
                if !seen.insert(url.as_str().to_string()) {
                    continue;
                }
                if result.urls.len() >= MAX_BATCH_SIZE {
                    result.truncated = true;
                } else {
                    result.urls.push(url);
                }
            }
            Err(_) => result.rejected.push(piece.chars().take(80).collect()),
        }
    }
    result
}

fn is_forbidden_char(c: char) -> bool {
    let value = c as u32;
    value < 0x20
        || value == 0x7F
        || c.is_whitespace()
        || (0x80..=0x9F).contains(&value)
        // Bidirectional overrides and zero-width characters can disguise where a link really goes.
        || (0x200B..=0x200F).contains(&value)
        || (0x202A..=0x202E).contains(&value)
        || (0x2066..=0x2069).contains(&value)
        || value == 0xFEFF
}

fn is_scheme_like(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// Distinguishes `example.com:8080/x` (host and port) from `mailto:someone` (a scheme).
fn looks_like_host_with_port(text: &str) -> bool {
    let Some(colon) = text.find(':') else { return false };
    let host = &text[..colon];
    let port: String = text[colon + 1..].chars().take_while(|c| !"/?#".contains(*c)).collect();
    host.contains('.') && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
}

fn looks_like_bare_host(text: &str) -> bool {
    match text.chars().next() {
        Some(first) if first.is_alphanumeric() => {}
        _ => return false,
    }
    let authority: String = text.chars().take_while(|c| !"/?#".contains(*c)).collect();
    let host = authority.split(':').next().unwrap_or("");
    !authority.contains('@') && is_plausible_host(host) && host.contains('.')
}

fn is_plausible_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.contains(':') || host.starts_with('[') {
        // IPv6 literal, with or without brackets.
        let inner = host.trim_matches(|c| c == '[' || c == ']');
        return inner.chars().count() >= 2 && inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }
    if host.starts_with('.') || host.ends_with('.') || host.starts_with('-') || host.contains("..") {
        return false;
    }
    host.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '_' || !c.is_ascii())
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_web_links() {
        for input in [
            "https://example.com/watch?v=abc123",
            "http://example.com/media/clip.mp4",
            "https://sub.domain.example.co.uk/a/b/c?x=1&y=2#t=30",
            "https://example.com:8443/path",
            "http://127.0.0.1:8080/sample.mp4",
            "http://[::1]:8080/sample.mp4",
            "https://例え.jp/動画",
        ] {
            let url = validate(input).unwrap_or_else(|e| panic!("{input} rejected: {e:?}"));
            assert!(url.as_str().starts_with("http"), "{input}");
            assert!(!url.host().is_empty(), "{input}");
        }
    }

    #[test]
    fn normalises_scheme_and_host_case_and_trims() {
        let url = validate("  HTTPS://Example.COM/Path?Q=1 \n").unwrap();
        assert_eq!(url.as_str(), "https://example.com/Path?Q=1");
    }

    #[test]
    fn upgrades_a_bare_host_to_https() {
        assert_eq!(validate("example.com/clip").unwrap().as_str(), "https://example.com/clip");
        assert_eq!(validate("www.example.com").unwrap().as_str(), "https://www.example.com/");
        assert_eq!(validate("example.com:8080/x").unwrap().as_str(), "https://example.com:8080/x");
    }

    #[test]
    fn rejects_non_web_schemes() {
        for input in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/file.mp3",
            "data:text/html,<script>alert(1)</script>",
            "mailto:someone@example.com",
            "smb://server/share",
            "ytsearch:something",
        ] {
            match validate(input) {
                Err(UrlValidationError::UnsupportedScheme(_))
                | Err(UrlValidationError::Malformed)
                | Err(UrlValidationError::ContainsWhitespaceOrControlCharacters) => {}
                other => panic!("{input} should have been rejected, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_anything_that_could_be_read_as_an_option() {
        for input in ["--exec=touch /tmp/pwned", "-o/tmp/x", "--config-location=/tmp/evil.conf", "-", "--"] {
            assert!(validate(input).is_err(), "{input}");
        }
    }

    #[test]
    fn rejects_malformed_and_dangerous_input() {
        assert_eq!(validate(""), Err(UrlValidationError::Empty));
        assert_eq!(validate("   \n "), Err(UrlValidationError::Empty));
        assert_eq!(validate("https://"), Err(UrlValidationError::MissingHost));
        assert_eq!(validate("not a link"), Err(UrlValidationError::ContainsWhitespaceOrControlCharacters));
        assert_eq!(validate("justtext"), Err(UrlValidationError::Malformed));
        assert_eq!(validate("https://user:secret@example.com/"), Err(UrlValidationError::ContainsCredentials));
        assert_eq!(validate("https://example.com/a\u{0}b"), Err(UrlValidationError::ContainsWhitespaceOrControlCharacters));
        assert_eq!(validate("https://example.com/\u{202E}gpj.exe"), Err(UrlValidationError::ContainsWhitespaceOrControlCharacters));
        assert_eq!(validate(&format!("https://example.com/{}", "a".repeat(3000))), Err(UrlValidationError::TooLong));
        assert!(validate("https://-bad-.example..com/").is_err());
        assert_eq!(validate("https://example.com:0/"), Err(UrlValidationError::InvalidPort));
        assert_eq!(validate("https://example.com:99999/"), Err(UrlValidationError::InvalidPort));
    }

    #[test]
    fn extracts_several_links_dropping_duplicates_and_junk() {
        let text = "https://example.com/one\nhttps://example.com/two   https://example.com/one\nnonsense\n<https://example.com/three>,\nfile:///etc/hosts";
        let extraction = extract(text);
        let urls: Vec<_> = extraction.urls.iter().map(|u| u.as_str().to_string()).collect();
        assert_eq!(urls, ["https://example.com/one", "https://example.com/two", "https://example.com/three"]);
        assert_eq!(extraction.rejected, ["nonsense", "file:///etc/hosts"]);
        assert!(!extraction.truncated);
    }

    #[test]
    fn caps_batch_size() {
        let text: Vec<String> = (0..MAX_BATCH_SIZE + 25).map(|i| format!("https://example.com/{i}")).collect();
        let extraction = extract(&text.join("\n"));
        assert_eq!(extraction.urls.len(), MAX_BATCH_SIZE);
        assert!(extraction.truncated);
    }

    #[test]
    fn display_name_is_compact() {
        assert_eq!(validate("https://www.example.com/media/clip").unwrap().display_name(), "example.com/media/clip");
        assert_eq!(validate("https://example.com").unwrap().display_name(), "example.com");
        assert_eq!(validate("https://example.com/caf%C3%A9").unwrap().display_name(), "example.com/café");
    }
}
