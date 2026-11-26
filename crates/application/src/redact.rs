//! Scrubs secrets from text before it is logged or shown.
//!
//! Secrets should never reach a log in the first place (they travel as `SecretString`); this is
//! the safety net for the cases where an error message from a driver or server echoes one.
//! Recognised: API keys (`sk-...`, `sk_...`), `Bearer` tokens, `password=`/`pwd=`/`secret=`/
//! `token=`/`api_key=` values (also with `:`), and `user:password@host` in URLs.

const MASK: &str = "[redacted]";

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | '=' | '~')
}

fn is_boundary(prev: Option<char>) -> bool {
    prev.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
}

/// Keys whose value is secret, matched case-insensitively.
const SECRET_KEYS: &[&str] = &[
    "password", "passwd", "pwd", "secret", "token", "api_key", "apikey", "api-key",
];

pub fn redact(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let prev = i.checked_sub(1).map(|p| chars[p]);

        // `scheme://user:password@host`
        if chars[i] == ':' && chars.get(i + 1) == Some(&'/') && chars.get(i + 2) == Some(&'/') {
            out.push_str("://");
            i += 3;
            let rest: String = chars[i..]
                .iter()
                .take_while(|c| !c.is_whitespace() && **c != '/')
                .collect();
            if let Some(at) = rest.rfind('@')
                && let Some(colon) = rest[..at].find(':')
            {
                out.push_str(&rest[..colon]);
                out.push(':');
                out.push_str(MASK);
                out.push('@');
                i += rest[..at].chars().count() + 1;
            }
            continue;
        }

        // `sk-...` / `sk_...` API keys
        if is_boundary(prev)
            && (chars[i] == 's' || chars[i] == 'S')
            && matches!(chars.get(i + 1), Some('k' | 'K'))
            && matches!(chars.get(i + 2), Some('-' | '_'))
        {
            let len = chars[i + 3..]
                .iter()
                .take_while(|c| is_token_char(**c))
                .count();
            if len >= 8 {
                out.push_str(MASK);
                i += 3 + len;
                continue;
            }
        }

        // `Bearer <token>`
        if is_boundary(prev)
            && chars[i..]
                .iter()
                .take(7)
                .collect::<String>()
                .eq_ignore_ascii_case("bearer ")
        {
            let start = i + 7;
            let len = chars[start..]
                .iter()
                .take_while(|c| is_token_char(**c))
                .count();
            if len >= 8 {
                out.extend(chars[i..start].iter());
                out.push_str(MASK);
                i = start + len;
                continue;
            }
        }

        // `key=value`, `key: value`, `key = "quoted value"`, `"key" : "value"`
        if is_boundary(prev) && chars[i].is_ascii_alphabetic() {
            let word: String = chars[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .collect();
            if SECRET_KEYS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
                let mut j = i + word.chars().count();
                // The closing quote of a JSON key, then spaces, then the separator.
                if matches!(chars.get(j), Some('"' | '\'')) {
                    j += 1;
                }
                while chars.get(j) == Some(&' ') {
                    j += 1;
                }
                if matches!(chars.get(j), Some('=' | ':')) {
                    j += 1;
                    while chars.get(j) == Some(&' ') {
                        j += 1;
                    }
                    // A quoted value is masked whole, spaces included.
                    let (start, len) = match chars.get(j) {
                        Some(&q @ ('"' | '\'')) => {
                            let start = j + 1;
                            (
                                start,
                                chars[start..].iter().take_while(|c| **c != q).count(),
                            )
                        }
                        _ => (
                            j,
                            chars[j..]
                                .iter()
                                .take_while(|c| {
                                    !c.is_whitespace()
                                        && !matches!(c, '"' | '\'' | '&' | ',' | ';' | ')' | '}')
                                })
                                .count(),
                        ),
                    };
                    if len > 0 {
                        out.extend(chars[i..start].iter());
                        out.push_str(MASK);
                        i = start + len;
                        continue;
                    }
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        redact(s)
    }

    #[test]
    fn api_keys_are_masked() {
        assert_eq!(
            r("invalid key sk-abc123XYZ789 rejected"),
            "invalid key [redacted] rejected"
        );
        assert_eq!(r("key=sk_live_abcdef1234567890"), "key=[redacted]");
        assert_eq!(
            r("anthropic sk-ant-api03-AbCdEf_123-xyz failed"),
            "anthropic [redacted] failed"
        );
    }

    #[test]
    fn short_or_unrelated_sk_words_are_left_alone() {
        assert_eq!(
            r("task-list and sk-1 and desk-lamp-001"),
            "task-list and sk-1 and desk-lamp-001"
        );
        assert_eq!(r("risk-assessment-document"), "risk-assessment-document");
    }

    #[test]
    fn bearer_tokens_are_masked() {
        assert_eq!(
            r("Authorization: Bearer abcDEF123456.xyz"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(r("bearer abcdefgh12345678"), "bearer [redacted]");
        assert_eq!(r("a bearer of news"), "a bearer of news");
    }

    #[test]
    fn password_like_key_values_are_masked() {
        assert_eq!(
            r("host=db user=a password=hunter2 port=5432"),
            "host=db user=a password=[redacted] port=5432"
        );
        assert_eq!(r("PASSWORD: hunter2"), "PASSWORD: [redacted]");
        assert_eq!(
            r("{\"password\":\"hunter2\",\"user\":\"a\"}"),
            "{\"password\":\"[redacted]\",\"user\":\"a\"}"
        );
        assert_eq!(
            r("api_key=abc123 token=zzz"),
            "api_key=[redacted] token=[redacted]"
        );
        assert_eq!(r("?pwd=secret&x=1"), "?pwd=[redacted]&x=1");
    }

    #[test]
    fn spaces_around_the_separator_and_quoted_values_with_spaces_are_masked_whole() {
        assert_eq!(r("password = hunter2"), "password = [redacted]");
        assert_eq!(
            r("\"password\" : \"hunter2\""),
            "\"password\" : \"[redacted]\""
        );
        assert_eq!(
            r("host=h password='my pass phrase' user=u"),
            "host=h password='[redacted]' user=u"
        );
        assert_eq!(r("pwd: \"two words\", next"), "pwd: \"[redacted]\", next");
        let leaked = r("password='my pass'");
        assert!(
            !leaked.contains("pass'") && !leaked.contains("my"),
            "{leaked}"
        );
    }

    #[test]
    fn keys_inside_other_words_and_empty_values_are_left_alone() {
        assert_eq!(r("passwords are hashed"), "passwords are hashed");
        assert_eq!(r("password="), "password=");
        assert_eq!(r("mypassword=x"), "mypassword=x");
    }

    #[test]
    fn credentials_in_urls_are_masked() {
        assert_eq!(
            r("postgres://alice:hunter2@db.local:5432/app"),
            "postgres://alice:[redacted]@db.local:5432/app"
        );
        assert_eq!(r("http://example.com/path"), "http://example.com/path");
        assert_eq!(r("https://user@example.com/"), "https://user@example.com/");
    }

    #[test]
    fn several_secrets_and_unicode_text_survive() {
        let input = "ошибка: password=пароль1 and sk-abcdefgh12345 ✓";
        assert_eq!(r(input), "ошибка: password=[redacted] and [redacted] ✓");
    }

    #[test]
    fn ordinary_text_is_unchanged() {
        for s in [
            "",
            "SELECT * FROM users WHERE id = 1",
            "connection refused (os error 111)",
            "HTTP 429: slow down",
        ] {
            assert_eq!(r(s), s);
        }
    }
}
