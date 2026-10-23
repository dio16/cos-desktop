//! Bounded fallback redaction for unavoidable free-text diagnostics.
//!
//! Product diagnostic routes are typed by construction and never carry remote
//! payloads. This module covers the remaining free-text route (the debug-only
//! fixture trace) so user input, server strings or accidental interpolation
//! cannot forge lines, exceed a bound or expose a credential, signed locator or
//! private path. It is not an authorization or a persistence policy.
//!
//! Parsing is incremental: no full-input copy and no token vector. Sensitive
//! context survives whitespace/control separators, so `token= secret` and
//! `?sig=\nsecret` cannot leak the value in the following token. The output
//! stops at the byte limit; an over-long token is redacted rather than exposed.

/// Maximum bytes retained from one diagnostic line.
pub const MAX_DIAGNOSTIC_LINE_BYTES: usize = 512;

/// Replacement for an identified sensitive value.
pub const REDACTED: &str = "[redacted]";

const PRIVATE_PATH: &str = "[private-path]";
const SIGNED_LOCATOR: &str = "[signed-locator]";
const TOKEN_LIMIT: usize = 256;

const SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "api-key",
    "apikey",
    "authorization",
    "credential",
    "expires",
    "password",
    "refresh_token",
    "secret",
    "sig",
    "signature",
    "token",
    "x-amz-credential",
    "x-amz-security-token",
    "x-amz-signature",
];

#[derive(Clone, Copy, Debug, Default)]
struct State {
    redact_next: bool,
    rest_of_line: bool,
    path_continuations: u8,
    path_quote: Option<char>,
}

/// Render one diagnostic line with sensitive free text removed.
///
/// Control characters are separators, so a value cannot start a forged log
/// line. A URL carrying a query or fragment is removed as a whole because it may
/// be a signed locator; drive, UNC, POSIX and remaining backslash path tokens
/// are removed, including their space-separated continuation tokens. Credential
/// assignments and Bearer values are removed even when the value is separated by
/// whitespace or a control character. Output is capped at
/// [`MAX_DIAGNOSTIC_LINE_BYTES`].
pub fn redact_diagnostic_text(input: &str) -> String {
    redact_diagnostic_text_within(input, MAX_DIAGNOSTIC_LINE_BYTES)
}

/// Render with an explicit byte cap, used to include a caller-owned prefix and
/// the terminating newline inside one complete bounded line.
pub fn redact_diagnostic_text_within(input: &str, limit: usize) -> String {
    let mut output = String::with_capacity(input.len().min(limit));
    let mut token = String::with_capacity(TOKEN_LIMIT);
    let mut token_is_long = false;
    let mut long_token_closed_quote = false;
    let mut long_token_quote = None;
    let mut long_token_is_url = false;
    let mut long_token_has_query = false;
    let mut previous = None;
    let mut previous_previous = None;
    let mut state = State::default();

    for character in input.chars() {
        if character.is_whitespace() || character.is_control() {
            if !token.is_empty() || token_is_long {
                flush(
                    &mut output,
                    &token,
                    token_is_long,
                    long_token_closed_quote,
                    long_token_is_url && long_token_has_query,
                    &mut state,
                    limit,
                );
                token.clear();
                token_is_long = false;
                long_token_closed_quote = false;
                long_token_quote = None;
                long_token_is_url = false;
                long_token_has_query = false;
                previous = None;
                previous_previous = None;
                if output.len() >= limit {
                    return output;
                }
            }
            continue;
        }
        if token_is_long {
            if character == '?' || character == '#' {
                long_token_has_query = true;
            }
            if previous_previous == Some(':') && previous == Some('/') && character == '/' {
                long_token_is_url = true;
            }
            previous_previous = previous;
            previous = Some(character);
            if state.path_quote == Some(character) || long_token_quote == Some(character) {
                long_token_closed_quote = true;
            }
            continue;
        }
        if token.len() + character.len_utf8() > TOKEN_LIMIT {
            token_is_long = true;
            long_token_is_url = token.contains("://");
            long_token_has_query = token.contains('?') || token.contains('#');
            let mut tail = token.chars().rev();
            previous = tail.next();
            previous_previous = tail.next();
            if character == '?' || character == '#' {
                long_token_has_query = true;
            }
            if previous_previous == Some(':') && previous == Some('/') && character == '/' {
                long_token_is_url = true;
            }
            previous_previous = previous;
            previous = Some(character);
            let prefix_state = classify_prefix(&token, false);
            long_token_quote = prefix_state.path_quote;
            if state.path_quote == Some(character) || long_token_quote == Some(character) {
                long_token_closed_quote = true;
            }
        } else {
            token.push(character);
        }
    }
    if !token.is_empty() || token_is_long {
        flush(
            &mut output,
            &token,
            token_is_long,
            long_token_closed_quote,
            long_token_is_url && long_token_has_query,
            &mut state,
            limit,
        );
    }
    output
}

