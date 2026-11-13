//! The message box while a command is in it: the client's
//! `applicationCommand` node (870748), the command's name and then one pill
//! per option given (84046 `em`), in the order they were added.
//!
//! The box holds it as text, in the client's own plain form, with tags:
//!
//! ```text
//! /roll count:3 size:D6
//! └name┘└key─┘v└e┘└key┘└v┘└e┘
//! ```
//!
//! The name is `/<display name>` and a space; a key is `<display name>:`,
//! which can't be edited; the value runs to the key's end, a space that
//! closes the pill (the client's zero-width span after it). Text between
//! the name and the first key, and after an end, is outside every pill. The
//! UI hands over the text and where its key and end tags are ([`Marked`]),
//! and applies the [`Change`]s that come back; everything that decides is
//! here. Offsets are in chars.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::commands::{Command, CommandOption, Values, option_type};

/// The box's text and where its structure tags are.
#[derive(Clone, Debug)]
pub struct Marked<'a> {
    pub text: &'a str,
    /// The `pill-key` ranges.
    pub keys: Vec<Range<usize>>,
    /// The `pill-end` spaces.
    pub ends: Vec<usize>,
}

/// The box as the client's `applicationCommand` node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// `/<display name>`; the space after it is at `name.end`.
    pub name: Range<usize>,
    pub pills: Vec<Pill>,
    /// Where text outside the pills is, which typed keys are looked for in:
    /// from the name's space to the first key, and from each end to the next
    /// key.
    pub outside: Vec<Range<usize>>,
}

/// An option given (`applicationCommandOption`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pill {
    /// The option's name.
    pub option: String,
    pub key: Range<usize>,
    pub value: Range<usize>,
    /// The space that closes it; where it would be, if it went missing.
    pub end: usize,
}

/// Where the caret goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caret {
    At(usize),
    /// A pill's whole value, as the client selects it.
    Select(Range<usize>),
}

/// What a piece of inserted text is tagged as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Plain,
    Name,
    Key,
    End,
}

/// One edit of the box. A [`Change`]'s edits apply in order, and their
/// offsets are in the text as it was before any of them: they go from the
/// end of the text to its start, and inserts at one place come last part
/// first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Delete(Range<usize>),
    Insert { at: usize, text: String, part: Part },
}

/// Edits, and where the caret ends up in the new text (none: where the
/// edits leave it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub edits: Vec<Edit>,
    pub caret: Option<Caret>,
}

impl Change {
    /// `text` after the edits.
    pub fn apply(&self, text: &str) -> String {
        let mut chars: Vec<char> = text.chars().collect();
        for edit in &self.edits {
            match edit {
                Edit::Delete(range) => {
                    chars.drain(range.clone());
                }
                Edit::Insert { at, text, .. } => {
                    chars.splice(*at..*at, text.chars());
                }
            }
        }
        chars.into_iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.caret.is_none()
    }
}

// ---- Reading the box ----

/// The line as nodes: what the text says, whatever its spacing.
#[derive(Clone, Debug, PartialEq)]
enum Node {
    Text(String),
    Pill { option: String, value: String },
}

/// What the box holds now: its line, its nodes (the first always the text
/// after the name), and each char's tag as far as structure goes.
struct Parsed {
    chars: Vec<char>,
    tags: Vec<Part>,
    line: Line,
    nodes: Vec<Node>,
}

fn name_len(command: &Command) -> usize {
    1 + command.display_name.chars().count()
}

fn option<'c>(command: &'c Command, name: &str) -> Option<&'c CommandOption> {
    command.options.iter().find(|o| o.name == name)
}

fn key_text(option: &CommandOption) -> String {
    format!("{}:", option.display_name)
}

fn slice(chars: &[char], range: Range<usize>) -> String {
    chars[range.start.min(chars.len())..range.end.min(chars.len())].iter().collect()
}

fn parse(command: &Command, marked: &Marked<'_>) -> Parsed {
    let chars: Vec<char> = marked.text.chars().collect();
    let n = chars.len();
    let name = 0..name_len(command).min(n);
    // Keys that are an option's, each option once, in order.
    let mut keys: Vec<Range<usize>> = marked.keys.iter().filter(|k| k.end <= n && k.start < k.end).cloned().collect();
    keys.sort_by_key(|k| k.start);
    let mut valid: Vec<(String, Range<usize>)> = Vec::new();
    for key in keys {
        let text = slice(&chars, key.clone());
        let found = command.options.iter().find(|o| key_text(o) == text);
        if let Some(found) = found
            && !valid.iter().any(|(name, _)| *name == found.name)
            && valid.last().is_none_or(|(_, last)| last.end <= key.start)
        {
            valid.push((found.name.clone(), key));
        }
    }
    let starts: Vec<usize> = valid.iter().map(|(_, k)| k.start).collect();
    // The text before a key keeps one space apart from it, which isn't its own.
    let content = |range: Range<usize>, before_key: bool| {
        let mut end = range.end;
        if before_key && end > range.start && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        range.start..end
    };
    let lead_start = if chars.get(name.end) == Some(&' ') { name.end + 1 } else { name.end };
    let first = starts.first().copied().unwrap_or(n).max(lead_start);
    let mut nodes = vec![Node::Text(slice(&chars, content(lead_start..first, !valid.is_empty())))];
    let mut outside = Vec::with_capacity(valid.len() + 1);
    outside.push(name.end.min(first)..first);
    let mut pills = Vec::new();
    for (i, (option, key)) in valid.iter().enumerate() {
        let next = starts.get(i + 1).copied().unwrap_or(n);
        let end = marked.ends.iter().copied().filter(|&e| key.end <= e && e < next && chars[e] == ' ').min();
        let (value, after) = match end {
            Some(end) => (key.end..end, end + 1),
            None => {
                let value = content(key.end..next, i + 1 < valid.len());
                (value.clone(), value.end)
            }
        };
        let end = end.unwrap_or(value.end);
        nodes.push(Node::Pill { option: option.clone(), value: slice(&chars, value.clone()) });
        let text = slice(&chars, content(after..next, i + 1 < valid.len()));
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }
        outside.push(after.min(next)..next);
        pills.push(Pill { option: option.clone(), key: key.clone(), value, end });
    }
    let mut tags = vec![Part::Plain; n];
    for pill in &pills {
        tags[pill.key.clone()].fill(Part::Key);
        if pill.end < n && marked.ends.contains(&pill.end) {
            tags[pill.end] = Part::End;
        }
    }
    Parsed { chars, tags, line: Line { name, pills, outside }, nodes }
}

/// The line the box holds (`n$` in 317681): its keys taken as the options
/// whose `name:` they are.
pub fn read(command: &Command, marked: &Marked<'_>) -> Line {
    parse(command, marked).line
}

/// Whether the box still holds the command (870748, before anything else on
/// a change): the text before the first key starts with `/<display name>`,
/// and, without pills, a space follows it.
pub fn still_command(command: &Command, line: &Line, text: &str) -> bool {
    let name = format!("/{}", command.display_name);
    let first: String = text.chars().take(line.pills.first().map_or(usize::MAX, |p| p.key.start)).collect();
    first.starts_with(&name) && (!line.pills.is_empty() || first[name.len()..].starts_with(' '))
}

/// The text a command that ended leaves in the box (`IQ` in 323350, as `j`
/// in 870748 keeps it): the line as it reads, without trailing space.
pub fn plain(text: &str) -> String {
    text.trim_end().to_owned()
}

// ---- Writing it back ----

/// The text and tags of `nodes`, and its line.
struct Rendered {
    chars: Vec<char>,
    parts: Vec<Part>,
    line: Line,
}

fn render(command: &Command, nodes: &[Node]) -> Rendered {
    let mut chars: Vec<char> = Vec::new();
    let mut parts = Vec::new();
    let mut push = |chars: &mut Vec<char>, text: &str, part: Part| {
        for c in text.chars() {
            chars.push(c);
            parts.push(part);
        }
    };
    push(&mut chars, &format!("/{} ", command.display_name), Part::Name);
    let name = 0..name_len(command);
    let mut pills = Vec::new();
    let mut outside = Vec::new();
    let mut segment = name.end;
    for (i, node) in nodes.iter().enumerate() {
        let pill_next = matches!(nodes.get(i + 1), Some(Node::Pill { .. }));
        match node {
            Node::Text(text) => {
                push(&mut chars, text, Part::Plain);
                if pill_next && !text.is_empty() {
                    push(&mut chars, " ", Part::Plain);
                }
            }
            Node::Pill { option: name, value } => {
                outside.push(segment..chars.len());
                let key_text = option(command, name).map_or_else(|| format!("{name}:"), key_text);
                let start = chars.len();
                push(&mut chars, &key_text, Part::Key);
                let key = start..chars.len();
                push(&mut chars, value, Part::Plain);
                let value = key.end..chars.len();
                let end = chars.len();
                push(&mut chars, " ", Part::End);
                pills.push(Pill { option: name.clone(), key, value, end });
                segment = chars.len();
            }
        }
    }
    outside.push(segment..chars.len());
    let parts = parts;
    Rendered { chars, parts, line: Line { name, pills, outside } }
}

