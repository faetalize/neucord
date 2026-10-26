//! Parse Discord's message markup into a span tree the UI renders.
//!
//! Supports: bold, italic, underline, strikethrough, spoiler, inline code,
//! fenced/indented code blocks, block quotes, links, and the `<...>` entities
//! (user / channel / role / custom emoji / timestamp). Headings and lists are
//! left as plain text for now.

/// A rendered span. Styles nest, so `Styled` carries children.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String),
    Styled(Style, Vec<Span>),
    /// Inline `code`; no nested styling.
    Code(String),
    /// A fenced or indented code block, with an optional language.
    CodeBlock { lang: Option<String>, code: String },
    /// `[label](url)` or a bare URL. `label` already parsed into spans.
    Link { url: String, children: Vec<Span> },
    /// One `<...>` entity, resolved to display text by the UI.
    Entity(Entity),
    /// A hard line break.
    Break,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    /// `> ` quoted line(s).
    Quote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entity {
    User(u64),
    Channel(u64),
    Role(u64),
    /// `<:name:id>` or animated `<a:name:id>`.
    Emoji { name: String, id: u64, animated: bool },
    /// `<t:unix[:style]>`.
    Timestamp { unix: i64, style: char },
}

/// Parse a whole message body.
pub fn parse(input: &str) -> Vec<Span> {
    let mut blocks = Vec::new();
    let mut rest = input;
    // Code blocks and quotes are line/region structures, handled before inline.
    while !rest.is_empty() {
        if let Some((block, after)) = take_code_block(rest) {
            blocks.push(block);
            rest = after;
        } else if let Some((block, after)) = take_quote(rest) {
            blocks.push(block);
            rest = after;
        } else {
            // Consume up to the next code fence or quote, inline-parse it.
            let end = next_block_boundary(rest);
            let (chunk, after) = rest.split_at(end);
            blocks.extend(parse_inline(chunk));
            rest = after;
        }
    }
    blocks
}

/// Where the next ``` fence or line-start `> ` begins, or the end.
fn next_block_boundary(s: &str) -> usize {
    let mut at_line_start = true;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if at_line_start {
            if s[i..].starts_with("```") {
                return i;
            }
            if s[i..].starts_with("> ") || s[i..].starts_with(">>> ") {
                return i;
            }
        }
        at_line_start = bytes[i] == b'\n';
        i += 1;
    }
    s.len()
}

fn take_code_block(s: &str) -> Option<(Span, &str)> {
    let body = s.strip_prefix("```")?;
    let end = body.find("```")?;
    let (inner, after) = body.split_at(end);
    let after = &after[3..];
    // First line is a language tag only if it is a single bare word.
    let (lang, code) = match inner.split_once('\n') {
        Some((first, rest))
            if !first.is_empty()
                && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '#') =>
        {
            (Some(first.to_string()), rest)
        }
        _ => (None, inner),
    };
    let code = code.strip_prefix('\n').unwrap_or(code);
    Some((Span::CodeBlock { lang, code: code.trim_end_matches('\n').to_string() }, after))
}

fn take_quote(s: &str) -> Option<(Span, &str)> {
    if let Some(rest) = s.strip_prefix(">>> ") {
        // Multiline quote: everything to the end.
        return Some((Span::Styled(Style::Quote, parse_inline(rest)), ""));
    }
    if !s.starts_with("> ") {
        return None;
    }
    // Consecutive `> ` lines form one quote.
    let mut quoted = String::new();
    let mut consumed = 0;
    for line in s.split_inclusive('\n') {
        match line.strip_prefix("> ") {
            Some(content) => {
                quoted.push_str(content);
                consumed += line.len();
            }
            None => break,
        }
    }
    let after = &s[consumed..];
    Some((Span::Styled(Style::Quote, parse_inline(quoted.trim_end_matches('\n'))), after))
}