fn flush(
    out: &mut String,
    token: &str,
    token_is_long: bool,
    long_token_closed_quote: bool,
    long_token_url_query: bool,
    state: &mut State,
    limit: usize,
) {
    if token_is_long {
        append(out, REDACTED, limit);
        if state.rest_of_line {
            // Once active, no token length, content or classification may
            // deactivate withholding.
            return;
        }
        if state.path_quote.is_some() {
            if long_token_closed_quote {
                state.path_continuations = 0;
                state.path_quote = None;
            }
            // An open quoted path keeps its continuation state and a real
            // closing quote is detected without retaining the long token.
            return;
        }
        if state.path_continuations > 0 {
            // An unquoted path keeps withholding subsequent fragments.
            return;
        }
        if state.redact_next {
            // A separated sensitive value has no reliable boundary; fail closed
            // for the remainder of the line.
            state.redact_next = false;
            state.rest_of_line = true;
            return;
        }
        // Reuse the same bounded classifier as a short token on the retained
        // prefix. A long key=value/private-path, sensitive assignment or signed
        // locator must not weaken the corresponding short-token guarantee.
        *state = classify_prefix(token, long_token_closed_quote);
        if state.redact_next {
            state.redact_next = false;
            state.rest_of_line = true;
        }
        if long_token_url_query {
            // A query/fragment marker may appear after the retained prefix. If
            // the URL ends with a sensitive assignment, the following token is
            // the detached value; fail closed because the exact boundary is not
            // retained.
            state.rest_of_line = true;
        }
        return;
    }
    if state.rest_of_line {
        if token.eq_ignore_ascii_case("bearer") {
            append(out, "Bearer", limit);
            return;
        }
        append(out, REDACTED, limit);
        return;
    }
    if state.redact_next {
        if token.eq_ignore_ascii_case("bearer") {
            // Keep the marker so the following token is still redacted.
            append(out, "Bearer", limit);
            return;
        }
        append(out, REDACTED, limit);
        state.redact_next = false;
        state.rest_of_line = true;
        return;
    }
    if state.path_continuations > 0 {
        if let Some(quote) = state.path_quote {
            // Inside an explicitly quoted path, everything up to the closing
            // quote is still part of that path; a safe-looking key=value cannot
            // escape it.
            append(out, PRIVATE_PATH, limit);
            if token.ends_with(quote) {
                state.path_continuations = 0;
                state.path_quote = None;
            }
            return;
        }
        if is_safe_assignment(token) {
            state.path_continuations = 0;
        } else {
            append(out, PRIVATE_PATH, limit);
            // A multi-word unquoted path keeps continuation state until a
            // structured non-path field or the end of the line.
            return;
        }
    }
    if looks_like_path(token) {
        append(out, PRIVATE_PATH, limit);
        state.path_continuations = 1;
        state.path_quote = None;
        return;
    }
    if token.eq_ignore_ascii_case("bearer") {
        append(out, "Bearer", limit);
        state.rest_of_line = true;
        return;
    }
    if is_url(token) {
        if is_signed_locator(token) {
            append(out, SIGNED_LOCATOR, limit);
            state.rest_of_line = token.ends_with('=');
        } else {
            append(out, token, limit);
        }
        return;
    }
    if token.find(['=', ':']).is_none() && is_sensitive_key(clean_key(token)) {
        // A separator may arrive in the next token after whitespace/control
        // splitting; fail closed for the following value.
        append(out, token, limit);
        state.rest_of_line = true;
        return;
    }
    if let Some(index) = token.find(['=', ':']) {
        let separator = token.as_bytes()[index] as char;
        let raw_key = &token[..index];
        let value = &token[index + 1..];
        let key = clean_key(raw_key);
        if key_is_plain(key) && is_sensitive_key(key) {
            if value.is_empty() {
                append(out, &format!("{raw_key}{separator}"), limit);
                state.rest_of_line = true;
                return;
            }
            if value.eq_ignore_ascii_case("bearer") {
                append(out, &format!("{raw_key}{separator}Bearer"), limit);
                state.rest_of_line = true;
                return;
            }
            append(out, &format!("{raw_key}{separator}{REDACTED}"), limit);
            return;
        }
        if key_is_plain(key) && is_signed_locator(value) {
            append(out, &format!("{raw_key}{separator}{SIGNED_LOCATOR}"), limit);
            state.rest_of_line = value.ends_with('=');
            return;
        }
        if key_is_plain(key) && is_private_path_value(value) {
            if separator == ':'
                && !is_sensitive_key(key)
                && !looks_like_path(value)
                && value.contains('/')
            {
                // `fixture:task/Task-184` is an opaque namespaced identity, not
                // a diagnostic key with a path value.
                append(out, token, limit);
                return;
            }
            append(out, &format!("{raw_key}{separator}{PRIVATE_PATH}"), limit);
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'');
            let closed = quote.is_some_and(|quote| value.len() > 1 && value.ends_with(quote));
            if !closed {
                state.path_continuations = 1;
                state.path_quote = quote;
            }
            return;
        }
    }
    if is_signed_locator(token) {
        append(out, SIGNED_LOCATOR, limit);
        state.redact_next = token.ends_with('=');
        return;
    }
    append(out, token, limit);
}

