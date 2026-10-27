//! Parse Discord's message markup into a span tree the UI renders.
//!
//! Supports: bold, italic, underline, strikethrough, spoiler, inline code,
//! fenced code blocks, block quotes, headings, subtext, lists, links, and
//! the `<...>` entities (user / channel / role / custom emoji / timestamp).
//! Block rules follow the official client's.

/// A rendered span. Styles nest, so `Styled` carries children.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String),
    Styled(Style, Vec<Span>),
    /// Inline `code`; no nested styling.
    Code(String),
    /// A fenced or indented code block, with an optional language.
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    /// `[label](url)` or a bare URL. `label` already parsed into spans.
    Link {
        url: String,
        children: Vec<Span>,
    },
    /// One `<...>` entity, resolved to display text by the UI.
    Entity(Entity),
    /// A hard line break.
    Break,
    /// `# `, `## ` or `### ` at the start of a line.
    Heading {
        level: u8,
        children: Vec<Span>,
    },
    /// `-# ` at the start of a line: small print.
    Subtext(Vec<Span>),
    /// Lines starting `- `, `* ` or `1. `. Each item may hold blocks,
    /// nested lists among them.
    List {
        /// The first number of a numbered list.
        start: Option<u32>,
        items: Vec<Vec<Span>>,
    },
}

impl Span {
    /// The spans directly inside this one, list items' included.
    pub fn children(&self) -> &[Span] {
        match self {
            Span::Styled(_, children)
            | Span::Link { children, .. }
            | Span::Heading { children, .. }
            | Span::Subtext(children) => children,
            _ => &[],
        }
    }

    /// Every span inside this one, at any depth.
    pub fn descendants(&self) -> Vec<&Span> {
        let mut out = Vec::new();
        let mut stack: Vec<&Span> = vec![self];
        while let Some(span) = stack.pop() {
            let children: Box<dyn Iterator<Item = &Span>> = match span {
                Span::List { items, .. } => Box::new(items.iter().flatten()),
                other => Box::new(other.children().iter()),
            };
            for child in children {
                out.push(child);
                stack.push(child);
            }
        }
        out
    }
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
    Emoji {
        name: String,
        id: u64,
        animated: bool,
    },
    /// `<t:unix[:style]>`.
    Timestamp {
        unix: i64,
        style: char,
    },
}

/// Parse a whole message body.
pub fn parse(input: &str) -> Vec<Span> {
    parse_with(input, true)
}

/// Parse a message body, with headings and lists only if `headings`:
/// messages from before Discord had them show them as typed.
pub fn parse_with(input: &str, headings: bool) -> Vec<Span> {
    Blocks { headings, lists: headings, quotes: true, depth: 0 }.parse(input)
}

/// Lists nest at most this deep; deeper, the markers are text.
const MAX_LIST_DEPTH: u8 = 10;

/// Which block rules apply where: not quotes in quotes, nor headings in lists.
#[derive(Clone, Copy)]
struct Blocks {
    headings: bool,
    lists: bool,
    quotes: bool,
    depth: u8,
}

impl Blocks {
    fn parse(self, input: &str) -> Vec<Span> {
        let mut blocks = Vec::new();
        let mut rest = input;
        // Blocks start at the start of a line; the text between is inline.
        while !rest.is_empty() {
            if let Some((block, after)) = self.take_block(rest) {
                blocks.push(block);
                rest = after;
            } else {
                let end = self.next_block(rest);
                let (chunk, after) = rest.split_at(end);
                blocks.extend(parse_inline(chunk));
                rest = after;
            }
        }
        blocks
    }

    fn take_block(self, s: &str) -> Option<(Span, &str)> {
        take_code_block(s)
            .or_else(|| self.quotes.then(|| take_quote(s, self)).flatten())
            .or_else(|| self.headings.then(|| take_heading(s)).flatten())
            .or_else(|| take_subtext(s))
            .or_else(|| self.lists.then(|| take_list(s, self)).flatten())
    }

    /// Where the next line that starts a block begins, or the end.
    fn next_block(self, s: &str) -> usize {
        let mut line_start = s.find('\n').map(|i| i + 1);
        while let Some(start) = line_start.filter(|&i| i < s.len()) {
            if self.take_block(&s[start..]).is_some() {
                return start;
            }
            line_start = s[start..].find('\n').map(|i| start + i + 1);
        }
        s.len()
    }
}

/// The first line of `s`, and what follows its newline.
fn split_line(s: &str) -> (&str, &str) {
    match s.split_once('\n') {
        Some((line, after)) => (line, after),
        None => (s, ""),
    }
}