/// What tells two texts apart: their chars and which are keys and ends.
fn same(a: (char, Part), b: (char, Part)) -> bool {
    let class = |p: Part| match p {
        Part::Key => 1,
        Part::End => 2,
        Part::Plain | Part::Name => 0,
    };
    a.0 == b.0 && class(a.1) == class(b.1)
}

/// The edits from what the box holds to `new`: the stretch where they
/// differ, replaced.
fn edits(old: &Parsed, new: &Rendered) -> (Vec<Edit>, Range<usize>, usize) {
    let (a, b) = (&old.chars, &new.chars);
    let pair = |chars: &[char], parts: &[Part], i: usize| (chars[i], parts[i]);
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && same(pair(a, &old.tags, prefix), pair(b, &new.parts, prefix)) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && same(pair(a, &old.tags, a.len() - 1 - suffix), pair(b, &new.parts, b.len() - 1 - suffix))
    {
        suffix += 1;
    }
    let (old_end, new_end) = (a.len() - suffix, b.len() - suffix);
    let mut edits = Vec::new();
    if old_end > prefix {
        edits.push(Edit::Delete(prefix..old_end));
    }
    edits.extend(inserts(new, prefix..new_end, prefix));
    (edits, prefix..old_end, new_end)
}

/// Where a caret at `at` is once `changed` (in the old text) became the new
/// text up to `new_end`: the same place outside it, else as far into the
/// new stretch as it was into the old one.
fn moved(at: usize, changed: &Range<usize>, new_end: usize) -> usize {
    if at <= changed.start {
        at
    } else if at >= changed.end {
        at + new_end - changed.end
    } else {
        (changed.start + (at - changed.start)).min(new_end)
    }
}

fn change_to(old: &Parsed, new: &Rendered, caret: Option<Caret>) -> Change {
    let (edits, _, _) = edits(old, new);
    Change { edits, caret }
}

/// What a range deletion, an undo or a paste left wrong, set right: ends
/// missing, ends or keys that are no pill's, keys whose text isn't an
/// option's any more, the name's space when pills follow (`O` in 870748),
/// the space between outside text and a key, and line breaks anywhere but
/// a STRING value, which become spaces (`insertText` in 68059). `caret`
/// stays where it was in what is kept.
pub fn repair(command: &Command, marked: &Marked<'_>, caret: usize) -> Change {
    let parsed = parse(command, marked);
    let nodes: Vec<Node> = parsed
        .nodes
        .iter()
        .map(|node| match node {
            Node::Text(text) => Node::Text(text.replace(['\r', '\n'], " ")),
            Node::Pill { option: name, value } => {
                let string = option(command, name).is_some_and(|o| o.kind == option_type::STRING);
                let value = if string { value.clone() } else { value.replace(['\r', '\n'], " ") };
                Node::Pill { option: name.clone(), value }
            }
        })
        .collect();
    let new = render(command, &nodes);
    let (edits, changed, new_end) = edits(&parsed, &new);
    if edits.is_empty() {
        return Change::default();
    }
    Change { edits, caret: Some(Caret::At(moved(caret, &changed, new_end))) }
}

// ---- Activation ----

/// A key typed in outside text (`pY` in 317681): the option, where its key
/// and value are, and its value trimmed.
struct Typed {
    option: String,
    from: usize,
    key_end: usize,
    value_end: usize,
    value: String,
}

/// The chars a key's name is made of (`[\p{L}\p{N}\p{sc=Deva}\p{sc=Thai}_-]`).
fn key_char(c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || c == '-'
        || ('\u{0900}'..='\u{097F}').contains(&c)
        || ('\u{0E00}'..='\u{0E7F}').contains(&c)
}