fn clean_key(raw: &str) -> &str {
    raw.trim_matches(|c| c == '"' || c == '\'')
        .trim_start_matches(['?', '&', '#'])
}

/// Classify the retained prefix of an overlong token with the same bounded
/// rules as a short token. The full long token is never retained.
fn classify_prefix(prefix: &str, closed_quote: bool) -> State {
    let mut state = State::default();
    if let Some(index) = prefix.find(['=', ':']) {
        let separator = prefix.as_bytes()[index] as char;
        let raw_key = &prefix[..index];
        let value = &prefix[index + 1..];
        let key = clean_key(raw_key);
        if key_is_plain(key) {
            if is_sensitive_key(key) {
                // The value may be a truncated secret; fail closed.
                state.redact_next = true;
                return state;
            }
            if is_signed_locator(value) {
                state.redact_next = value.ends_with('=');
                return state;
            }
            if is_private_path_value(value) {
                if separator == ':' && !looks_like_path(value) && value.contains('/') {
                    // Opaque namespaced identity, not a path value.
                    return state;
                }
                let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'');
                let closed = quote.is_some_and(|quote| value.len() > 1 && value.ends_with(quote));
                if !closed && !closed_quote {
                    state.path_continuations = 1;
                    state.path_quote = quote;
                }
                return state;
            }
        }
    }
    if is_signed_locator(prefix) {
        state.redact_next = prefix.ends_with('=');
        return state;
    }
    let trimmed = prefix.trim_start_matches(['"', '\'']);
    if looks_like_path(trimmed) {
        if !closed_quote {
            state.path_continuations = 1;
            state.path_quote = prefix.chars().next().filter(|c| *c == '"' || *c == '\'');
        }
        return state;
    }
    if is_sensitive_key(clean_key(prefix)) {
        state.redact_next = true;
    }
    state
}

fn key_is_plain(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.'\"".contains(&byte))
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.trim_matches(|c| c == '"' || c == '\'');
    SENSITIVE_KEYS
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

fn is_signed_locator(value: &str) -> bool {
    value.contains("://") && (value.contains('?') || value.contains('#'))
}

/// A token that is a URL, not a `key=value` field whose value is a URL.
fn is_url(token: &str) -> bool {
    let Some(index) = token.find("://") else {
        return false;
    };
    if index == 0 || !token.as_bytes()[0].is_ascii_alphabetic() {
        return false;
    }
    token[..index]
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte))
}

fn looks_like_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    token.starts_with("\\\\") || token.starts_with('/') || token.contains('\\')
}

/// A non-sensitive `key=value`/`key:value` diagnostic that may follow a path
/// token without being part of that path. Keeps safe counters visible.
fn is_safe_assignment(token: &str) -> bool {
    if token.contains('\\') || token.starts_with('/') {
        return false;
    }
    let Some(index) = token.find(['=', ':']) else {
        return false;
    };
    let key = clean_key(&token[..index]);
    let value = &token[index + 1..];
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    key_is_plain(key)
        && !is_sensitive_key(key)
        && !is_signed_locator(value)
        && !is_private_path_value(value)
}

