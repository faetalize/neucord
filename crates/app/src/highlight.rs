//! Syntax highlighting for code blocks, by GtkSourceView's language
//! definitions in its Adwaita style schemes, turned into Pango markup for a
//! label: a source view is a text view, which can't size a list's rows.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use sourceview::prelude::*;

/// Highlighted blocks kept, as rows are rebuilt while scrolling.
const CACHED: usize = 256;
/// Lines longer than this aren't highlighted (the official client's limit).
const LONGEST_LINE: usize = 1000;

thread_local! {
    static CACHE: RefCell<HashMap<(String, String, bool), Option<String>>> = RefCell::default();
}

/// `code` in `lang` as Pango markup, for a light or dark background. `None`
/// for a language we don't know, or code too long to highlight.
pub fn markup(lang: &str, code: &str, dark: bool) -> Option<String> {
    if code.lines().any(|line| line.len() > LONGEST_LINE) {
        return None;
    }
    let key = (lang.to_owned(), code.to_owned(), dark);
    if let Some(cached) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return cached;
    }
    let markup = highlight(lang, code, dark);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHED {
            cache.clear();
        }
        cache.insert(key, markup.clone());
    });
    markup
}

fn language(lang: &str) -> Option<sourceview::Language> {
    let manager = sourceview::LanguageManager::default();
    let name = discord::markup::code_language(lang);
    // Where GtkSourceView names a language differently.
    let id = match name.as_str() {
        "javascript" => "js",
        "bash" | "zsh" => "sh",
        "python" => "python3",
        "dockerfile" => "docker",
        "x86asm" | "asm" => "nasm",
        "batch" => "dosbatch",
        "elisp" => "scheme",
        other => other,
    };
    manager
        .language(id)
        // Else the tag as a file extension: `.rs`, `.kt`, ….
        .or_else(|| manager.guess_language(Some(&format!("code.{lang}")), None))
}

fn highlight(lang: &str, code: &str, dark: bool) -> Option<String> {
    let language = language(lang)?;
    let scheme = sourceview::StyleSchemeManager::default().scheme(if dark { "Adwaita-dark" } else { "Adwaita" })?;
    let buffer = sourceview::Buffer::with_language(&language);
    buffer.set_style_scheme(Some(&scheme));
    buffer.set_text(code);
    let (start, end) = buffer.bounds();
    buffer.ensure_highlight(&start, &end);

    let mut out = String::new();
    let mut at = start;
    while at < end {
        let mut next = at;
        if !next.forward_to_tag_toggle(None::<&gtk::TextTag>) || next > end {
            next = end;
        }
        let text = buffer.text(&at, &next, true);
        // Tags come in rising priority: the last to set a property wins.
        let look = at.tags().iter().fold(Look::default(), |look, tag| look.with(tag));
        let attributes = look.attributes();
        if attributes.is_empty() {
            out.push_str(&glib::markup_escape_text(&text));
        } else {
            out.push_str(&format!("<span{attributes}>{}</span>", glib::markup_escape_text(&text)));
        }
        at = next;
    }
    Some(out)
}

/// What the tags on a stretch of code make it look like.
#[derive(Default)]
struct Look {
    foreground: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strikethrough: Option<bool>,
}

impl Look {
    fn with(mut self, tag: &gtk::TextTag) -> Self {
        if tag.is_foreground_set()
            && let Some(colour) = tag.foreground_rgba()
        {
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            self.foreground =
                Some(format!("#{:02x}{:02x}{:02x}", byte(colour.red()), byte(colour.green()), byte(colour.blue())));
        }
        if tag.is_weight_set() {
            self.bold = Some(tag.weight() >= 600);
        }
        if tag.is_style_set() {
            self.italic = Some(tag.style() == gtk::pango::Style::Italic);
        }
        if tag.is_underline_set() {
            self.underline = Some(tag.underline() != gtk::pango::Underline::None);
        }
        if tag.is_strikethrough_set() {
            self.strikethrough = Some(tag.is_strikethrough());
        }
        self
    }

    /// As Pango span attributes, each with a leading space.
    fn attributes(&self) -> String {
        let mut out = String::new();
        if let Some(colour) = &self.foreground {
            out += &format!(" foreground=\"{colour}\"");
        }
        if self.bold == Some(true) {
            out += " weight=\"bold\"";
        }
        if self.italic == Some(true) {
            out += " style=\"italic\"";
        }
        if self.underline == Some(true) {
            out += " underline=\"single\"";
        }
        if self.strikethrough == Some(true) {
            out += " strikethrough=\"true\"";
        }
        out
    }
}