/// The keys typed in `segments` for the options in `remaining`, each once,
/// at a segment's start or after whitespace, by exact display name. A
/// value runs to the next key or the segment's end, trimmed.
fn typed_in(command: &Command, chars: &[char], segments: &[Range<usize>], mut remaining: Vec<String>) -> Vec<Typed> {
    let mut found: Vec<Typed> = Vec::new();
    for segment in segments {
        let mut at_segment: Vec<(String, usize, usize)> = Vec::new();
        let mut i = segment.start;
        while i < segment.end {
            if !key_char(chars[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < segment.end && key_char(chars[i]) {
                i += 1;
            }
            if i >= segment.end || chars[i] != ':' {
                continue;
            }
            i += 1;
            if start != segment.start && !chars[start - 1].is_whitespace() {
                continue;
            }
            let name: String = chars[start..i - 1].iter().collect();
            let Some(position) = remaining.iter().position(|r| *r == name) else { continue };
            remaining.remove(position);
            if let Some(option) = command.options.iter().find(|o| o.display_name == name) {
                at_segment.push((option.name.clone(), start, i));
            }
        }
        for (k, (option, from, key_end)) in at_segment.iter().enumerate() {
            let value_end = at_segment.get(k + 1).map_or(segment.end, |next| next.1);
            let value = slice(chars, *key_end..value_end).trim().to_owned();
            found.push(Typed { option: option.clone(), from: *from, key_end: *key_end, value_end, value });
        }
    }
    found
}

/// The display names of the options without a pill.
fn missing_names(command: &Command, line: &Line) -> Vec<String> {
    let present = |o: &&CommandOption| line.pills.iter().any(|p| p.option == o.name);
    command.options.iter().filter(|o| !present(o)).map(|o| o.display_name.clone()).collect()
}

/// The box once `command` became active over `typed`, what it held
/// (870748, on `commandChanged`): the keys typed become pills, then every
/// required option (or one with a value in `initial`) not typed, in the
/// command's order; the text after `/<name> ` before the first key goes to
/// the first of those that takes text, else stays after the name. The
/// caret goes into the first empty pill, else to the end of the last one
/// added, else to the end; with no pill added, the one-option rule follows
/// ([`single_option`]).
pub fn activate(command: &Command, typed: &str, initial: &Values) -> Change {
    let chars: Vec<char> = typed.chars().collect();
    let all: Vec<String> = command.options.iter().map(|o| o.display_name.clone()).collect();
    let keys = typed_in(command, &chars, std::slice::from_ref(&(0..chars.len())), all);
    let before: String = chars[..keys.first().map_or(chars.len(), |k| k.from)].iter().collect();
    let lower = before.to_lowercase();
    let mut leftover = String::new();
    for prefix in [format!("/{} ", command.display_name), format!("/{} ", command.name)] {
        if lower.starts_with(&prefix.to_lowercase()) {
            leftover = before.chars().skip(prefix.chars().count()).collect::<String>().trim().to_owned();
            break;
        }
    }
    let mut nodes = vec![Node::Text(String::new())];
    let mut first_empty: Option<String> = None;
    let mut last_added: Option<String> = None;
    for key in &keys {
        let value = initial.get(&key.option).cloned().unwrap_or_else(|| key.value.clone());
        if value.is_empty() && first_empty.is_none() {
            first_empty = Some(key.option.clone());
        }
        nodes.push(Node::Pill { option: key.option.clone(), value });
    }
    for option in &command.options {
        let given = initial.get(&option.name);
        if keys.iter().any(|k| k.option == option.name) || !(option.required || given.is_some()) {
            continue;
        }
        let value = if !leftover.is_empty() && option.kind != option_type::ATTACHMENT {
            last_added = Some(option.name.clone());
            std::mem::take(&mut leftover)
        } else {
            if given.is_none() {
                last_added = Some(option.name.clone());
            }
            given.cloned().unwrap_or_default()
        };
        if value.is_empty() && first_empty.is_none() {
            first_empty = Some(option.name.clone());
        }
        nodes.push(Node::Pill { option: option.name.clone(), value });
    }
    nodes[0] = Node::Text(leftover.replace(['\r', '\n'], " "));
    // The one-option rule, when no pill was added for a missing value.
    if last_added.is_none()
        && let Some(only) = single(command)
        && let [Node::Text(lead)] = &nodes[..]
        && !lead.is_empty()
        // `_` compares the line to the name lowercased: a name with capitals never matches.
        && command.display_name.to_lowercase() == command.display_name
    {
        let value = lead.clone();
        nodes = vec![Node::Text(String::new()), Node::Pill { option: only.name.clone(), value }];
        last_added = Some(only.name.clone());
    }
    let new = render(command, &nodes);
    let pill = |name: &Option<String>| name.as_ref().and_then(|n| new.line.pills.iter().find(|p| p.option == *n));
    let caret = match (pill(&first_empty), pill(&last_added)) {
        (Some(empty), _) => Caret::Select(empty.value.clone()),
        (None, Some(last)) => Caret::At(last.value.end),
        (None, None) => Caret::At(new.chars.len()),
    };
    let mut edits = Vec::new();
    let length = typed.chars().count();
    if length > 0 {
        edits.push(Edit::Delete(0..length));
    }
    edits.extend(inserts(&new, 0..new.chars.len(), 0));
    Change { edits, caret: Some(caret) }
}

/// The edits that insert `range` of `new` at `at`: one for each run of a
/// part, the last first.
fn inserts(new: &Rendered, range: Range<usize>, at: usize) -> Vec<Edit> {
    let mut runs: Vec<(String, Part)> = Vec::new();
    for i in range {
        match runs.last_mut() {
            Some((text, part)) if *part == new.parts[i] => text.push(new.chars[i]),
            _ => runs.push((new.chars[i].to_string(), new.parts[i])),
        }
    }
    runs.into_iter().rev().map(|(text, part)| Edit::Insert { at, text, part }).collect()
}

/// A command's only option, when the one-option rule applies to it: one
/// option, optional, taking text.
fn single(command: &Command) -> Option<&CommandOption> {
    match &command.options[..] {
        [only] if !only.required && only.kind != option_type::ATTACHMENT => Some(only),
        _ => None,
    }
}

// ---- Changes while it is active ----

/// Keys typed outside the pills become pills (`pY` in 317681, then
/// `textToInline` in 35277 for each): the value up to the next key or pill,
/// trimmed. A caret at a key's start stays before its pill; one inside the
/// typed range or at its end goes to the end of the new value. The text
/// after the name is trimmed after. `None` when no key was typed, and then
/// [`single_option`] applies.
pub fn typed_keys(command: &Command, marked: &Marked<'_>, caret: usize) -> Option<Change> {
    let parsed = parse(command, marked);
    let remaining = missing_names(command, &parsed.line);
    let typed = typed_in(command, &parsed.chars, &parsed.line.outside, remaining);
    if typed.is_empty() {
        return None;
    }
    // Rebuild the nodes, cutting each outside segment at its keys.
    let mut nodes: Vec<Node> = Vec::new();
    let mut pills = parsed.line.pills.iter();
    let mut landing: Option<(String, bool)> = None;
    for (s, segment) in parsed.line.outside.iter().enumerate() {
        if s > 0
            && let Some(pill) = pills.next()
        {
            nodes.push(Node::Pill { option: pill.option.clone(), value: slice(&parsed.chars, pill.value.clone()) });
        }
        let start = if s == 0 {
            if parsed.chars.get(segment.start) == Some(&' ') { segment.start + 1 } else { segment.start }
        } else {
            segment.start
        };
        let mut at = start;
        for key in typed.iter().filter(|k| segment.contains(&k.from)) {
            nodes.push(Node::Text(slice(&parsed.chars, at..key.from).trim_end().to_owned()));
            nodes.push(Node::Pill { option: key.option.clone(), value: key.value.clone() });
            if caret == key.from {
                landing = Some((key.option.clone(), false));
            } else if caret > key.from && caret <= key.value_end.max(key.key_end) {
                landing = Some((key.option.clone(), true));
            }
            at = key.value_end;
        }
        let text = slice(&parsed.chars, at..segment.end);
        let next_is_pill = s + 1 < parsed.line.outside.len();
        nodes.push(Node::Text(if next_is_pill { text.trim_end().to_owned() } else { text }));
    }
    let nodes = tidy(nodes, true);
    let new = render(command, &nodes);
    let caret = match landing {
        Some((option, end)) => new
            .line
            .pills
            .iter()
            .find(|p| p.option == option)
            .map(|p| if end { Caret::At(p.value.end) } else { Caret::At(p.key.start) }),
        None => {
            let (_, changed, new_end) = edits(&parsed, &new);
            Some(Caret::At(moved(caret, &changed, new_end)))
        }
    };
    Some(change_to(&parsed, &new, caret))
}

/// Nodes as the editor normalizes them: the first one text, trimmed when
/// `trim_lead` (the client trims the command's first text after turning
/// keys into pills); texts next to each other merged and empty ones after
/// pills dropped.
fn tidy(nodes: Vec<Node>, trim_lead: bool) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for node in nodes {
        if let (Some(Node::Text(last)), Node::Text(text)) = (out.last_mut(), &node) {
            last.push_str(text);
            continue;
        }
        if matches!(&node, Node::Text(text) if text.is_empty()) && !out.is_empty() {
            continue;
        }
        out.push(node);
    }
    match out.first_mut() {
        Some(Node::Text(lead)) if trim_lead => *lead = lead.trim().to_owned(),
        Some(Node::Text(_)) => {}
        _ => out.insert(0, Node::Text(String::new())),
    }
    out
}

/// The one-option rule (`_` in 870748): a command whose only option is
/// optional and takes text, with no pill yet, makes whatever follows
/// `/<name> ` that option's pill at once, untrimmed, with the caret at its
/// end (or before it, where the caret was at its start).
pub fn single_option(command: &Command, line: &Line, text: &str, caret: usize) -> Option<Change> {
    let only = single(command)?;
    if !line.pills.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    // The client compares the box's text to the name lowercased.
    let prefix = format!("/{} ", command.display_name).to_lowercase();
    let start = command.display_name.chars().count() + 2;
    if !text.starts_with(&prefix) || chars.len() <= start {
        return None;
    }
    let value = slice(&chars, start..chars.len());
    let parsed = parse(command, &Marked { text, keys: Vec::new(), ends: Vec::new() });
    let new = render(command, &[Node::Text(String::new()), Node::Pill { option: only.name.clone(), value }]);
    let pill = &new.line.pills[0];
    let caret = if caret <= start { Caret::At(pill.key.start) } else { Caret::At(pill.value.end) };
    Some(change_to(&parsed, &new, Some(caret)))
}

/// The pill holding the caret (`M3` in 317681).
pub fn active(line: &Line, caret: usize) -> Option<&Pill> {
    line.pills.iter().find(|p| p.key.end <= caret && caret <= p.end)
}

/// What each pill holds, as written, by option (`SQ` in 317681); options
/// without a pill are left out. `validate` and `run_options` trim where the
/// client does. A file pill holds the name of the file `staged` has for its
/// option, or nothing (`g` in 317681), never the text in it, so a run never
/// names a file it doesn't carry.
pub fn values(command: &Command, line: &Line, text: &str, staged: &Values) -> Values {
    let chars: Vec<char> = text.chars().collect();
    line.pills
        .iter()
        .map(|p| match option(command, &p.option) {
            Some(o) if o.kind == option_type::ATTACHMENT => {
                (p.option.clone(), staged.get(&p.option).cloned().unwrap_or_default())
            }
            _ => (p.option.clone(), slice(&chars, p.value.clone())),
        })
        .collect()
}

/// Each file pill's text set to what it shows (84046 `em`): the name of
/// the file `staged` has for its option, or nothing. A file pill is a void
/// node (870748's `isVoid`): whatever was typed or pasted into it goes.
/// `caret` stays where it was in what is kept.
pub fn file_texts(command: &Command, marked: &Marked<'_>, staged: &Values, caret: usize) -> Change {
    let parsed = parse(command, marked);
    let nodes: Vec<Node> = parsed
        .nodes
        .iter()
        .map(|node| match node {
            Node::Pill { option: name, .. }
                if option(command, name).is_some_and(|o| o.kind == option_type::ATTACHMENT) =>
            {
                Node::Pill { option: name.clone(), value: staged.get(name).cloned().unwrap_or_default() }
            }
            node => node.clone(),
        })
        .collect();
    let new = render(command, &nodes);
    let (edits, changed, new_end) = edits(&parsed, &new);
    if edits.is_empty() {
        return Change::default();
    }
    Change { edits, caret: Some(Caret::At(moved(caret, &changed, new_end))) }
}

/// Backspace or Delete at the caret ([`backspace`], [`delete`]), where a
/// file pill holding a file goes whole rather than a char of its name, as
/// the editor deletes a void node.
pub fn erase(command: &Command, line: &Line, text: &str, caret: usize, back: bool) -> Option<Change> {
    let filled = |at: usize| {
        line.pills.iter().find(|p| {
            !p.value.is_empty()
                && p.value.contains(&at)
                && option(command, &p.option).is_some_and(|o| o.kind == option_type::ATTACHMENT)
        })
    };
    let change = if back { backspace(line, text, caret) } else { delete(line, text, caret) };
    let touched = match change.as_ref().map(|c| &c.edits[..]) {
        Some([Edit::Delete(range)]) if range.len() == 1 => range.start,
        Some(_) => return change,
        // Inside a value, where the box deletes as usual.
        None if back => caret.checked_sub(1)?,
        None => caret,
    };
    filled(touched).map(remove).or(change)
}

/// The file option a pasted file goes to (84046's paste handler): the one
/// whose pill holds the caret, else the command's first with nothing in
/// `staged`, with a pill or not.
pub fn paste_option<'c>(command: &'c Command, holding: Option<&str>, staged: &Values) -> Option<&'c CommandOption> {
    let file = |o: &&CommandOption| o.kind == option_type::ATTACHMENT;
    holding
        .and_then(|name| option(command, name))
        .filter(file)
        .or_else(|| command.options.iter().filter(file).find(|o| !staged.contains_key(&o.name)))
}