/// Inline markup within a block of text.
fn parse_inline(input: &str) -> Vec<Span> {
    Parser { bytes: input.as_bytes(), input, pos: 0 }.run(None)
}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    /// Parse until `close` (a delimiter) is found or input ends. Returns the
    /// spans; the caller skips the closing delimiter.
    fn run(&mut self, close: Option<&str>) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, spans: &mut Vec<Span>| {
            if !text.is_empty() {
                spans.push(Span::Text(std::mem::take(text)));
            }
        };

        while self.pos < self.bytes.len() {
            let rest = &self.input[self.pos..];
            if let Some(close) = close
                && rest.starts_with(close) {
                    break;
                }

            if rest.starts_with('\n') {
                flush(&mut text, &mut spans);
                spans.push(Span::Break);
                self.pos += 1;
                continue;
            }
            // Inline code: run of backticks, closed by the same run.
            if rest.starts_with('`')
                && let Some(span) = self.take_inline_code() {
                    flush(&mut text, &mut spans);
                    spans.push(span);
                    continue;
                }
            // `<...>` entity.
            if rest.starts_with('<')
                && let Some((entity, len)) = parse_entity(rest) {
                    flush(&mut text, &mut spans);
                    spans.push(Span::Entity(entity));
                    self.pos += len;
                    continue;
                }
            // `[label](url)` link.
            if rest.starts_with('[')
                && let Some((span, len)) = self.take_link() {
                    flush(&mut text, &mut spans);
                    spans.push(span);
                    self.pos += len;
                    continue;
                }
            // Bare URL.
            if rest.starts_with("http://") || rest.starts_with("https://") {
                let len = url_len(rest);
                flush(&mut text, &mut spans);
                let url = rest[..len].to_string();
                spans.push(Span::Link { url: url.clone(), children: vec![Span::Text(url)] });
                self.pos += len;
                continue;
            }
            // Style delimiters, longest first so ** beats *.
            if let Some((style, delim)) = self.match_delim() {
                self.pos += delim.len();
                let children = self.run(Some(delim));
                // Only accept if the closing delimiter was actually there.
                if self.input[self.pos..].starts_with(delim) {
                    self.pos += delim.len();
                    flush(&mut text, &mut spans);
                    spans.push(Span::Styled(style, children));
                    continue;
                }
                // Unterminated: emit the delimiter literally, keep children.
                flush(&mut text, &mut spans);
                spans.push(Span::Text(delim.to_string()));
                spans.extend(children);
                continue;
            }

            // Default: copy one char.
            let ch = rest.chars().next().unwrap();
            text.push(ch);
            self.pos += ch.len_utf8();
        }
        flush(&mut text, &mut spans);
        spans
    }

    fn match_delim(&self) -> Option<(Style, &'static str)> {
        let rest = &self.input[self.pos..];
        const DELIMS: &[(&str, Style)] = &[
            ("***", Style::Bold), // treated as bold; italic nests inside via **/*
            ("**", Style::Bold),
            ("__", Style::Underline),
            ("~~", Style::Strikethrough),
            ("||", Style::Spoiler),
            ("*", Style::Italic),
            ("_", Style::Italic),
        ];
        DELIMS.iter().find(|(d, _)| rest.starts_with(d)).map(|(d, s)| (*s, *d))
    }

    fn take_inline_code(&mut self) -> Option<Span> {
        let rest = &self.input[self.pos..];
        let ticks = rest.bytes().take_while(|&b| b == b'`').count();
        let fence = &rest[..ticks];
        let body = &rest[ticks..];
        let end = body.find(fence)?;
        let code = &body[..end];
        self.pos += ticks + end + ticks;
        Some(Span::Code(code.trim().to_string()))
    }

    fn take_link(&mut self) -> Option<(Span, usize)> {
        let rest = &self.input[self.pos..];
        let close = rest.find("](")?;
        let label = &rest[1..close];
        if label.contains('\n') {
            return None;
        }
        let after = &rest[close + 2..];
        let paren = after.find(')')?;
        let url = &after[..paren];
        if url.is_empty() || url.contains(char::is_whitespace) {
            return None;
        }
        let total = close + 2 + paren + 1;
        Some((Span::Link { url: url.to_string(), children: parse_inline(label) }, total))
    }
}

fn url_len(s: &str) -> usize {
    let mut end = s.find(|c: char| c.is_whitespace() || c == '>' || c == ')').unwrap_or(s.len());
    // Don't swallow trailing sentence punctuation.
    while end > 0 && matches!(s.as_bytes()[end - 1], b'.' | b',' | b'!' | b'?' | b';' | b':') {
        end -= 1;
    }
    end
}

