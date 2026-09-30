//! One combat log line: a timestamp, two spaces, then a comma-separated event.
//!
//! ```text
//! 9/27/2026 21:35:45.1241  ENCOUNTER_START,3199,"Lightblossom Trinity",8,5,2859
//! ```
//!
//! The app parses only what it needs to find boundaries and show a pull's
//! result; the server does the real parse.

/// A line split into its timestamp, event name and the fields after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    pub timestamp: &'a str,
    pub event: &'a str,
    /// Everything after the event name's comma (empty for an event with no fields).
    pub rest: &'a str,
}

/// Splits a raw line (with or without its line ending). `None` for anything
/// that isn't a combat log line: a fragment, a blank line, bytes that aren't
/// UTF-8.
pub fn parse(raw: &[u8]) -> Option<Line<'_>> {
    let raw = trim_newline(raw);
    // The event name is ASCII; only look far enough to find it, so a line
    // with a stray invalid byte in a name further on still parses.
    let sep = find_double_space(raw)?;
    let timestamp = std::str::from_utf8(&raw[..sep]).ok()?;
    let body = &raw[sep + 2..];
    let comma = body.iter().position(|&b| b == b',').unwrap_or(body.len());
    let event = std::str::from_utf8(&body[..comma]).ok()?;
    if event.is_empty()
        || !event
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit())
    {
        return None;
    }
    let rest = if comma < body.len() {
        std::str::from_utf8(&body[comma + 1..]).unwrap_or("")
    } else {
        ""
    };
    Some(Line {
        timestamp,
        event,
        rest,
    })
}

/// The event name alone, cheaply: most lines are damage and heals, and the
/// splitter only needs the full fields of a handful of events.
pub fn event_name(raw: &[u8]) -> Option<&str> {
    let sep = find_double_space(raw)?;
    let body = &raw[sep + 2..];
    let end = body
        .iter()
        .position(|&b| b == b',' || b == b'\r' || b == b'\n')
        .unwrap_or(body.len());
    std::str::from_utf8(&body[..end]).ok()
}

pub fn trim_newline(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    &raw[..end]
}

fn find_double_space(raw: &[u8]) -> Option<usize> {
    // Timestamps are short; don't scan a long garbage line for a separator.
    let limit = raw.len().min(64);
    raw[..limit].windows(2).position(|w| w == b"  ")
}

/// Splits fields on commas, keeping quoted names ("Name, the Bold") and
/// bracketed lists (`[9,10,147]`, `(1,2)`) whole. Quotes are removed from a
/// quoted field.
pub fn fields(rest: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut in_quotes = false;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'[' | b'(' if !in_quotes => depth += 1,
            b']' | b')' if !in_quotes => depth -= 1,
            b',' if !in_quotes && depth <= 0 => {
                out.push(unquote(&rest[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    if start <= rest.len() && !rest.is_empty() {
        out.push(unquote(&rest[start..]));
    }
    out
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_line_with_crlf() {
        let l = parse(
            b"9/27/2026 21:45:04.5521  ENCOUNTER_START,3199,\"Lightblossom Trinity\",8,5,2859\r\n",
        )
        .unwrap();
        assert_eq!(l.timestamp, "9/27/2026 21:45:04.5521");
        assert_eq!(l.event, "ENCOUNTER_START");
        assert_eq!(
            fields(l.rest),
            vec!["3199", "Lightblossom Trinity", "8", "5", "2859"]
        );
    }

    #[test]
    fn keeps_quoted_commas_and_brackets() {
        let f = fields("\"The Blinding Vale\",2859,584,14,[9,10,147]");
        assert_eq!(
            f,
            vec!["The Blinding Vale", "2859", "584", "14", "[9,10,147]"]
        );
        let f = fields("3001,\"Boss, the Unkind\",16,20,2900");
        assert_eq!(f[1], "Boss, the Unkind");
    }

    #[test]
    fn rejects_fragments() {
        assert!(parse(b"").is_none());
        assert!(parse(b"0x512,0x0,Creature-0-1").is_none());
        assert!(parse(b"garbage  lowercase,1").is_none());
        assert_eq!(
            event_name(b"1/1/2026 00:00:00.0001  SPELL_DAMAGE,a,b"),
            Some("SPELL_DAMAGE")
        );
    }
}