/// Tab (`selectNextCommandOption` in 35277): the next pill's whole value,
/// else the end of the box.
pub fn next(line: &Line, text: &str, caret: usize) -> Caret {
    let holding = active(line, caret).map(|p| p.option.clone());
    line.pills
        .iter()
        .find(|p| p.key.start >= caret && Some(&p.option) != holding.as_ref())
        .map_or(Caret::At(text.chars().count()), |p| Caret::Select(p.value.clone()))
}

/// Shift+Tab (`selectPreviousCommandOption`): the previous pill's whole
/// value, else the end of the command's first text (the name and what was
/// typed after it), if the caret is past it.
pub fn previous(line: &Line, text: &str, caret: usize) -> Option<Caret> {
    let holding = active(line, caret).map(|p| p.option.clone());
    let found = line.pills.iter().rev().find(|p| p.end < caret && Some(&p.option) != holding.as_ref());
    if let Some(pill) = found {
        return Some(Caret::Select(pill.value.clone()));
    }
    let chars: Vec<char> = text.chars().collect();
    let lead = line.outside.first().cloned().unwrap_or(line.name.end..line.name.end);
    let lead_text = slice(&chars, lead.clone());
    let first_end =
        if lead_text.trim().is_empty() { line.name.end } else { lead.start + lead_text.trim_end().chars().count() };
    (caret > first_end).then_some(Caret::At(first_end))
}

/// An option picked from the options list (`K` in 496533, then `pY`): its
/// key in place of the word being typed, `word`, with the caret in its
/// empty value.
pub fn add(command: &Command, marked: &Marked<'_>, option: &str, word: Range<usize>) -> Change {
    let parsed = parse(command, marked);
    let mut nodes: Vec<Node> = Vec::new();
    let mut pills = parsed.line.pills.iter();
    for (s, segment) in parsed.line.outside.iter().enumerate() {
        if s > 0
            && let Some(pill) = pills.next()
        {
            nodes.push(Node::Pill { option: pill.option.clone(), value: slice(&parsed.chars, pill.value.clone()) });
        }
        let start =
            if s == 0 && parsed.chars.get(segment.start) == Some(&' ') { segment.start + 1 } else { segment.start };
        // What is before the next pill, without the space that keeps it apart.
        let pill_next = s + 1 < parsed.line.outside.len();
        let before_next = |text: String| match text.strip_suffix(' ') {
            Some(kept) if pill_next => kept.to_owned(),
            _ => text,
        };
        if segment.contains(&word.start) || (segment.end == word.start && segment.start <= word.start) {
            let word = word.start.max(start)..word.end.min(segment.end).max(word.start.max(start));
            nodes.push(Node::Text(slice(&parsed.chars, start..word.start).trim_end().to_owned()));
            nodes.push(Node::Pill { option: option.to_owned(), value: String::new() });
            nodes.push(Node::Text(before_next(slice(&parsed.chars, word.end..segment.end).trim_start().to_owned())));
        } else {
            nodes.push(Node::Text(before_next(slice(&parsed.chars, start..segment.end))));
        }
    }
    let nodes = tidy(nodes, false);
    let new = render(command, &nodes);
    let caret = new.line.pills.iter().find(|p| p.option == option).map(|p| Caret::At(p.value.start));
    change_to(&parsed, &new, caret)
}

/// Sending with required options left out (`insertCommandOption` in
/// 35277): an empty pill for each at the end, in the command's order.
pub fn add_missing(command: &Command, marked: &Marked<'_>) -> Change {
    let parsed = parse(command, marked);
    let mut nodes = parsed.nodes.clone();
    for option in &command.options {
        if option.required && !parsed.line.pills.iter().any(|p| p.option == option.name) {
            nodes.push(Node::Pill { option: option.name.clone(), value: String::new() });
        }
    }
    let new = render(command, &nodes);
    change_to(&parsed, &new, None)
}

/// The options whose pills deleting `selection` takes whole, key and all
/// (`deleteFragment` in 68059): none when it is within one pill; else a
/// start at a value's start counts from its key, an end at a value's end
/// counts past its end, and every pill inside is taken.
pub fn covered(line: &Line, selection: Range<usize>) -> Vec<String> {
    if line.pills.iter().any(|p| p.key.end <= selection.start && selection.end <= p.end) {
        return Vec::new();
    }
    let mut start = selection.start;
    let mut end = selection.end;
    for pill in &line.pills {
        if start == pill.value.start {
            start = pill.key.start;
        }
        if end == pill.value.end {
            end = pill.end + 1;
        }
    }
    line.pills.iter().filter(|p| start <= p.key.start && p.end < end).map(|p| p.option.clone()).collect()
}

/// A choice or mention picked for `option` (`insertAutocomplete` in 84046,
/// `removeInlineChildren`): its whole value replaced, the caret at its end;
/// the caller then moves on with [`next`].
pub fn set_value(line: &Line, option: &str, value: &str) -> Change {
    let Some(pill) = line.pills.iter().find(|p| p.option == option) else { return Change::default() };
    let mut edits = Vec::new();
    if !pill.value.is_empty() {
        edits.push(Edit::Delete(pill.value.clone()));
    }
    if !value.is_empty() {
        edits.push(Edit::Insert { at: pill.value.start, text: value.to_owned(), part: Part::Plain });
    }
    Change { edits, caret: Some(Caret::At(pill.value.start + value.chars().count())) }
}

fn remove(pill: &Pill) -> Change {
    Change { edits: vec![Edit::Delete(pill.key.start..pill.end + 1)], caret: Some(Caret::At(pill.key.start)) }
}

fn delete_at(at: usize) -> Change {
    Change { edits: vec![Edit::Delete(at..at + 1)], caret: Some(Caret::At(at)) }
}

/// Where the outside text before `pill` has its last char, if it has any:
/// one space apart from the key is the separator, not the text.
fn text_before(line: &Line, text: &[char], index: usize) -> Option<usize> {
    let pill = &line.pills[index];
    let segment = &line.outside[index];
    let start = if index == 0 { segment.start + 1 } else { segment.start };
    let mut end = pill.key.start;
    if end > start && text[end - 1].is_whitespace() {
        end -= 1;
    }
    (end > start).then_some(end - 1)
}

/// Backspace where the caret touches a pill (`R` in 68059, `deleteBackward`):
/// in an empty value, the pill goes; right after a pill, its last char goes
/// (or the pill, if empty); at a value's start, the char before the pill
/// goes (or the pill before it, likewise, or the name's last char). `None`
/// where the box deletes as usual. Ctrl+Backspace counts as one char here.
pub fn backspace(line: &Line, text: &str, caret: usize) -> Option<Change> {
    let chars: Vec<char> = text.chars().collect();
    for (i, pill) in line.pills.iter().enumerate() {
        if pill.value.is_empty() && caret == pill.value.start {
            return Some(remove(pill));
        }
        if caret == pill.end + 1 && chars.get(pill.end) == Some(&' ') {
            return Some(if pill.value.is_empty() { remove(pill) } else { delete_at(pill.value.end - 1) });
        }
        if caret == pill.value.start || caret == pill.key.start {
            return Some(before_pill(line, &chars, i));
        }
    }
    None
}

/// Backspace at the start of pill `index`'s value: what is before it.
fn before_pill(line: &Line, chars: &[char], index: usize) -> Change {
    if let Some(last) = text_before(line, chars, index) {
        return delete_at(last);
    }
    match index.checked_sub(1).map(|p| &line.pills[p]) {
        Some(previous) if previous.value.is_empty() => remove(previous),
        Some(previous) => delete_at(previous.value.end - 1),
        None => delete_at(line.name.end - 1),
    }
}

/// Delete, mirroring [`backspace`] forwards: in an empty value the pill
/// goes; at a value's end the next char outside goes, or the next pill's
/// first (or the pill, if empty); before a pill, its first char goes (or
/// the pill, if empty). At the very end nothing goes.
pub fn delete(line: &Line, text: &str, caret: usize) -> Option<Change> {
    let chars: Vec<char> = text.chars().collect();
    let into = |pill: &Pill| if pill.value.is_empty() { remove(pill) } else { delete_at(pill.value.start) };
    for (i, pill) in line.pills.iter().enumerate() {
        if pill.value.is_empty() && caret == pill.value.start {
            return Some(remove(pill));
        }
        if caret == pill.value.end {
            let after = pill.end + 1;
            return Some(match line.pills.get(i + 1) {
                Some(next) if next.key.start == after => into(next),
                _ if after < chars.len() => delete_at(after),
                _ => Change { edits: Vec::new(), caret: Some(Caret::At(caret)) },
            });
        }
        // Before the key, or before the space that keeps outside text apart from it.
        let first_text_end = text_before(line, &chars, i)
            .map_or(if i == 0 { line.name.end } else { line.pills[i - 1].end + 1 }, |last| last + 1);
        if caret == pill.key.start || caret == first_text_end {
            return Some(into(pill));
        }
    }
    None
}