/// Parse one `<...>` entity at the start of `s`. Returns the entity and the
/// number of bytes consumed (including the angle brackets).
fn parse_entity(s: &str) -> Option<(Entity, usize)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    let len = end + 1;
    let entity = if let Some(id) = inner.strip_prefix("@&") {
        Entity::Role(id.parse().ok()?)
    } else if let Some(id) = inner.strip_prefix('@') {
        // `<@123>` or `<@!123>` (nickname mention).
        Entity::User(id.trim_start_matches('!').parse().ok()?)
    } else if let Some(id) = inner.strip_prefix('#') {
        Entity::Channel(id.parse().ok()?)
    } else if let Some(rest) = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':')) {
        let animated = inner.starts_with("a:");
        let (name, id) = rest.split_once(':')?;
        Entity::Emoji { name: name.to_string(), id: id.parse().ok()?, animated }
    } else {
        let rest = inner.strip_prefix("t:")?;
        let (unix, style) = match rest.split_once(':') {
            Some((u, s)) => (u.parse().ok()?, s.chars().next().unwrap_or('f')),
            None => (rest.parse().ok()?, 'f'),
        };
        Entity::Timestamp { unix, style }
    };
    Some((entity, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Span {
        Span::Text(s.into())
    }

    #[test]
    fn plain() {
        assert_eq!(parse("hello world"), vec![text("hello world")]);
    }

    #[test]
    fn bold_italic_nesting() {
        assert_eq!(
            parse("**bold *and italic* end**"),
            vec![Span::Styled(
                Style::Bold,
                vec![text("bold "), Span::Styled(Style::Italic, vec![text("and italic")]), text(" end")],
            )]
        );
    }

    #[test]
    fn all_simple_styles() {
        assert_eq!(parse("__u__"), vec![Span::Styled(Style::Underline, vec![text("u")])]);
        assert_eq!(parse("~~s~~"), vec![Span::Styled(Style::Strikethrough, vec![text("s")])]);
        assert_eq!(parse("||spoiler||"), vec![Span::Styled(Style::Spoiler, vec![text("spoiler")])]);
    }

    #[test]
    fn unterminated_delimiter_is_literal() {
        assert_eq!(parse("a ** b"), vec![text("a "), text("**"), text(" b")]);
    }

    #[test]
    fn inline_code_keeps_contents_literal() {
        assert_eq!(parse("use `**not bold**`"), vec![text("use "), Span::Code("**not bold**".into())]);
    }

    #[test]
    fn code_block_with_lang() {
        assert_eq!(
            parse("```rust\nfn main() {}\n```"),
            vec![Span::CodeBlock { lang: Some("rust".into()), code: "fn main() {}".into() }]
        );
    }

    #[test]
    fn code_block_without_lang() {
        assert_eq!(
            parse("```\njust text\n```"),
            vec![Span::CodeBlock { lang: None, code: "just text".into() }]
        );
    }

    #[test]
    fn quote_consumes_consecutive_lines() {
        assert_eq!(
            parse("> a\n> b\nc"),
            vec![
                Span::Styled(Style::Quote, vec![text("a"), Span::Break, text("b")]),
                text("c"),
            ]
        );
    }

    #[test]
    fn entities() {
        assert_eq!(parse("<@123>"), vec![Span::Entity(Entity::User(123))]);
        assert_eq!(parse("<@!123>"), vec![Span::Entity(Entity::User(123))]);
        assert_eq!(parse("<@&7>"), vec![Span::Entity(Entity::Role(7))]);
        assert_eq!(parse("<#9>"), vec![Span::Entity(Entity::Channel(9))]);
        assert_eq!(
            parse("<:blob:42>"),
            vec![Span::Entity(Entity::Emoji { name: "blob".into(), id: 42, animated: false })]
        );
        assert_eq!(
            parse("<a:spin:43>"),
            vec![Span::Entity(Entity::Emoji { name: "spin".into(), id: 43, animated: true })]
        );
        assert_eq!(parse("<t:1700000000:R>"), vec![Span::Entity(Entity::Timestamp { unix: 1_700_000_000, style: 'R' })]);
    }

    #[test]
    fn links() {
        assert_eq!(
            parse("[docs](https://example.com)"),
            vec![Span::Link { url: "https://example.com".into(), children: vec![text("docs")] }]
        );
        assert_eq!(
            parse("see https://example.com."),
            vec![
                text("see "),
                Span::Link { url: "https://example.com".into(), children: vec![text("https://example.com")] },
                text("."),
            ]
        );
    }

    #[test]
    fn malformed_entity_is_text() {
        assert_eq!(parse("<@abc>"), vec![text("<@abc>")]);
        assert_eq!(parse("a < b"), vec![text("a < b")]);
    }

    #[test]
    fn mixed_message() {
        let spans = parse("hey <@5> check **this** out: `code`");
        assert_eq!(spans.len(), 6);
        assert_eq!(spans[0], text("hey "));
        assert_eq!(spans[1], Span::Entity(Entity::User(5)));
    }
}