fn take_code_block(s: &str) -> Option<(Span, &str)> {
    let body = s.strip_prefix("```")?;
    let end = body.find("```")?;
    let (inner, after) = body.split_at(end);
    let after = &after[3..];
    // First line is a language tag only if it is a single bare word.
    let (lang, code) = match inner.split_once('\n') {
        Some((first, rest))
            if !first.is_empty() && first.chars().all(|c| c.is_ascii_alphanumeric() || "+-#._".contains(c)) =>
        {
            (Some(first.to_string()), rest)
        }
        _ => (None, inner),
    };
    let code = code.strip_prefix('\n').unwrap_or(code);
    Some((Span::CodeBlock { lang, code: code.trim_end_matches('\n').to_string() }, after))
}

fn take_quote(s: &str, blocks: Blocks) -> Option<(Span, &str)> {
    let inner = Blocks { quotes: false, ..blocks };
    if let Some(rest) = s.strip_prefix(">>> ") {
        // Multiline quote: everything to the end.
        return Some((Span::Styled(Style::Quote, inner.parse(rest)), ""));
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
    Some((Span::Styled(Style::Quote, inner.parse(quoted.trim_end_matches('\n'))), after))
}

/// `#`, `##` or `###`, then space, at the start of `s` (after spaces).
/// Returns the level and what follows the space.
fn heading_marker(s: &str) -> Option<(u8, &str)> {
    let hashes = s.bytes().take_while(|&b| b == b'#').count();
    let rest = &s[hashes..];
    ((1..=3).contains(&hashes) && rest.starts_with(char::is_whitespace)).then_some((hashes as u8, rest))
}

fn take_heading(s: &str) -> Option<(Span, &str)> {
    let (line, after) = split_line(s);
    let (level, rest) = heading_marker(line.trim_start_matches(' '))?;
    let text = rest.trim();
    // `# # x` is not a heading of `# x`.
    if heading_marker(text).is_some() {
        return None;
    }
    // Closing hashes are dropped, but the text keeps at least a character.
    let text = match text.trim_end_matches('#') {
        "" => &text[..1.min(text.len())],
        trimmed => trimmed.trim_end(),
    };
    if text.is_empty() {
        return None;
    }
    Some((Span::Heading { level, children: parse_inline(text) }, after))
}

fn take_subtext(s: &str) -> Option<(Span, &str)> {
    let (line, after) = split_line(s);
    let rest = line.trim_start_matches(' ').strip_prefix("-#")?;
    let text = rest.strip_prefix(' ')?.trim_start_matches(' ');
    if text.is_empty() || text.starts_with("-#") {
        return None;
    }
    Some((Span::Subtext(parse_inline(text)), after))
}

/// A list marker and its space at the start of `s`: its length, and the
/// number of a numbered one.
fn list_marker(s: &str) -> Option<(usize, Option<u32>)> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    let (len, number) = if digits > 0 && s[digits..].starts_with('.') {
        // Numbers past a billion are clamped, as are those below one.
        let number = s[..digits].parse::<u64>().unwrap_or(u64::MAX).clamp(1, 1_000_000_000) as u32;
        (digits + 1, Some(number))
    } else if s.starts_with(['-', '*']) {
        (1, None)
    } else {
        return None;
    };
    s[len..].starts_with(' ').then_some((len + 1, number))
}

fn take_list(s: &str, blocks: Blocks) -> Option<(Span, &str)> {
    if blocks.depth >= MAX_LIST_DEPTH {
        return None;
    }
    let indent = s.bytes().take_while(|&b| b == b' ').count();
    let (_, start) = list_marker(&s[indent..])?;
    // The list runs on through lines that are indented, or that start
    // another item at its own indent.
    let at_item = |line: &str| {
        line.get(..indent).is_some_and(|i| i.bytes().all(|b| b == b' ')) && list_marker(&line[indent..]).is_some()
    };
    let mut end = 0;
    // Each item's text, and how far its text is indented.
    let mut items: Vec<(String, usize)> = Vec::new();
    for (n, line) in s.split_inclusive('\n').enumerate() {
        let item_start = at_item(line);
        if n > 0 && !line.starts_with(' ') && !item_start {
            break;
        }
        end += line.len();
        if item_start {
            let (marker, _) = list_marker(&line[indent..]).expect("checked");
            items.push((line[indent + marker..].to_owned(), indent + marker));
        } else if let Some((item, offset)) = items.last_mut() {
            // Lines under an item belong to it, indented as if it started
            // at the margin.
            let spaces = line.len() - line.trim_start_matches(' ').len();
            item.push_str(&line[spaces.min(*offset)..]);
        }
    }
    let inner = Blocks { headings: false, quotes: false, depth: blocks.depth + 1, ..blocks };
    let items = items.iter().map(|(item, _)| inner.parse(item.trim_end_matches('\n'))).collect();
    Some((Span::List { start, items }, &s[end..]))
}