fn is_private_path_value(value: &str) -> bool {
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    if looks_like_path(value) {
        return true;
    }
    if !value.contains('/') || value.contains("://") {
        return false;
    }
    // An opaque namespaced identity such as `fixture:task/1` is not a path.
    !matches!(
        (value.find(':'), value.find('/')),
        (Some(colon), Some(slash)) if colon < slash
    )
}

fn append(out: &mut String, text: &str, limit: usize) {
    if text.is_empty() {
        return;
    }
    let separator = usize::from(!out.is_empty());
    let used = out.len() + separator;
    if used >= limit {
        return;
    }
    if separator == 1 {
        out.push(' ');
    }
    let remaining = limit - out.len();
    if text.len() <= remaining {
        out.push_str(text);
    } else {
        for character in text.chars() {
            if out.len() + character.len_utf8() > limit {
                break;
            }
            out.push(character);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DIAGNOSTIC_LINE_BYTES, REDACTED, redact_diagnostic_text, redact_diagnostic_text_within,
    };

    #[test]
    fn credential_assignments_and_bearer_values_never_survive() {
        assert_eq!(
            redact_diagnostic_text("token=s3cr3t-value next"),
            "token=[redacted] next"
        );
        assert_eq!(
            redact_diagnostic_text("Authorization: Bearer abc.def.ghi"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(
            redact_diagnostic_text("api-key=abc123 password:\"hunter2\""),
            "api-key=[redacted] password:[redacted]"
        );
    }

    #[test]
    fn separated_values_do_not_escape_through_whitespace_or_controls() {
        for input in [
            "token= synthetic-secret-123",
            "token=\nsynthetic-secret-123",
            "token=\u{0}synthetic-secret-123",
            "sig: synthetic-secret-123",
            "?sig=\nsynthetic-secret-123",
            "https://host/file?sig=\nsynthetic-secret-123",
            "?sig\n=synthetic-secret-123",
            "token\n=synthetic-secret-123",
            "sig\n:synthetic-secret-123",
        ] {
            let redacted = redact_diagnostic_text(input);
            assert!(
                !redacted.contains("synthetic-secret-123"),
                "a separated sensitive value leaked: {redacted}"
            );
        }
        assert_eq!(
            redact_diagnostic_text("Authorization:Bearer bearer-token-value"),
            "Authorization:Bearer [redacted]"
        );
        assert!(
            !redact_diagnostic_text("Authorization:Bearer bearer-token-value")
                .contains("bearer-token-value")
        );
        for input in [
            "token= first second third",
            "Authorization: Bearer first second",
            "?sig\n=first second",
            "sig\n:first second",
        ] {
            let redacted = redact_diagnostic_text(input);
            for fragment in ["first", "second", "third"] {
                assert!(
                    !redacted.contains(fragment),
                    "a multi-token separated secret leaked {fragment}: {redacted}"
                );
            }
        }
        assert_eq!(
            redact_diagnostic_text("token=abc frame=3"),
            "token=[redacted] frame=3",
            "a complete same-token value does not hide following safe fields"
        );
        for size in [255usize, 256, 257, 300] {
            let long = "X".repeat(size);
            for input in [
                format!("token= first {long} SecretSuffix"),
                format!("sig: first {long} SecretSuffix"),
                format!("Authorization: Bearer first {long} SecretSuffix"),
                format!("?sig= first {long} SecretSuffix"),
                format!("media=https://host/file{long}?sig= first {long} SecretSuffix"),
                format!("token=\nfirst\n{long}\nSecretSuffix"),
            ] {
                let output = redact_diagnostic_text(&input);
                assert!(
                    !output.contains("SecretSuffix"),
                    "rest-of-line withholding was lost across a {size}-byte token: {output}"
                );
            }
        }
    }

    #[test]
    fn signed_locator_and_private_paths_are_removed_with_continuations() {
        assert_eq!(
            redact_diagnostic_text("media=https://host/o?X-Amz-Signature=abc"),
            "media=[signed-locator]"
        );
        assert_eq!(
            redact_diagnostic_text(r"C:\Users\operator\private\input.png"),
            "[private-path]"
        );
        assert_eq!(
            redact_diagnostic_text(r"\\server\share\secret.bin"),
            "[private-path]"
        );
        assert_eq!(
            redact_diagnostic_text(r"C:\Users\John Doe\secret.png"),
            "[private-path] [private-path]"
        );
        assert_eq!(
            redact_diagnostic_text("C:/Users/First Last/private.png"),
            "[private-path] [private-path]"
        );
        assert_eq!(
            redact_diagnostic_text("/home/operator/secret.png"),
            "[private-path]"
        );
    }

    #[test]
    fn quoted_paths_and_detached_signatures_are_removed() {
        assert_eq!(
            redact_diagnostic_text(r#"query="C:/Users/Alice/private.png""#),
            "query=[private-path]"
        );
        assert_eq!(
            redact_diagnostic_text(r#"query="/home/operator/secret.png""#),
            "query=[private-path]"
        );
        for input in [
            "media=https://host/file?sig= secret123",
            "media=https://host/file?sig=\nsecret123",
            "media=https://host/file?sig=\u{0}secret123",
        ] {
            let redacted = redact_diagnostic_text(input);
            assert!(
                !redacted.contains("secret123"),
                "a detached signature survived: {redacted}"
            );
        }
        let multi = redact_diagnostic_text("C:/Users/First Middle Last/private.png");
        assert!(!multi.contains("Last/private.png"), "{multi}");
        let named = redact_diagnostic_text("C:/Users/First Name=Secret/file.png");
        assert!(!named.contains("Secret/file.png"), "{named}");
        for input in [
            r#"query="C:/Users/First Middle Last/private.png""#,
            r#"query="/home/First Middle Last/private.png""#,
            r#"query="C:/Users/First Name=Secret/file.png""#,
        ] {
            let redacted = redact_diagnostic_text(input);
            for fragment in ["Middle", "Last/private.png", "Secret/file.png"] {
                assert!(
                    !redacted.contains(fragment),
                    "a quoted path continuation leaked {fragment}: {redacted}"
                );
            }
        }
        assert_eq!(
            redact_diagnostic_text(
                r#"query="C:/Users/First Middle Last/private.png" result_index=0"#
            ),
            "query=[private-path] [private-path] [private-path] result_index=0",
            "a closing quote ends the path and the next structured field is visible"
        );
        assert_eq!(
            redact_diagnostic_text("frame=3 count=10 offset_px=-232"),
            "frame=3 count=10 offset_px=-232",
            "ordinary numeric diagnostics stay visible"
        );
        assert_eq!(
            redact_diagnostic_text("anchor_task=fixture:task/Task-184 active=fixture:candidate/a"),
            "anchor_task=fixture:task/Task-184 active=fixture:candidate/a",
            "opaque namespaced identities are not private paths"
        );
    }

    #[test]
    fn overlong_tokens_never_clear_active_path_or_secret_state() {
        let long = "X".repeat(300);
        let cases = [
            format!(r#"query="C:/Users/First {long} SecretSuffix/file.png" result_index=0"#),
            format!(r#"query="/home/First {long} SecretSuffix/file.png" result_index=0"#),
            format!(r#"query="C:/Users/First {long}png" result_index=0"#),
            format!(r#"query="/home/First {long} SecretSuffix/file.png"#),
            format!("token= {long} SecretSuffix"),
        ];
        for input in cases {
            let output = redact_diagnostic_text(&input);
            assert!(
                !output.contains("SecretSuffix/file.png"),
                "an overlong token exposed a path fragment: {output}"
            );
        }
        let closed = redact_diagnostic_text(&format!(
            r#"query="C:/Users/First {long} SecretSuffix/file.png" result_index=0"#
        ));
        assert!(
            closed.contains("result_index=0"),
            "the closing quote must end path continuation: {closed}"
        );
        for input in [
            format!("C:/Users/First {long} SecretSuffix/file.png frame=3"),
            format!("/home/First {long} SecretSuffix/file.png frame=3"),
            format!("C:/Users/FirstXX{long} SecretSuffix/file.png frame=3"),
            format!("\"C:/Users/First{long} SecretSuffix/file.png"),
        ] {
            let output = redact_diagnostic_text(&input);
            assert!(
                !output.contains("SecretSuffix/file.png"),
                "an overlong unquoted path exposed a fragment: {output}"
            );
            if !input.starts_with('"') {
                assert!(
                    output.contains("frame=3"),
                    "a safe field after the path boundary must remain visible: {output}"
                );
            }
        }
        let quoted_closed =
            redact_diagnostic_text(&format!("\"C:/Users/First{long}\" result_index=0"));
        assert!(
            quoted_closed.contains("result_index=0"),
            "a standalone quoted long path that closes must end withholding: {quoted_closed}"
        );
        for size in [255usize, 256, 257, 300] {
            let long = "X".repeat(size);
            for (input, visible) in [
                (
                    format!(r#"query="C:/Users/First{long} SecretSuffix/file.png" frame=3"#),
                    "frame=3",
                ),
                (
                    format!("path=C:/Users/First{long} SecretSuffix/file.png count=10"),
                    "count=10",
                ),
                (
                    format!(r#"path="/home/First{long} SecretSuffix/file.png" result_index=0"#),
                    "result_index=0",
                ),
            ] {
                let output = redact_diagnostic_text(&input);
                assert!(
                    !output.contains("SecretSuffix/file.png"),
                    "a long key=value path exposed a fragment at {size}: {output}"
                );
                assert!(
                    output.contains(visible),
                    "a safe field after the path boundary must remain visible at {size}: {output}"
                );
            }
            let closed =
                redact_diagnostic_text(&format!(r#"query="C:/Users/First{long}" result_index=0"#));
            assert!(
                closed.contains("result_index=0"),
                "a closing quote inside an overlong key=value path must end withholding at {size}: {closed}"
            );
            let unclosed = format!(r#"query="C:/Users/First{long} SecretSuffix/file.png"#);
            assert!(
                !redact_diagnostic_text(&unclosed).contains("SecretSuffix/file.png"),
                "an unclosed overlong key=value path exposed a fragment at {size}"
            );
        }
        assert_eq!(
            redact_diagnostic_text("frame=3 count=10 offset_px=-232 fixture:task/Task-184"),
            "frame=3 count=10 offset_px=-232 fixture:task/Task-184"
        );
    }

    #[test]
    fn late_signed_url_metadata_never_releases_a_detached_value() {
        let long = "X".repeat(300);
        for suffix in ["?sig=", "#signature="] {
            for input in [
                format!("media=https://host.example/file/{long}{suffix} SyntheticSecretValue"),
                format!("https://host.example/{long}{suffix} SyntheticSecretValue"),
                format!("media=https://host.example/{long}{suffix}\nSyntheticSecretValue"),
            ] {
                let output = redact_diagnostic_text(&input);
                assert!(
                    !output.contains("SyntheticSecretValue"),
                    "late {suffix} metadata released a detached value: {output}"
                );
            }
        }
        let boundary = "X".repeat(250);
        for input in [
            format!("media=https://host/{boundary}?sig= SyntheticSecretValue"),
            format!("media=https://host/{boundary}#signature= SyntheticSecretValue"),
        ] {
            let output = redact_diagnostic_text(&input);
            assert!(
                !output.contains("SyntheticSecretValue"),
                "a boundary signature released a detached value: {output}"
            );
        }
        let at_limit = format!(
            "media=https://host/{}?sig= SyntheticSecretValue",
            "X".repeat(256 - "media=https://host/".len())
        );
        assert!(
            !redact_diagnostic_text(&at_limit).contains("SyntheticSecretValue"),
            "a query marker at the token boundary released a detached value"
        );
    }

    #[test]
    fn control_characters_cannot_forge_a_second_line_and_output_is_bounded() {
        assert_eq!(
            redact_diagnostic_text("first\nsecond\r\nthird\u{0}tail"),
            "first second third tail"
        );
        let long = "x".repeat(MAX_DIAGNOSTIC_LINE_BYTES * 2);
        let redacted = redact_diagnostic_text(&long);
        assert_eq!(redacted, REDACTED);
        assert_eq!(
            redact_diagnostic_text_within("abcdef", 3),
            "abc",
            "an explicit cap truncates on a UTF-8 boundary"
        );
        assert_eq!(
            redact_diagnostic_text_within("ab😀cd", 6),
            "ab😀",
            "the cap includes the complete multi-byte character"
        );
        assert_eq!(
            redact_diagnostic_text("C:\\private\\x frame=3 count=10"),
            "[private-path] frame=3 count=10"
        );
    }
}