/// The hint after the line (`e6` in 84046): how many options aren't in the
/// box yet.
pub fn placeholder(command: &Command, line: &Line) -> Option<String> {
    let left = command.options.iter().filter(|o| !line.pills.iter().any(|p| p.option == o.name)).count();
    match (left, line.pills.len()) {
        (0, _) => None,
        (n, d) if d > 0 => Some(format!("+{n} more")),
        (1, _) => Some("+1 option".to_owned()),
        (n, _) => Some(format!("+{n} options")),
    }
}

/// The options list (`K` in 496533): the options without a pill whose
/// name starts with `word` lowercased, required ones and the others.
pub fn remaining<'c>(
    command: &'c Command,
    line: &Line,
    word: &str,
) -> (Vec<&'c CommandOption>, Vec<&'c CommandOption>) {
    let word = word.to_lowercase();
    command
        .options
        .iter()
        .filter(|o| !line.pills.iter().any(|p| p.option == o.name))
        .filter(|o| o.display_name.starts_with(&word))
        .partition(|o| o.required)
}

/// A list's header: its words, then what was searched for, which the
/// client shows in bold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The client's words, or a section's name (an app's).
    pub words: std::borrow::Cow<'static, str>,
    pub query: Option<String>,
}

/// The headers of the options list (`K`): required options, then the rest.
pub fn options_headers(required: bool, query: &str) -> (Header, Header) {
    let q = (!query.is_empty()).then(|| query.to_owned());
    let words = |plain, matching| if q.is_some() { matching } else { plain };
    let rest = if required { ("Other options", "Options matching") } else { ("Options", "Options matching") };
    (
        Header { words: words("Required options", "Required options matching").into(), query: q.clone() },
        Header { words: words(rest.0, rest.1).into(), query: q.clone() },
    )
}

/// The one header over a pill's mentions (`renderResults` of `e7` in
/// 496533): members, unless none are listed and `@everyone` is (options),
/// or members aren't offered or none are listed and roles are. None when
/// nothing is listed.
pub fn mention_header(
    offered_members: bool,
    members: usize,
    everyone: bool,
    roles: usize,
    query: &str,
) -> Option<Header> {
    if members == 0 && !everyone && roles == 0 {
        return None;
    }
    let (plain, matching) = if members == 0 && everyone {
        ("Options", "Options matching")
    } else if !offered_members || (members == 0 && roles > 0) {
        ("Roles", "Roles matching")
    } else {
        ("Members", "Members matching")
    };
    Some(match query {
        "" => Header { words: plain.into(), query: None },
        q => Header { words: matching.into(), query: Some(format!("@{q}")) },
    })
}

// ---- Option states ----

/// What the client keeps of each option while a command is active
/// (`optionStates` in 861382): whether it has a pill, whether the caret is
/// in it, and how its last check went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    has_value: bool,
    is_active: bool,
    last: Option<Result<(), String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptionStates {
    states: BTreeMap<String, State>,
    active: Option<String>,
}

/// How an option is checked: its value (none without a pill) and whether
/// an empty one passes.
pub type Check<'a> = &'a dyn Fn(&CommandOption, Option<&str>, bool) -> Result<(), String>;

impl OptionStates {
    /// A command became active: no option has a pill or a result.
    pub fn new(command: &Command) -> OptionStates {
        let states = command.options.iter().map(|o| (o.name.clone(), State::default())).collect();
        OptionStates { states, active: None }
    }

    /// The box or its caret changed (`R` in 870748): `all` checks every
    /// option (on activation, with `allow_empty`); otherwise the option the
    /// caret just left is checked, and one that failed and has (or had) a
    /// pill, so its error follows the typing. A result is kept only where
    /// the pill or the caret changed, or a failed one was checked again; an
    /// option without a pill has none.
    pub fn update(
        &mut self,
        command: &Command,
        values: &Values,
        active: Option<&str>,
        all: bool,
        allow_empty: bool,
        check: Check<'_>,
    ) {
        let left = if all { None } else { self.active.clone() };
        for option in &command.options {
            let old = self.states.get(&option.name).cloned().unwrap_or_default();
            let failed = matches!(old.last, Some(Err(_)));
            let has_value = values.contains_key(&option.name);
            let checked = all
                || (left.as_deref() == Some(option.name.as_str()) && left.as_deref() != active)
                || (failed && (has_value || old.has_value));
            let last = if checked {
                Some(check(option, values.get(&option.name).map(String::as_str), allow_empty))
            } else {
                old.last.clone()
            };
            let new = State { has_value, is_active: active == Some(option.name.as_str()), last };
            if old.has_value != new.has_value || old.is_active != new.is_active || (checked && failed) {
                let last = if new.has_value { new.last } else { None };
                self.states.insert(option.name.clone(), State { last, ..new });
            }
        }
        self.active = active.map(str::to_owned);
    }

    /// Enter (`eX` in 84046, `cd` in 317681): every option checked, empty
    /// ones failing; results kept for the options with a pill. The options
    /// that failed: the pills in their order in the box, then the required
    /// options without one.
    pub fn submit(
        &mut self,
        command: &Command,
        line: &Line,
        values: &Values,
        check: Check<'_>,
    ) -> Result<(), Vec<String>> {
        let mut failing: Vec<String> = Vec::new();
        let mut results = BTreeMap::new();
        for option in &command.options {
            let result = check(option, values.get(&option.name).map(String::as_str), false);
            if values.contains_key(&option.name)
                && let Some(state) = self.states.get_mut(&option.name)
            {
                state.last = Some(result.clone());
            }
            results.insert(option.name.clone(), result);
        }
        for pill in &line.pills {
            if matches!(results.get(&pill.option), Some(Err(_))) {
                failing.push(pill.option.clone());
            }
        }
        for option in &command.options {
            if option.required && !values.contains_key(&option.name) && !failing.contains(&option.name) {
                failing.push(option.name.clone());
            }
        }
        if failing.is_empty() { Ok(()) } else { Err(failing) }
    }

    /// Whether the option's pill is drawn as wrong: its last check didn't
    /// pass, or it has none (`success !== true`).
    pub fn errored(&self, option: &str) -> bool {
        !matches!(self.states.get(option).and_then(|s| s.last.as_ref()), Some(Ok(())))
    }