/// The language a code block's tag names, by the official client's
/// aliases: `js` is `javascript`, `rs` is `rust`, …. Other tags are
/// returned lowercased.
pub fn code_language(tag: &str) -> String {
    let tag = tag.to_ascii_lowercase();
    let name = match tag.as_str() {
        "js" | "mjs" | "cjs" | "jsx" | "node" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "py" | "gyp" => "python",
        "rs" => "rust",
        "cs" | "csharp" | "c#" => "c-sharp",
        "h" | "hpp" | "cc" | "cxx" | "c++" => "cpp",
        "yml" => "yaml",
        "sh" | "shell" | "console" => "bash",
        "kt" => "kotlin",
        "rb" => "ruby",
        "golang" => "go",
        "md" => "markdown",
        "jsonc" => "json",
        "htm" | "xhtml" | "hbs" | "erb" => "html",
        "svg" | "xsl" | "rss" => "xml",
        "less" => "css",
        "ps1" | "pwsh" => "powershell",
        "tf" => "hcl",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "patch" => "diff",
        "pgsql" | "postgres" => "sql",
        _ => return tag,
    };
    name.to_owned()
}

/// A stretch of an `ansi` code block, and how its escape codes style it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ansi {
    pub text: String,
    pub bold: bool,
    pub underline: bool,
    /// One of the eight colours, 0 to 7 (codes 30 to 37 and 40 to 47).
    pub foreground: Option<u8>,
    pub background: Option<u8>,
}