    /// What its last check said was wrong.
    pub fn error(&self, option: &str) -> Option<&str> {
        match self.states.get(option)?.last.as_ref()? {
            Err(why) => Some(why),
            Ok(()) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn command(name: &str, options: Value) -> Command {
        Command::of_json(&json!({"id": "910", "application_id": "900", "type": 1, "name": name,
                                 "description": "", "options": options}))
    }

    /// count INTEGER required, size STRING with choices, who USER.
    fn roll() -> Command {
        command(
            "roll",
            json!([
                {"type": 4, "name": "count", "description": "How many", "required": true},
                {"type": 3, "name": "size", "description": "Which", "choices": [{"name": "D6", "value": "6"}]},
                {"type": 6, "name": "who", "description": "Whose"},
            ]),
        )
    }

    fn echo() -> Command {
        command("echo", json!([{"type": 3, "name": "message", "description": ""}]))
    }

    /// The box as the UI holds it: its text and each char's tag.
    #[derive(Clone, Debug)]
    struct Box {
        chars: Vec<char>,
        parts: Vec<Part>,
        caret: usize,
    }

    impl Box {
        fn typed(text: &str) -> Box {
            Box {
                chars: text.chars().collect(),
                parts: vec![Part::Plain; text.chars().count()],
                caret: text.chars().count(),
            }
        }

        fn text(&self) -> String {
            self.chars.iter().collect()
        }

        fn apply(&mut self, change: &Change) {
            let before = self.text();
            for edit in &change.edits {
                match edit {
                    Edit::Delete(range) => {
                        self.chars.drain(range.clone());
                        self.parts.drain(range.clone());
                    }
                    Edit::Insert { at, text, part } => {
                        self.chars.splice(*at..*at, text.chars());
                        self.parts.splice(*at..*at, std::iter::repeat_n(*part, text.chars().count()));
                    }
                }
            }
            assert_eq!(change.apply(&before), self.text());
            self.caret = match &change.caret {
                Some(Caret::At(at)) => *at,
                Some(Caret::Select(range)) => range.end,
                None => self.caret.min(self.chars.len()),
            };
        }

        fn marked(&self) -> (String, Vec<Range<usize>>, Vec<usize>) {
            let mut keys: Vec<Range<usize>> = Vec::new();
            for (i, part) in self.parts.iter().enumerate() {
                if *part == Part::Key {
                    match keys.last_mut() {
                        Some(run) if run.end == i => run.end = i + 1,
                        _ => keys.push(i..i + 1),
                    }
                }
            }
            let ends = self.parts.iter().enumerate().filter(|(_, p)| **p == Part::End).map(|(i, _)| i).collect();
            (self.text(), keys, ends)
        }

        fn line(&self, command: &Command) -> Line {
            let (text, keys, ends) = self.marked();
            read(command, &Marked { text: &text, keys, ends })
        }

        /// Text typed at the caret, as the box takes it: plain, never a key's.
        fn type_text(&mut self, text: &str) {
            let n = text.chars().count();
            self.chars.splice(self.caret..self.caret, text.chars());
            self.parts.splice(self.caret..self.caret, std::iter::repeat_n(Part::Plain, n));
            self.caret += n;
        }

        /// What the UI does after a change: repair, then typed keys or the
        /// one-option rule.
        fn sync(&mut self, command: &Command) {
            let (text, keys, ends) = self.marked();
            let repaired = repair(command, &Marked { text: &text, keys, ends }, self.caret);
            if !repaired.is_empty() {
                self.apply(&repaired);
            }
            let (text, keys, ends) = self.marked();
            let marked = Marked { text: &text, keys, ends };
            match typed_keys(command, &marked, self.caret) {
                Some(change) => self.apply(&change),
                None => {
                    let line = read(command, &marked);
                    if let Some(change) = single_option(command, &line, &text, self.caret) {
                        self.apply(&change);
                    }
                }
            }
        }
    }

    fn activated(command: &Command, typed: &str) -> (Box, Change) {
        let mut b = Box::typed(typed);
        let change = activate(command, typed, &Values::new());
        b.apply(&change);
        (b, change)
    }

    #[test]
    fn a_command_starts_with_its_required_options() {
        let roll = roll();
        for typed in ["/ro", "/roll "] {
            let (b, change) = activated(&roll, typed);
            assert_eq!(b.text(), "/roll count: ", "{typed}");
            assert_eq!(change.caret, Some(Caret::Select(12..12)), "in count");
            assert_eq!(b.parts[..6], [Part::Name; 6]);
            assert_eq!(b.parts[6..12], [Part::Key; 6]);
            assert_eq!(b.parts[12], Part::End);
        }
        // A key pasted first, the missing required one after it, the caret in that.
        let (b, change) = activated(&roll, "/roll size:D6");
        assert_eq!(b.text(), "/roll size:D6 count: ");
        assert_eq!(change.caret, Some(Caret::Select(20..20)));
        // What follows the name goes to the first missing option.
        let (b, change) = activated(&roll, "/roll c");
        assert_eq!(b.text(), "/roll count:c ");
        assert_eq!(change.caret, Some(Caret::At(13)), "the end of count");
        // The untranslated name works too.
        let mut localized = roll.clone();
        localized.display_name = "würfeln".into();
        let (b, _) = activated(&localized, "/roll 4");
        assert_eq!(b.text(), "/würfeln count:4 ");
    }

    #[test]
    fn what_follows_never_goes_to_a_file() {
        let upload = command(
            "upload",
            json!([{"type": 11, "name": "file", "description": "", "required": true},
                   {"type": 3, "name": "note", "description": "", "required": true}]),
        );
        let (b, change) = activated(&upload, "/upload hi");
        assert_eq!(b.text(), "/upload file: note:hi ");
        assert_eq!(change.caret, Some(Caret::Select(13..13)), "the empty file pill");
    }

    #[test]
    fn a_lone_optional_option_takes_what_is_typed() {
        let echo = echo();
        let (mut b, change) = activated(&echo, "/echo ");
        assert_eq!(b.text(), "/echo ");
        assert_eq!(change.caret, Some(Caret::At(6)));
        b.type_text("h");
        b.sync(&echo);
        assert_eq!(b.text(), "/echo message:h ");
        assert_eq!(b.caret, 15, "at the end of the value");
        b.type_text("i");
        b.sync(&echo);
        assert_eq!(b.text(), "/echo message:hi ");
        // Picked with text after it, at once.
        let (b, change) = activated(&echo, "/echo hello");
        assert_eq!(b.text(), "/echo message:hello ");
        assert_eq!(change.caret, Some(Caret::At(19)));
    }

    #[test]
    fn keys_typed_outside_the_pills_become_pills() {
        let roll = roll();
        let (mut b, _) = activated(&roll, "/roll 3");
        assert_eq!(b.text(), "/roll count:3 ");
        b.caret = b.chars.len();
        b.type_text("size:D6  who:");
        b.sync(&roll);
        assert_eq!(b.text(), "/roll count:3 size:D6 who: ");
        assert_eq!(b.caret, b.text().chars().count() - 1, "in the last new pill");
        let line = b.line(&roll);
        assert_eq!(
            values(&roll, &line, &b.text(), &Values::new()).into_iter().collect::<Vec<_>>(),
            [
                ("count".to_owned(), "3".to_owned()),
                ("size".to_owned(), "D6".to_owned()),
                ("who".to_owned(), String::new()),
            ]
        );
        // Not at a word's start, not an option's name, or already given: text.
        for typed in ["xsize:", "@size:", "Size:", "count:"] {
            let (mut b, _) = activated(&roll, "/roll 3");
            b.type_text(typed);
            b.sync(&roll);
            assert_eq!(b.line(&roll).pills.len(), 1, "{typed}");
        }
    }

    #[test]
    fn a_key_is_any_letters_of_any_script_and_dashes() {
        let names = command(
            "names",
            json!([{"type": 3, "name": "a", "name_localized": "नाम", "description": ""},
                   {"type": 3, "name": "b", "name_localized": "ชื่อ", "description": ""},
                   {"type": 3, "name": "c", "name_localized": "x_y-z", "description": ""}]),
        );
        let (b, _) = activated(&names, "/names नाम:1 ชื่อ:2 x_y-z: 3 ");
        let line = b.line(&names);
        assert_eq!(line.pills.iter().map(|p| p.option.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(values(&names, &line, &b.text(), &Values::new())["c"], "3", "trimmed");
    }

    #[test]
    fn a_name_broken_ends_the_command() {
        let roll = roll();
        let (b, _) = activated(&roll, "/roll 3");
        let line = b.line(&roll);
        assert!(still_command(&roll, &line, &b.text()));
        let broken = b.text().replacen("/roll", "/rol", 1);
        assert!(!still_command(&roll, &b.line(&roll), &broken));
        assert_eq!(plain(&b.text()), "/roll count:3");
        let none = Line { name: 0..5, pills: Vec::new(), outside: std::iter::once(5..5).collect() };
        assert!(!still_command(&roll, &none, "/roll"), "no pills, no space");
        assert!(still_command(&roll, &none, "/roll "));
    }

    #[test]
    fn the_caret_finds_its_pill_and_moves_between_them() {
        let roll = roll();
        let (b, _) = activated(&roll, "/roll size:D6 who:x count:3 ");
        let text = b.text();
        assert_eq!(text, "/roll size:D6 who:x count:3 ");
        let line = b.line(&roll);
        assert_eq!(active(&line, 11).map(|p| p.option.as_str()), Some("size"), "at the value's start");
        assert_eq!(active(&line, 13).map(|p| p.option.as_str()), Some("size"), "at its end");
        assert_eq!(active(&line, 6), None, "before the key");
        assert_eq!(next(&line, &text, 11), Caret::Select(18..19), "who");
        assert_eq!(next(&line, &text, 6), Caret::Select(11..13), "size, from before it");
        assert_eq!(next(&line, &text, 26), Caret::At(28), "past the last: the end");
        assert_eq!(previous(&line, &text, 18), Some(Caret::Select(11..13)));
        assert_eq!(previous(&line, &text, 11), Some(Caret::At(5)), "before the first: the name's end");
        assert_eq!(previous(&line, &text, 5), None, "already there");
    }

    #[test]
    fn values_are_as_written() {
        let roll = roll();
        let (mut b, _) = activated(&roll, "/roll ");
        b.type_text("3 ");
        let line = b.line(&roll);
        assert_eq!(
            values(&roll, &line, &b.text(), &Values::new())["count"],
            "3 ",
            "an INTEGER's spaces stay for its query"
        );
    }

    #[test]
    fn a_selection_takes_the_pills_it_covers_whole() {
        let roll = roll();
        let (b, _) = activated(&roll, "/roll size:D6 who:x count:3 ");
        let line = b.line(&roll);
        assert!(covered(&line, 11..12).is_empty(), "within one value");
        assert_eq!(covered(&line, 11..19), ["size", "who"], "from a value's start to one's end");
        assert_eq!(covered(&line, 11..14), ["size"], "a value's start past its end");
        assert_eq!(covered(&line, 9..17), [""; 0], "into a key, out at another's");
        assert_eq!(covered(&line, 12..26), ["who"], "ending inside count keeps it");
        assert_eq!(covered(&line, 0..28), ["size", "who", "count"], "everything");
    }

    #[test]
    fn backspace_and_delete_at_a_pill() {
        let roll = roll();
        let fresh = |typed: &str| activated(&roll, typed).0;
        let run = |b: &Box, change: Option<Change>| {
            let mut b = b.clone();
            b.apply(&change.expect("handled"));
            (b.text(), b.caret)
        };
        // /roll size:D6 who: count:3
        let b = fresh("/roll size:D6 who: count:3 ");
        let (line, text) = (b.line(&roll), b.text());
        assert_eq!(text, "/roll size:D6 who: count:3 ");
        assert_eq!(run(&b, backspace(&line, &text, 18)), ("/roll size:D6 count:3 ".into(), 14), "an empty pill goes");
        assert_eq!(
            run(&b, backspace(&line, &text, 14)),
            ("/roll size:D who: count:3 ".into(), 12),
            "after a pill: its last char"
        );
        assert_eq!(
            run(&b, backspace(&line, &text, 19)),
            ("/roll size:D6 count:3 ".into(), 14),
            "after an empty one: it"
        );
        assert_eq!(
            run(&b, backspace(&line, &text, 25)),
            ("/roll size:D6 count:3 ".into(), 14),
            "before count: the empty pill"
        );
        assert_eq!(
            run(&b, backspace(&line, &text, 11)),
            ("/rol size:D6 who: count:3 ".into(), 4),
            "the name's last char"
        );
        assert_eq!(
            run(&b, backspace(&line, &text, 6)),
            ("/rol size:D6 who: count:3 ".into(), 4),
            "before the first key"
        );
        assert!(backspace(&line, &text, 12).is_none(), "inside a value");
        assert_eq!(run(&b, delete(&line, &text, 18)), ("/roll size:D6 count:3 ".into(), 14), "an empty pill goes");
        assert_eq!(run(&b, delete(&line, &text, 13)), ("/roll size:D6 count:3 ".into(), 14), "the next, empty");
        assert_eq!(
            run(&b, delete(&line, &text, 5)),
            ("/roll size:6 who: count:3 ".into(), 11),
            "from the name: size's first"
        );
        assert_eq!(run(&b, delete(&line, &text, 26)), (text.clone(), 26), "at the very end, nothing");
        assert!(delete(&line, &text, 12).is_none());
        // The last pill gone, the command stays: `/roll ` keeps its space.
        let b = fresh("/roll ");
        let line = b.line(&roll);
        let (left, _) = run(&b, backspace(&line, &b.text(), 12));
        assert_eq!(left, "/roll ");
        assert!(still_command(&roll, &read(&roll, &Marked { text: &left, keys: vec![], ends: vec![] }), &left));
        // Outside text before a pill loses its last char, not the space that keeps it apart.
        let mut b = fresh("/roll size:D6 count:3 ");
        b.caret = 14;
        b.type_text("ab ");
        let (line, text) = (b.line(&roll), b.text());
        assert_eq!(text, "/roll size:D6 ab count:3 ");
        assert_eq!(run(&b, backspace(&line, &text, 23)), ("/roll size:D6 a count:3 ".into(), 15));
        assert_eq!(
            run(&b, delete(&line, &text, 16)),
            ("/roll size:D6 ab count: ".into(), 23),
            "from the text: count's first"
        );
        assert_eq!(
            run(&b, backspace(&line, &text, 17)),
            ("/roll size:D6 a count:3 ".into(), 15),
            "between it and the key"
        );
    }

    #[test]
    fn an_option_picked_leaves_the_text_around_it() {
        let roll = roll();
        let (mut b, _) = activated(&roll, "/roll size:D6 count:3 ");
        b.caret = 14;
        b.type_text("ab ");
        b.caret = b.chars.len();
        b.type_text("w");
        let (text, keys, ends) = b.marked();
        assert_eq!(text, "/roll size:D6 ab count:3 w");
        let change = add(&roll, &Marked { text: &text, keys, ends }, "who", 25..26);
        b.apply(&change);
        assert_eq!(b.text(), "/roll size:D6 ab count:3 who: ");
        assert_eq!(change.edits.len(), 3, "only the word is replaced: {:?}", change.edits);
    }

    #[test]
    fn what_a_range_deletion_or_a_paste_left_is_set_right() {
        let roll = roll();
        let (mut b, _) = activated(&roll, "/roll size:D6 count:3 ");
        // The end of size deleted along with part of what follows.
        b.chars.remove(13);
        b.parts.remove(13);
        b.caret = 13;
        b.sync(&roll);
        assert_eq!(b.text(), "/roll size:D6 count:3 ");
        assert_eq!(b.parts[13], Part::End);
        // The name's space.
        b.chars.remove(5);
        b.parts.remove(5);
        b.caret = 5;
        b.sync(&roll);
        assert_eq!(b.text(), "/roll size:D6 count:3 ");
        // A line break outside a STRING value.
        let at = b.text().chars().count() - 1;
        b.caret = at;
        b.type_text("\n");
        b.sync(&roll);
        assert_eq!(b.text(), "/roll size:D6 count:3  ");
        // A key whose option isn't the command's any more is text.
        let (text, mut keys, ends) = b.marked();
        keys.push(0..1);
        assert!(repair(&roll, &Marked { text: &text, keys, ends }, 0).is_empty(), "a key not an option's is left");
    }

    #[test]
    fn picking_an_option_or_a_value() {
        let roll = roll();
        let (mut b, _) = activated(&roll, "/roll 3");
        b.caret = b.chars.len();
        b.type_text("si");
        let (text, keys, ends) = b.marked();
        let change = add(&roll, &Marked { text: &text, keys, ends }, "size", 14..16);
        b.apply(&change);
        assert_eq!(b.text(), "/roll count:3 size: ");
        assert_eq!(change.caret, Some(Caret::At(19)), "in its value");
        let line = b.line(&roll);
        b.apply(&set_value(&line, "size", "D6"));
        assert_eq!(b.text(), "/roll count:3 size:D6 ");
        let line = b.line(&roll);
        b.apply(&set_value(&line, "count", "<@1>"));
        assert_eq!(b.text(), "/roll count:<@1> size:D6 ");
        // Sending with the required ones missing adds them, empty, at the end.
        let need = command(
            "need",
            json!([{"type": 3, "name": "a", "description": "", "required": true},
                   {"type": 3, "name": "b", "description": ""},
                   {"type": 3, "name": "c", "description": "", "required": true}]),
        );
        let (b, _) = activated(&need, "/need b:x");
        assert_eq!(b.text(), "/need b:x a: c: ");
        // With a and c taken out, sending puts them back.
        let (text, mut keys, mut ends) = b.marked();
        keys.truncate(1);
        ends.truncate(1);
        let short: String = text.chars().take(10).collect();
        let change = add_missing(&need, &Marked { text: &short, keys, ends });
        assert_eq!(change.apply(&short), "/need b:x a: c: ");
    }

    #[test]
    fn the_hint_counts_the_options_left() {
        let roll = roll();
        let (b, _) = activated(&roll, "/roll ");
        assert_eq!(placeholder(&roll, &b.line(&roll)), Some("+2 more".into()));
        let none = Line { name: 0..5, pills: Vec::new(), outside: std::iter::once(5..6).collect() };
        assert_eq!(placeholder(&roll, &none), Some("+3 options".into()));
        assert_eq!(placeholder(&echo(), &none), Some("+1 option".into()));
        let (b, _) = activated(&roll, "/roll count:1 size:D6 who:x");
        assert_eq!(placeholder(&roll, &b.line(&roll)), None);
        let (b, _) = activated(&roll, "/roll ");
        let (required, others) = remaining(&roll, &b.line(&roll), "");
        assert!(required.is_empty());
        assert_eq!(others.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["size", "who"]);
        let (_, others) = remaining(&roll, &b.line(&roll), "W");
        assert_eq!(others.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["who"], "lowercased");
        let (required, _) = remaining(&roll, &none, "c");
        assert_eq!(required.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["count"]);
        assert_eq!(options_headers(true, "").0, Header { words: "Required options".into(), query: None });
        assert_eq!(options_headers(true, "").1.words, "Other options");
        assert_eq!(options_headers(false, "w").1, Header { words: "Options matching".into(), query: Some("w".into()) });
    }

    #[test]
    fn one_header_over_a_pills_mentions() {
        let header = |m, members, everyone, roles, q| {
            mention_header(m, members, everyone, roles, q).map(|h| h.words.into_owned())
        };
        assert_eq!(header(true, 2, true, 1, ""), Some("Members".to_owned()));
        assert_eq!(header(true, 0, true, 1, ""), Some("Options".to_owned()), "only @everyone and roles");
        assert_eq!(header(false, 0, false, 2, ""), Some("Roles".to_owned()));
        assert_eq!(header(true, 0, false, 2, ""), Some("Roles".to_owned()));
        assert_eq!(header(false, 0, true, 2, ""), Some("Options".to_owned()), "a ROLE pill with @everyone listed");
        assert_eq!(header(true, 0, false, 0, ""), None);
        assert_eq!(
            mention_header(true, 1, false, 0, "al"),
            Some(Header { words: "Members matching".into(), query: Some("@al".into()) })
        );
    }

    fn check(option: &CommandOption, content: Option<&str>, allow_empty: bool) -> Result<(), String> {
        crate::commands::validate(option, content, allow_empty, &NoNames, &Default::default())
    }

    struct NoNames;
    impl crate::commands::Names for NoNames {
        fn user(&self, _: &str) -> Option<crate::Snowflake> {
            None
        }
        fn role(&self, _: &str) -> Option<crate::Snowflake> {
            None
        }
        fn channel(&self, _: &str) -> Option<crate::Snowflake> {
            None
        }
        fn guild(&self) -> Option<crate::Snowflake> {
            None
        }
    }

    fn values_of(pairs: &[(&str, &str)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn options_are_checked_as_the_caret_leaves_them() {
        let roll = roll();
        let mut states = OptionStates::new(&roll);
        // Activation checks all, empty allowed.
        states.update(&roll, &values_of(&[("count", "")]), Some("count"), true, true, &check);
        assert!(!states.errored("count"), "empty passes while it starts");
        // Typing a wrong value isn't checked while the caret is in it.
        states.update(&roll, &values_of(&[("count", "x")]), Some("count"), false, false, &check);
        assert!(!states.errored("count"));
        // Leaving it checks it.
        states.update(&roll, &values_of(&[("count", "x")]), None, false, false, &check);
        assert_eq!(states.error("count"), Some("Input a valid integer."));
        // A failing pill is checked on every change, and clears as it is fixed.
        states.update(&roll, &values_of(&[("count", "4")]), None, false, false, &check);
        assert!(!states.errored("count"));
        // A pill typed as `name:` has no result until left.
        states.update(&roll, &values_of(&[("count", "4"), ("who", "")]), Some("who"), false, false, &check);
        assert!(states.errored("who") && states.error("who").is_none());
        states.update(&roll, &values_of(&[("count", "4"), ("who", "")]), None, false, false, &check);
        assert_eq!(states.error("who"), Some("Not a valid user."), "an empty optional pill fails as its type");
        // A pill removed forgets its result.
        states.update(&roll, &values_of(&[("count", "4")]), None, false, false, &check);
        assert!(states.error("who").is_none());
    }

    #[test]
    fn sending_lists_what_fails_in_the_boxs_order() {
        let roll = roll();
        let (b, _) = activated(&roll, "/roll who:x size:D7");
        let line = b.line(&roll);
        let values = values(&roll, &line, &b.text(), &Values::new());
        let mut states = OptionStates::new(&roll);
        let failed = states.submit(&roll, &line, &values, &check).unwrap_err();
        // Pills in the box's order (who, size, count), count empty as required.
        assert_eq!(failed, ["who", "size", "count"]);
        assert_eq!(states.error("size"), Some("Not a valid choice."));
        let line = Line { pills: line.pills[..2].to_vec(), ..line };
        let values: Values = values.into_iter().filter(|(k, _)| k != "count").collect();
        let mut states = OptionStates::new(&roll);
        assert_eq!(states.submit(&roll, &line, &values, &check).unwrap_err(), ["who", "size", "count"]);
        assert!(states.error("count").is_none(), "no result kept without a pill");
        let ok = values_of(&[("count", "2")]);
        assert_eq!(OptionStates::new(&roll).submit(&roll, &line, &ok, &check), Ok(()));
    }

    #[test]
    fn text_typed_in_a_file_pill_is_no_file_to_send() {
        let upload = command(
            "upload",
            json!([{"type": 11, "name": "file", "description": "", "required": true},
                   {"type": 3, "name": "note", "description": ""}]),
        );
        let (b, _) = activated(&upload, "/upload file:cat.png note:hi");
        let line = b.line(&upload);
        assert_eq!(line.pills.len(), 2, "the key is still a pill");
        let values = values(&upload, &line, &b.text(), &Values::new());
        let mut states = OptionStates::new(&upload);
        assert_eq!(states.submit(&upload, &line, &values, &check).unwrap_err(), ["file"], "required, no file");
        let note = values_of(&[("note", "hi")]);
        let sent =
            crate::commands::run_options(&upload, &values, &NoNames, &Default::default(), None).unwrap_or_default();
        assert_eq!(
            sent,
            crate::commands::run_options(&upload, &note, &NoNames, &Default::default(), None).unwrap(),
            "no type 11 pointing at a file the run doesn't carry"
        );
    }

    fn upload() -> Command {
        command(
            "upload",
            json!([{"type": 11, "name": "file", "description": "", "required": true},
                   {"type": 11, "name": "picture", "description": ""},
                   {"type": 3, "name": "note", "description": ""}]),
        )
    }

    #[test]
    fn a_file_pill_holds_its_staged_file_and_nothing_typed() {
        let upload = upload();
        let (mut b, _) = activated(&upload, "/upload file:typed.png note:hi");
        let staged = values_of(&[("file", "cat.png"), ("picture", "dog.png")]);
        let (text, keys, ends) = b.marked();
        let change = file_texts(&upload, &Marked { text: &text, keys, ends }, &staged, 0);
        b.apply(&change);
        assert_eq!(b.text(), "/upload file:cat.png note:hi ", "the typed text goes, the file's name shows");
        let line = b.line(&upload);
        let given = values(&upload, &line, &b.text(), &staged);
        assert_eq!(given.get("file").map(String::as_str), Some("cat.png"));
        assert!(!given.contains_key("picture"), "a file without a pill isn't sent");
        // Taken back: the pill stays, empty.
        let (text, keys, ends) = b.marked();
        b.apply(&file_texts(&upload, &Marked { text: &text, keys, ends }, &Values::new(), 0));
        assert_eq!(b.text(), "/upload file: note:hi ");
        assert_eq!(values(&upload, &b.line(&upload), &b.text(), &Values::new())["file"], "");
        let (text, keys, ends) = b.marked();
        assert!(file_texts(&upload, &Marked { text: &text, keys, ends }, &Values::new(), 0).is_empty());
    }

    #[test]
    fn file_options_are_offered_as_any_other() {
        let upload = upload();
        let (b, _) = activated(&upload, "/upload ");
        let line = b.line(&upload);
        assert_eq!(placeholder(&upload, &line), Some("+2 more".into()));
        let (required, others) = remaining(&upload, &line, "");
        assert!(required.is_empty());
        assert_eq!(others.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["picture", "note"]);
        // A required file option left out gets its pill on Enter.
        let mut b = Box::typed("/upload note:hi ");
        b.parts[..8].fill(Part::Name);
        b.parts[8..13].fill(Part::Key);
        b.parts[15] = Part::End;
        let (text, keys, ends) = b.marked();
        b.apply(&add_missing(&upload, &Marked { text: &text, keys, ends }));
        assert_eq!(b.text(), "/upload note:hi file: ");
    }

    #[test]
    fn a_file_pill_holding_a_file_goes_whole() {
        let upload = upload();
        let (mut b, _) = activated(&upload, "/upload note:hi");
        let staged = values_of(&[("file", "cat.png")]);
        let (text, keys, ends) = b.marked();
        b.apply(&file_texts(&upload, &Marked { text: &text, keys, ends }, &staged, 0));
        let text = b.text();
        assert_eq!(text, "/upload note:hi file:cat.png ");
        let line = b.line(&upload);
        let run = |change: Option<Change>| {
            let mut b = b.clone();
            b.apply(&change.expect("handled"));
            (b.text(), b.caret)
        };
        let gone = ("/upload note:hi ".to_owned(), 16);
        // Right after the pill, inside its name, and Delete before its key or inside it.
        for (caret, back) in [(29, true), (28, true), (24, true), (16, false), (22, false)] {
            assert_eq!(run(erase(&upload, &line, &text, caret, back)), gone, "{caret} {back}");
        }
        // At the name's start, Backspace takes what is before the pill, as for any pill.
        assert_eq!(run(erase(&upload, &line, &text, 21, true)).0, "/upload note:h file:cat.png ");
        // Elsewhere, as ever.
        assert!(erase(&upload, &line, &text, 15, true).is_none(), "inside another value, the box deletes");
    }

    #[test]
    fn a_pasted_file_goes_to_the_file_option_in_hand_or_the_first_empty() {
        let upload = upload();
        let name = |o: Option<&CommandOption>| o.map(|o| o.name.clone());
        let none = Values::new();
        assert_eq!(name(paste_option(&upload, Some("picture"), &none)), Some("picture".into()));
        assert_eq!(name(paste_option(&upload, None, &none)), Some("file".into()));
        assert_eq!(name(paste_option(&upload, Some("note"), &none)), Some("file".into()), "not a file option");
        let filled = values_of(&[("file", "cat.png")]);
        assert_eq!(name(paste_option(&upload, None, &filled)), Some("picture".into()), "with a pill or not");
        assert_eq!(name(paste_option(&upload, Some("file"), &filled)), Some("file".into()), "replaces in hand");
        let full = values_of(&[("file", "cat.png"), ("picture", "dog.png")]);
        assert_eq!(name(paste_option(&upload, None, &full)), None);
    }
}