/// Split an `ansi` code block by the escape codes in it (`ESC[1;31m`),
/// which are dropped. Unknown codes change nothing.
pub fn ansi(code: &str) -> Vec<Ansi> {
    let mut out: Vec<Ansi> = Vec::new();
    let mut current = Ansi::default();
    let mut rest = code;
    while !rest.is_empty() {
        let Some(escape) = rest.find("\u{1b}[") else {
            current.text.push_str(rest);
            break;
        };
        current.text.push_str(&rest[..escape]);
        let after = &rest[escape + 2..];
        let Some(end) = after.find(|c: char| !(c.is_ascii_digit() || c == ';')) else {
            break;
        };
        if !after[end..].starts_with('m') {
            // Not a style code: keep it as text.
            current.text.push_str(&rest[escape..escape + 2]);
            rest = after;
            continue;
        }
        let mut next = Ansi { text: String::new(), ..current.clone() };
        for code in after[..end].split(';') {
            match code.parse::<u8>().unwrap_or(0) {
                0 => next = Ansi::default(),
                1 => next.bold = true,
                4 => next.underline = true,
                n @ 30..=37 => next.foreground = Some(n - 30),
                n @ 40..=47 => next.background = Some(n - 40),
                _ => {}
            }
        }
        if !current.text.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current = next;
        rest = &after[end + 1..];
    }
    if !current.text.is_empty() {
        out.push(current);
    }
    out
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
                && rest.starts_with(close)
            {
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
                && let Some(span) = self.take_inline_code()
            {
                flush(&mut text, &mut spans);
                spans.push(span);
                continue;
            }
            // `<...>` entity.
            if rest.starts_with('<')
                && let Some((entity, len)) = parse_entity(rest)
            {
                flush(&mut text, &mut spans);
                spans.push(Span::Entity(entity));
                self.pos += len;
                continue;
            }
            // `[label](url)` link.
            if rest.starts_with('[')
                && let Some((span, len)) = self.take_link()
            {
                flush(&mut text, &mut spans);
                spans.push(span);
                self.pos += len;
                continue;
            }
            // Bare URL.
            if rest.starts_with("http://") || rest.starts_with("https://") {
                // Inside a span, the URL ends where the span does: Discord
                // reads `||https://a.com||` as a spoilered link.
                let mut len = url_len(rest);
                if let Some(end) = close.and_then(|close| rest[..len].find(close)) {
                    len = end;
                }
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
        let link = |url: &str| Span::Link { url: url.into(), children: vec![text(url)] };
        assert_eq!(parse("||https://a.com||"), vec![Span::Styled(Style::Spoiler, vec![link("https://a.com")])]);
        assert_eq!(parse("https://a.com/x||y"), vec![link("https://a.com/x||y")], "only a span's own close ends a URL");
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
    fn code_languages() {
        assert_eq!(code_language("JS"), "javascript");
        assert_eq!(code_language("c++"), "cpp");
        assert_eq!(code_language("Rust"), "rust");
        assert_eq!(code_language("brainfuck"), "brainfuck");
    }

    #[test]
    fn ansi_codes() {
        let plain = |text: &str| Ansi { text: text.into(), ..Default::default() };
        assert_eq!(ansi("plain"), vec![plain("plain")]);
        assert_eq!(
            ansi("a\u{1b}[1;31mred\u{1b}[0m b \u{1b}[4;44mc"),
            vec![
                plain("a"),
                Ansi { text: "red".into(), bold: true, foreground: Some(1), ..Default::default() },
                plain(" b "),
                Ansi { text: "c".into(), underline: true, background: Some(4), ..Default::default() },
            ]
        );
        assert_eq!(ansi("x\u{1b}[2Jy"), vec![plain("x\u{1b}[2Jy")], "only style codes are read");
    }

    #[test]
    fn code_block_without_lang() {
        assert_eq!(parse("```\njust text\n```"), vec![Span::CodeBlock { lang: None, code: "just text".into() }]);
    }

    #[test]
    fn quote_consumes_consecutive_lines() {
        assert_eq!(
            parse("> a\n> b\nc"),
            vec![Span::Styled(Style::Quote, vec![text("a"), Span::Break, text("b")]), text("c"),]
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
        assert_eq!(
            parse("<t:1700000000:R>"),
            vec![Span::Entity(Entity::Timestamp { unix: 1_700_000_000, style: 'R' })]
        );
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

    fn heading(level: u8, s: &str) -> Span {
        Span::Heading { level, children: vec![text(s)] }
    }

    #[test]
    fn headings() {
        assert_eq!(parse("# Big"), vec![heading(1, "Big")]);
        assert_eq!(parse("## Mid ##\nafter"), vec![heading(2, "Mid"), text("after")]);
        assert_eq!(parse("before\n  ### Small"), vec![text("before"), Span::Break, heading(3, "Small")]);
        assert_eq!(parse("#### four"), vec![text("#### four")]);
        assert_eq!(parse("#no space"), vec![text("#no space")]);
        assert_eq!(parse("# # x"), vec![text("# # x")]);
        assert_eq!(parse("a # b"), vec![text("a # b")], "only at the start of a line");
        assert_eq!(parse_with("# old", false), vec![text("# old")], "older messages show the hashes");
    }

    #[test]
    fn subtext() {
        assert_eq!(
            parse("-# small *print*\nnext"),
            vec![Span::Subtext(vec![text("small "), Span::Styled(Style::Italic, vec![text("print")])]), text("next"),]
        );
        assert_eq!(parse("-#nospace"), vec![text("-#nospace")]);
        assert_eq!(parse("-# -# twice"), vec![text("-# -# twice")]);
    }

    #[test]
    fn lists() {
        assert_eq!(
            parse("- a\n* b\nafter"),
            vec![Span::List { start: None, items: vec![vec![text("a")], vec![text("b")]] }, text("after"),]
        );
        assert_eq!(
            parse("3. three\n9. four"),
            vec![Span::List { start: Some(3), items: vec![vec![text("three")], vec![text("four")]] }]
        );
        assert_eq!(parse("0. zero"), vec![Span::List { start: Some(1), items: vec![vec![text("zero")]] }]);
        assert_eq!(parse("-not a list"), vec![text("-not a list")]);
        assert_eq!(parse_with("- old", false), vec![text("- old")]);
    }

    #[test]
    fn nested_lists() {
        let list = |items: Vec<Vec<Span>>| Span::List { start: None, items };
        assert_eq!(
            parse("- a\n  - b\n  - c\n- d"),
            vec![list(vec![
                vec![text("a"), Span::Break, list(vec![vec![text("b")], vec![text("c")]])],
                vec![text("d")],
            ])]
        );
        // Continued lines stay in their item; headings don't apply inside.
        assert_eq!(parse("- a\n  # b"), vec![list(vec![vec![text("a"), Span::Break, text("# b")]])]);
        // Lists stop nesting at some depth.
        let deep: String = (0..12).map(|depth| format!("{}- x\n", "  ".repeat(depth))).collect();
        let mut depth = 0;
        let mut span = &parse(&deep)[0];
        while let Some(inner) = span.descendants().into_iter().find(|s| matches!(s, Span::List { .. })) {
            depth += 1;
            span = inner;
        }
        assert_eq!(depth + 1, MAX_LIST_DEPTH as usize);
    }

    #[test]
    fn blocks_in_quotes() {
        assert_eq!(
            parse("> # title\n> - item"),
            vec![Span::Styled(
                Style::Quote,
                vec![heading(1, "title"), Span::List { start: None, items: vec![vec![text("item")]] }]
            )]
        );
        assert_eq!(parse("> > a"), vec![Span::Styled(Style::Quote, vec![text("> a")])], "no quotes in quotes");
    }
}
