//! An app's form (a modal), answered with MODAL_SUBMIT: what
//! `INTERACTION_MODAL_CREATE` carries (225123) and which components the
//! client keeps of it (`ZV` in 257249), how each field is checked (`N` in
//! 911408), what a submit sends (`uD` in 969508), how it waits for its
//! answer (750128 `InteractionModalStore`), and what is kept per modal
//! `custom_id` until a submit succeeds (935548
//! `LocalInteractionComponentStateStore`).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::component::{Component, SelectOption};
use crate::interaction::pick_error;
use crate::limits::{Uploads, message_length};
use crate::members::Known;
use crate::model::User;
use crate::uploads;
use crate::{Snowflake, cdn};

/// `MODAL_SUBMIT` in 127258's interaction types.
pub const MODAL_SUBMIT: u8 = 5;
/// How long a submit waits for Discord's answer before it fails (750128).
/// With uploads it is 2 minutes from the submit and 10 s after they end.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a submit with uploads waits from the submit (750128), as well
/// as [`SUBMIT_TIMEOUT`] after its uploads end.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// What the form says when a submit failed, whatever the reason (`uJgdEu`).
pub const SUBMIT_ERROR: &str = "Something went wrong. Try again.";
/// A string select's face with nothing picked and no placeholder (`c7` in 257249).
pub const SELECT_PLACEHOLDER: &str = "Make a selection";

/// The limits the API gives a text input when the app gives none, which the
/// "between" message names in place of the client's formatted `undefined`.
const TEXT_MIN: usize = 0;
const TEXT_MAX: usize = 4000;

/// An app's form, as `INTERACTION_MODAL_CREATE` brings it.
#[derive(Clone, Debug, PartialEq)]
pub struct Modal {
    /// Sent back as `data.id`.
    pub id: Snowflake,
    /// The interaction that asked for it, if one of ours.
    pub nonce: Option<Snowflake>,
    pub channel: Snowflake,
    /// What its typed values are kept by, and sent back as `data.custom_id`.
    pub custom_id: String,
    pub title: String,
    pub application: ModalApp,
    pub components: Vec<Node>,
    /// `resolved.users`, to learn their names.
    pub resolved_users: Vec<User>,
    /// `resolved.members` whose user is in `resolved.users` (696451 drops
    /// the rest).
    pub resolved_members: Vec<(Snowflake, Known)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModalApp {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub bot: Option<User>,
}

/// One component the client keeps, at its index path: `"i"` at the top,
/// `"i,j"` in a row, `"i,0"` for a label's child, counted before dropped
/// ones are left out (`b` in 257249). Field states are kept by it.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: String,
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// In a modal, a legacy row holding one text input.
    Row(Vec<Node>),
    Label {
        label: String,
        description: Option<String>,
        child: Box<Node>,
    },
    TextInput(TextInput),
    StringSelect(StringSelect),
    EntitySelect(EntitySelect),
    /// Markup.
    Text(String),
    FileUpload(FileUpload),
    RadioGroup {
        custom_id: String,
        options: Vec<SelectOption>,
        required: bool,
    },
    CheckboxGroup {
        custom_id: String,
        options: Vec<SelectOption>,
        min: Option<usize>,
        max: Option<usize>,
        required: bool,
    },
    /// Drawn with its label's text.
    Checkbox {
        custom_id: String,
        default: Option<bool>,
    },
    /// A component the client draws but can't submit (a button, section,
    /// gallery, …): its submit throws before anything is sent.
    Unsubmittable(Component),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInput {
    pub custom_id: String,
    /// PARAGRAPH (style 2): several lines.
    pub paragraph: bool,
    /// Its own title, in a legacy row.
    pub label: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub required: bool,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StringSelect {
    pub custom_id: String,
    pub options: Vec<SelectOption>,
    pub placeholder: Option<String>,
    pub min: Option<usize>,
    pub max: Option<usize>,
    pub required: bool,
    pub disabled: bool,
}

/// What an entity select picks from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    User = 5,
    Role = 6,
    Mentionable = 7,
    Channel = 8,
}

/// What a select's `default_values` entry names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultKind {
    User,
    Role,
    Channel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntitySelect {
    pub kind: EntityKind,
    pub custom_id: String,
    pub placeholder: Option<String>,
    pub min: Option<usize>,
    pub max: Option<usize>,
    pub required: bool,
    pub disabled: bool,
    pub defaults: Vec<(DefaultKind, Snowflake)>,
    pub channel_types: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileUpload {
    pub custom_id: String,
    pub min: Option<usize>,
    pub max: Option<usize>,
    pub required: bool,
    pub file_types: Vec<String>,
}

/// A file field at its limit (`eV` in 232430).
pub const FILES_FULL: &str = "File upload limit reached";
pub const FILES_FULL_HINT: &str = "Remove some files to upload new ones.";
/// A file's remove button (`N86XcP`).
pub const REMOVE_FILE: &str = "Remove";

/// Why files weren't added to a file field (`T` in 232430's `eK`), in the
/// order it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRefusal {
    /// More than `max_values` together with those already there.
    TooMany { max: usize },
    /// One over the file limit (518960's `P`).
    TooLarge { max: u64 },
    /// The added ones together over the total.
    TotalTooLarge { max: u64 },
    /// A name the field's `file_types` doesn't take (424170).
    Type { types: String },
}

impl FileRefusal {
    pub fn heading(&self) -> &'static str {
        match self {
            FileRefusal::TooMany { .. } => "Too many uploads!",
            FileRefusal::TooLarge { .. } | FileRefusal::TotalTooLarge { .. } => "Your files are too powerful",
            FileRefusal::Type { .. } => uploads::INVALID_TYPE,
        }
    }

    /// 518960's `P` sizes with `Hb` (humanize's `filesize`).
    pub fn body(&self) -> String {
        match self {
            FileRefusal::TooMany { max: 1 } => "You can upload up to 1 file.".to_owned(),
            FileRefusal::TooMany { max } => format!("You can upload up to {} files.", grouped(*max)),
            FileRefusal::TooLarge { max } => format!("Max file size is {} please.", uploads::filesize(*max)),
            FileRefusal::TotalTooLarge { max } => {
                format!("Total message size must be less than {}", uploads::filesize(*max))
            }
            FileRefusal::Type { types } => uploads::invalid_type_words(types),
        }
    }
}

/// A size as the hint writes it (`Xq` in 255438 over kibibytes, with a
/// space): mebibytes rounded up, or gibibytes rounded.
fn hint_size(bytes: u64) -> String {
    let mib = (bytes as f64 / 1024.0 / 1024.0).ceil();
    if mib < 1024.0 { format!("{} MB", grouped(mib as usize)) } else { format!("{} GB", (mib / 1024.0).round()) }
}

impl FileUpload {
    /// What the empty area says, its last word opening the file picker
    /// (`y68I8P`, by `max_values`, the API's default 1 when missing, as in
    /// [`FileUpload::hint`]).
    pub fn drop_words(&self) -> (&'static str, &'static str) {
        let what = if self.max.unwrap_or(1) == 1 { "Drop file here or " } else { "Drop files here or " };
        (what, "browse")
    }

    /// The hint under it (`y` in 257249), with `max_size` the file limit.
    /// Without a `max_values` the client formats an undefined there; this
    /// writes the API's default, 1.
    pub fn hint(&self, max_size: u64) -> String {
        let (min, max) = (self.min.unwrap_or_default(), self.max.unwrap_or(1));
        let size = hint_size(max_size);
        let types = uploads::types_words(&self.file_types);
        match (types, min > 1) {
            (Some(types), false) if max == 1 => format!("Upload 1 {types} file under {size}."),
            (Some(types), false) => format!("Upload up to {} {types} files under {size}.", grouped(max)),
            (Some(types), true) if min == max => format!("Upload {} {types} files under {size}.", grouped(min)),
            (Some(types), true) => {
                format!("Upload {}\u{2013}{} {types} files under {size}.", grouped(min), grouped(max))
            }
            (None, false) if max == 1 => format!("Upload a file under {size}."),
            (None, false) => format!("Upload up to {} files under {size}.", grouped(max)),
            (None, true) if min == max => format!("Upload {} files under {size}.", grouped(min)),
            (None, true) => format!("Upload {}\u{2013}{} files under {size}.", grouped(min), grouped(max)),
        }
    }

    /// Whether files named and sized `added` may join the `already` there:
    /// the count, then the sizes (382287 `fJ` without deferring: any over
    /// the file limit, or the added ones together over the total), then the
    /// type. No empty-file check: the upload turns that down itself.
    pub fn check(&self, limits: &Uploads, already: usize, added: &[(&str, u64)]) -> Result<(), FileRefusal> {
        if let Some(max) = self.max
            && already + added.len() > max
        {
            return Err(FileRefusal::TooMany { max });
        }
        if added.iter().any(|(_, size)| *size > limits.file_size) {
            return Err(FileRefusal::TooLarge { max: limits.file_size });
        }
        if added.iter().map(|(_, size)| size).sum::<u64>() > limits.total_size {
            return Err(FileRefusal::TotalTooLarge { max: limits.total_size });
        }
        let extensions = uploads::extensions(&self.file_types);
        if !added.iter().all(|(name, _)| uploads::takes(&extensions, name)) {
            let types = uploads::types_words(&self.file_types).unwrap_or_default();
            return Err(FileRefusal::Type { types });
        }
        Ok(())
    }

    /// The picker's single or multiple choice (`multiple: max_values > 1`).
    pub fn picks_several(&self) -> bool {
        self.max.is_some_and(|max| max > 1)
    }

    /// Whether the field has all it may take (`eV` shows instead of the area).
    pub fn full(&self, count: usize) -> bool {
        self.max.is_some_and(|max| count >= max)
    }
}

fn id_of(value: &Value) -> Option<Snowflake> {
    value.as_str()?.parse().ok().map(Snowflake)
}

impl Modal {
    /// From the event's payload; `None` without the ids a submit needs.
    pub fn from_event(data: &Value) -> Option<Modal> {
        let app = &data["application"];
        let resolved_users: Vec<User> = data["resolved"]["users"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, user)| User::deserialize(user).ok())
            .collect();
        let resolved_members = data["resolved"]["members"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, member)| Some((Snowflake(id.parse().ok()?), Known::from_value(member))))
            .filter(|(id, _)| resolved_users.iter().any(|u| u.id == *id))
            .collect();
        Some(Modal {
            id: id_of(&data["id"])?,
            nonce: id_of(&data["nonce"]),
            channel: id_of(&data["channel_id"])?,
            custom_id: data["custom_id"].as_str()?.to_owned(),
            title: data["title"].as_str().unwrap_or_default().to_owned(),
            application: ModalApp {
                id: id_of(&app["id"])?,
                name: app["name"].as_str().unwrap_or_default().to_owned(),
                icon: app["icon"].as_str().map(str::to_owned),
                bot: User::deserialize(&app["bot"]).ok(),
            },
            components: nodes(&data["components"], &[]),
            resolved_users,
            resolved_members,
        })
    }

    /// The fields, in the order they show: rows and labels opened.
    pub fn fields(&self) -> Vec<&Node> {
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
            for node in nodes {
                match &node.kind {
                    NodeKind::Row(children) => walk(children, out),
                    NodeKind::Label { child, .. } => walk(std::slice::from_ref(child), out),
                    _ if node.is_field() => out.push(node),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.components, &mut out);
        out
    }

    /// The text input that takes the focus when the form opens: the first
    /// component's, if that is a row or label around one (`FG` in 969508).
    pub fn autofocus(&self) -> Option<&str> {
        let first = self.components.first()?;
        let child = match &first.kind {
            NodeKind::Row(children) => children.first()?,
            NodeKind::Label { child, .. } => child,
            _ => return None,
        };
        matches!(child.kind, NodeKind::TextInput(_)).then_some(child.path.as_str())
    }

    /// Whether Enter in a text input submits. The client's form has no submit
    /// button, so only HTML's implicit submission does it: with exactly one
    /// single-line input, where an entity select's search box counts as one.
    pub fn enter_submits(&self) -> bool {
        let inputs = self
            .fields()
            .iter()
            .filter(|node| match &node.kind {
                NodeKind::TextInput(input) => !input.paragraph,
                NodeKind::EntitySelect(_) => true,
                _ => false,
            })
            .count();
        inputs == 1
    }

    /// The app's name as the form says it (`P` in 969508): the bot's
    /// nickname in the selected server, else the bot's name, else the app's.
    pub fn app_name<'a>(&'a self, nick: Option<&'a str>) -> &'a str {
        nick.or(self.application.bot.as_ref().map(|bot| bot.username.as_str())).unwrap_or(&self.application.name)
    }

    /// The header's picture (`P` in 969508): the chain of
    /// [`cdn::application_icon`] with the bot first, where the bot is
    /// passed only if it is a member of the selected server (`member`, with
    /// its picture there if it has one). `None` is the symbolic fallback.
    pub fn app_icon(&self, member: Option<(Snowflake, Option<&str>)>, size: u16) -> Option<String> {
        let app = &self.application;
        let bot = member.and(app.bot.as_ref()).map(|bot| cdn::IconBot {
            id: bot.id.0,
            avatar: bot.avatar.as_deref(),
            discriminator: bot.discriminator.as_deref().unwrap_or_default(),
            bot: bot.bot,
        });
        let in_server = member.and_then(|(guild, avatar)| Some((guild.0, avatar?)));
        cdn::application_icon(app.id.0, app.icon.as_deref(), bot, in_server, true, size)
    }
}

/// The components the client keeps (`ZV` in 257249), each at its path.
/// Any other type is dropped with its index still counted, and so is a
/// label whose child was dropped and a section left empty.
fn nodes(list: &Value, parent: &[usize]) -> Vec<Node> {
    list.as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, value)| {
            let path: Vec<usize> = parent.iter().copied().chain([i]).collect();
            node(value, &path)
        })
        .collect()
}

fn node(value: &Value, path: &[usize]) -> Option<Node> {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    let custom_id = || text("custom_id").unwrap_or_default();
    let count = |key: &str| value[key].as_u64().map(|n| n as usize);
    let required = value["required"].as_bool().unwrap_or(false);
    let disabled = value["disabled"].as_bool().unwrap_or(false);
    let options = || -> Vec<SelectOption> {
        value["options"].as_array().into_iter().flatten().filter_map(|o| SelectOption::deserialize(o).ok()).collect()
    };
    let kind = match value["type"].as_u64()? {
        1 => NodeKind::Row(nodes(&value["components"], path)),
        3 => NodeKind::StringSelect(StringSelect {
            custom_id: custom_id(),
            options: options(),
            placeholder: text("placeholder"),
            min: count("min_values"),
            max: count("max_values"),
            required,
            disabled,
        }),
        4 => NodeKind::TextInput(TextInput {
            custom_id: custom_id(),
            paragraph: value["style"].as_u64() == Some(2),
            label: text("label"),
            value: text("value"),
            placeholder: text("placeholder"),
            required,
            min_length: count("min_length"),
            max_length: count("max_length"),
        }),
        kind @ 5..=8 => NodeKind::EntitySelect(EntitySelect {
            kind: match kind {
                5 => EntityKind::User,
                6 => EntityKind::Role,
                7 => EntityKind::Mentionable,
                _ => EntityKind::Channel,
            },
            custom_id: custom_id(),
            placeholder: text("placeholder"),
            min: count("min_values"),
            max: count("max_values"),
            required,
            disabled,
            defaults: value["default_values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| {
                    let kind = match d["type"].as_str()? {
                        "user" => DefaultKind::User,
                        "role" => DefaultKind::Role,
                        "channel" => DefaultKind::Channel,
                        _ => return None,
                    };
                    Some((kind, id_of(&d["id"])?))
                })
                .collect(),
            channel_types: value["channel_types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_u64().map(|t| t as u8))
                .collect(),
        }),
        10 => NodeKind::Text(text("content").unwrap_or_default()),
        18 => NodeKind::Label {
            label: text("label").unwrap_or_default(),
            description: text("description"),
            child: Box::new(node(&value["component"], &[path, &[0]].concat())?),
        },
        19 => NodeKind::FileUpload(FileUpload {
            custom_id: custom_id(),
            min: count("min_values"),
            max: count("max_values"),
            required,
            file_types: value["file_types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
        }),
        21 => NodeKind::RadioGroup { custom_id: custom_id(), options: options(), required },
        22 => NodeKind::CheckboxGroup {
            custom_id: custom_id(),
            options: options(),
            min: count("min_values"),
            max: count("max_values"),
            required,
        },
        23 => NodeKind::Checkbox { custom_id: custom_id(), default: value["default"].as_bool() },
        2 | 11..=14 | 17 | 20 => NodeKind::Unsubmittable(Component::deserialize(value).ok()?),
        // Kept only with something in it.
        9 if section_kept(value) => NodeKind::Unsubmittable(Component::deserialize(value).ok()?),
        16 if !value["content_inventory_entry"].is_null() => NodeKind::Unsubmittable(Component::Unknown),
        _ => return None,
    };
    let path = path.iter().map(usize::to_string).collect::<Vec<_>>().join(",");
    Some(Node { path, kind })
}

/// A section the client keeps: one kept child and a kept accessory.
fn section_kept(value: &Value) -> bool {
    let kept = |v: &Value| node(v, &[0]).is_some();
    value["components"].as_array().is_some_and(|c| c.iter().any(kept)) && kept(&value["accessory"])
}

impl Node {
    /// Whether it holds a value the submit sends.
    pub fn is_field(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::TextInput(_)
                | NodeKind::StringSelect(_)
                | NodeKind::EntitySelect(_)
                | NodeKind::FileUpload(_)
                | NodeKind::RadioGroup { .. }
                | NodeKind::CheckboxGroup { .. }
                | NodeKind::Checkbox { .. }
        )
    }

    /// Whether a field must have something (a label's asterisk).
    pub fn is_required(&self) -> bool {
        match &self.kind {
            NodeKind::TextInput(input) => input.required,
            NodeKind::StringSelect(select) => select.required,
            NodeKind::EntitySelect(select) => select.required,
            NodeKind::FileUpload(upload) => upload.required,
            NodeKind::RadioGroup { required, .. } | NodeKind::CheckboxGroup { required, .. } => *required,
            _ => false,
        }
    }
}

/// What a field holds, kept by its path (935548's states, one per type).
#[derive(Clone, Debug, PartialEq)]
pub enum FieldState {
    Text(String),
    /// A string select's picked values.
    Picks(Vec<String>),
    /// An entity select's picked ids.
    Entities(Vec<Snowflake>),
    /// A file upload's uploads, by the ids of the modal's upload list.
    Files(Vec<String>),
    Radio(Option<String>),
    /// A checkbox group's checked values.
    Checks(Vec<String>),
    Check(bool),
}

impl FieldState {
    /// Whether it is a state of `kind`'s type (`t?.type === n.type`): a
    /// state of another type is not that field's.
    fn fits(&self, kind: &NodeKind) -> bool {
        matches!(
            (self, kind),
            (FieldState::Text(_), NodeKind::TextInput(_))
                | (FieldState::Picks(_), NodeKind::StringSelect(_))
                | (FieldState::Entities(_), NodeKind::EntitySelect(_))
                | (FieldState::Files(_), NodeKind::FileUpload(_))
                | (FieldState::Radio(_), NodeKind::RadioGroup { .. })
                | (FieldState::Checks(_), NodeKind::CheckboxGroup { .. })
                | (FieldState::Check(_), NodeKind::Checkbox { .. })
        )
    }
}

/// What was typed and picked in one modal `custom_id`, kept across Cancel
/// and reopening (from any app, in any channel) until a submit succeeds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft(BTreeMap<String, FieldState>);

impl Draft {
    /// The field's state, if it holds one of its own type.
    pub fn state(&self, node: &Node) -> Option<&FieldState> {
        self.0.get(&node.path).filter(|state| state.fits(&node.kind))
    }

    pub fn set(&mut self, node: &Node, state: FieldState) {
        self.0.insert(node.path.clone(), state);
    }

    /// Give each field with no state of its own its first state
    /// ([`initial`]), as each field does when drawn. Returns the paths that
    /// got one, which are checked at once. A stored state of another type
    /// is replaced, where the client keeps it and its Submit then does
    /// nothing until that field is changed: Neucord sends what the client
    /// sends for the same modal without the stray state.
    pub fn start(&mut self, modal: &Modal, resolvable: impl Fn(DefaultKind, Snowflake) -> bool) -> Vec<String> {
        let mut started = Vec::new();
        for node in modal.fields() {
            if self.state(node).is_some() {
                continue;
            }
            self.0.remove(&node.path);
            if let Some(state) = initial(node, &resolvable) {
                self.0.insert(node.path.clone(), state);
                started.push(node.path.clone());
            }
        }
        started
    }
}

/// A field's first state (232430's `no`, `nt`, `er`, `e7`, `ev`, `eC`): a
/// text input's `value`; the `default` options, if any; the defaults that
/// resolve, if any; a radio group's first `default`; a checkbox's
/// `default`, if given.
pub fn initial(node: &Node, resolvable: impl Fn(DefaultKind, Snowflake) -> bool) -> Option<FieldState> {
    let defaults = |options: &[SelectOption]| -> Option<Vec<String>> {
        let picked: Vec<String> = options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect();
        (!picked.is_empty()).then_some(picked)
    };
    match &node.kind {
        NodeKind::TextInput(input) => input.value.clone().map(FieldState::Text),
        NodeKind::StringSelect(select) => defaults(&select.options).map(FieldState::Picks),
        NodeKind::EntitySelect(select) => {
            let ids: Vec<Snowflake> =
                select.defaults.iter().filter(|(kind, id)| resolvable(*kind, *id)).map(|(_, id)| *id).collect();
            (!ids.is_empty()).then_some(FieldState::Entities(ids))
        }
        NodeKind::RadioGroup { options, .. } => {
            options.iter().find(|o| o.default).map(|o| FieldState::Radio(Some(o.value.clone())))
        }
        NodeKind::CheckboxGroup { options, .. } => defaults(options).map(FieldState::Checks),
        NodeKind::Checkbox { default, .. } => default.map(FieldState::Check),
        _ => None,
    }
}

/// A count as an ICU number argument writes it in English: grouped by
/// thousands.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What is wrong with a field's state, in the client's words (`N` in
/// 911408, context `"modal"`). Lengths count UTF-16 units; a missing
/// minimum or maximum skips its check.
pub fn validate(node: &Node, state: Option<&FieldState>) -> Option<String> {
    let state = state.filter(|s| s.fits(&node.kind));
    let required = || node.is_required().then(|| "This field is required.".to_owned());
    match (&node.kind, state) {
        (NodeKind::TextInput(input), Some(FieldState::Text(text))) if !text.is_empty() => {
            let length = message_length(text);
            let short = input.min_length.is_some_and(|min| length < min);
            let long = input.max_length.is_some_and(|max| length > max);
            (short || long).then(|| {
                format!(
                    "Must be between {} and {} in length.",
                    grouped(input.min_length.unwrap_or(TEXT_MIN)),
                    grouped(input.max_length.unwrap_or(TEXT_MAX))
                )
            })
        }
        (NodeKind::TextInput(_), _) => required(),
        (NodeKind::StringSelect(select), state) => {
            let count = match state {
                Some(FieldState::Picks(values)) => values.len(),
                _ => 0,
            };
            pick_error(count, !select.required, select.min, select.max)
        }
        (NodeKind::EntitySelect(select), state) => {
            let count = match state {
                Some(FieldState::Entities(ids)) => ids.len(),
                _ => 0,
            };
            pick_error(count, !select.required, select.min, select.max)
        }
        (NodeKind::CheckboxGroup { min, max, required, .. }, state) => {
            let count = match state {
                Some(FieldState::Checks(values)) => values.len(),
                _ => 0,
            };
            pick_error(count, !required, *min, *max)
        }
        (NodeKind::FileUpload(upload), state) => {
            let count = match state {
                Some(FieldState::Files(ids)) => ids.len(),
                _ => 0,
            };
            match count {
                0 => required(),
                n if upload.min.is_some_and(|min| n < min) => {
                    Some(format!("You need to upload at least {} files.", grouped(upload.min.unwrap_or_default())))
                }
                n if upload.max.is_some_and(|max| n > max) => Some(match upload.max {
                    Some(1) => "You can upload up to 1 file.".to_owned(),
                    max => format!("You can upload up to {} files.", grouped(max.unwrap_or_default())),
                }),
                _ => None,
            }
        }
        (NodeKind::RadioGroup { .. }, Some(FieldState::Radio(Some(_)))) => None,
        (NodeKind::RadioGroup { .. }, _) => required(),
        _ => None,
    }
}

/// The longest start of `inserted`, in whole characters, that keeps
/// `current` within `max` UTF-16 units: an input's `maxlength` cuts what is
/// typed or pasted short rather than refusing it. A prefill over the limit
/// is not cut (it shows its error instead).
pub fn fit<'a>(current: &str, inserted: &'a str, max: Option<usize>) -> &'a str {
    let Some(max) = max else { return inserted };
    let mut room = max.saturating_sub(message_length(current));
    let mut end = 0;
    for (at, c) in inserted.char_indices() {
        let Some(left) = room.checked_sub(c.len_utf16()) else { break };
        room = left;
        end = at + c.len_utf8();
    }
    &inserted[..end]
}

/// The modal holds a component whose submit throws (`invariant(false,
/// "unreachable")` in 969508): nothing can be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsubmittable;

/// `data.components` of a submit (969508): the modal's tree with each
/// field's value, or `null` (`false` for a checkbox) where it has no state
/// of its type. `uploads` are the ids of the modal's uploads, in the order
/// `attachments` lists them; a file field sends its uploads' indexes there
/// (-1 for one not among them).
pub fn components(nodes: &[Node], draft: &Draft, uploads: &[String]) -> Result<Value, Unsubmittable> {
    nodes.iter().map(|node| submitted(node, draft, uploads)).collect::<Result<Vec<_>, _>>().map(Value::Array)
}

fn submitted(node: &Node, draft: &Draft, uploads: &[String]) -> Result<Value, Unsubmittable> {
    let state = draft.state(node);
    Ok(match &node.kind {
        NodeKind::Row(children) => json!({"type": 1, "components": components(children, draft, uploads)?}),
        NodeKind::Label { child, .. } => json!({"type": 18, "component": submitted(child, draft, uploads)?}),
        NodeKind::Text(_) => json!({"type": 10}),
        NodeKind::TextInput(input) => {
            let value = match state {
                Some(FieldState::Text(text)) => json!(text),
                _ => Value::Null,
            };
            json!({"type": 4, "custom_id": input.custom_id, "value": value})
        }
        NodeKind::StringSelect(select) => {
            let values = match state {
                Some(FieldState::Picks(values)) => json!(values),
                _ => Value::Null,
            };
            json!({"type": 3, "custom_id": select.custom_id, "values": values})
        }
        NodeKind::EntitySelect(select) => {
            let values = match state {
                Some(FieldState::Entities(ids)) => json!(ids),
                _ => Value::Null,
            };
            json!({"type": select.kind as u8, "custom_id": select.custom_id, "values": values})
        }
        NodeKind::FileUpload(upload) => {
            let values = match state {
                Some(FieldState::Files(ids)) => json!(
                    ids.iter()
                        .map(|id| uploads.iter().position(|u| u == id).map_or(-1, |i| i as i64))
                        .collect::<Vec<_>>()
                ),
                _ => Value::Null,
            };
            json!({"type": 19, "custom_id": upload.custom_id, "values": values})
        }
        NodeKind::RadioGroup { custom_id, .. } => {
            let value = match state {
                Some(FieldState::Radio(value)) => json!(value),
                _ => Value::Null,
            };
            json!({"type": 21, "custom_id": custom_id, "value": value})
        }
        NodeKind::CheckboxGroup { custom_id, .. } => {
            let values = match state {
                Some(FieldState::Checks(values)) => json!(values),
                _ => Value::Null,
            };
            json!({"type": 22, "custom_id": custom_id, "values": values})
        }
        NodeKind::Checkbox { custom_id, .. } => {
            let value = matches!(state, Some(FieldState::Check(true)));
            json!({"type": 23, "custom_id": custom_id, "value": value})
        }
        NodeKind::Unsubmittable(_) => return Err(Unsubmittable),
    })
}

/// What a submit sends (969508).
pub struct Submit<'a> {
    pub application: Snowflake,
    pub channel: Snowflake,
    /// The channel's server; `null` in a DM, never absent.
    pub guild: Option<Snowflake>,
    /// The session at submit time.
    pub session: &'a str,
    pub modal: Snowflake,
    pub custom_id: &'a str,
    /// From [`components`].
    pub components: Value,
    /// The modal's uploads (`uploads::Sent::body`), left out when none.
    pub attachments: Vec<Value>,
}

impl Submit<'_> {
    pub fn body(&self, nonce: Snowflake) -> Value {
        let mut data = json!({ "id": self.modal, "custom_id": self.custom_id, "components": self.components });
        if !self.attachments.is_empty() {
            data["attachments"] = json!(self.attachments);
        }
        json!({
            "type": MODAL_SUBMIT,
            "application_id": self.application,
            "channel_id": self.channel,
            "guild_id": self.guild,
            "data": data,
            "session_id": self.session,
            "nonce": nonce.to_string(),
        })
    }
}

/// Where the last submit is (750128's state, for its one nonce): it
/// outlives the form, so an answer still ends a submit whose form closed.
/// One is in flight at a time across all forms, where the client posts a
/// second one untracked (its assert is swallowed): Neucord holds it back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Submission {
    #[default]
    Idle,
    InFlight(Snowflake),
    Errored(Snowflake),
    Succeeded(Snowflake),
}

impl Submission {
    /// A submit goes as `nonce`, unless another is in flight.
    pub fn start(&mut self, nonce: Snowflake) -> bool {
        if self.in_flight().is_some() {
            return false;
        }
        *self = Submission::InFlight(nonce);
        true
    }

    pub fn in_flight(&self) -> Option<Snowflake> {
        match self {
            Submission::InFlight(nonce) => Some(*nonce),
            _ => None,
        }
    }

    /// Discord (or the request, or the timer) answered `nonce`: only the
    /// last submit's answer counts, and a success still lands after a
    /// failure (a late answer after the timer). Returns whether it was ours.
    pub fn answered(&mut self, nonce: Snowflake, ok: bool) -> bool {
        match *self {
            Submission::InFlight(n) if n == nonce => {
                *self = if ok { Submission::Succeeded(n) } else { Submission::Errored(n) };
                true
            }
            Submission::Errored(n) if n == nonce && ok => {
                *self = Submission::Succeeded(n);
                true
            }
            _ => false,
        }
    }

    /// The timer: a submit still in flight as `nonce` fails.
    pub fn timed_out(&mut self, nonce: Snowflake) -> bool {
        self.in_flight() == Some(nonce) && self.answered(nonce, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal(components: Value) -> Modal {
        Modal::from_event(&json!({
            "id": "70", "nonce": "71", "channel_id": "600", "custom_id": "feedback", "title": "Tell us",
            "application": {"id": "800", "name": "Formy", "icon": "f0"},
            "components": components,
        }))
        .unwrap()
    }

    fn everything() -> Value {
        json!([
            {"type": 1, "components": [{"type": 4, "custom_id": "name", "style": 1, "label": "Name"}]},
            {"type": 18, "label": "About", "description": "Say more",
             "component": {"type": 4, "custom_id": "about", "style": 2, "min_length": 5, "max_length": 200, "required": true}},
            {"type": 18, "label": "Pick", "component": {"type": 3, "custom_id": "pick",
             "options": [{"label": "A", "value": "a"}, {"label": "B", "value": "b", "default": true}]}},
            {"type": 10, "content": "Read **this**"},
            {"type": 99},
            {"type": 18, "label": "Gone", "component": {"type": 99}},
            {"type": 18, "label": "One", "component": {"type": 21, "custom_id": "one",
             "options": [{"label": "X", "value": "x"}, {"label": "Y", "value": "y", "default": true}]}},
            {"type": 18, "label": "Some", "component": {"type": 22, "custom_id": "some",
             "options": [{"label": "P", "value": "p", "default": true}, {"label": "Q", "value": "q"}], "max_values": 2}},
            {"type": 18, "label": "Agree", "component": {"type": 23, "custom_id": "agree"}},
            {"type": 18, "label": "Files", "component": {"type": 19, "custom_id": "files", "max_values": 2,
             "file_types": ["image", ".pdf"]}},
            {"type": 18, "label": "Who", "component": {"type": 5, "custom_id": "who",
             "default_values": [{"id": "301", "type": "user"}, {"id": "399", "type": "user"}]}},
        ])
    }

    #[test]
    fn a_form_keeps_what_the_client_keeps_at_its_paths() {
        let mut event = json!({
            "id": "70", "channel_id": "600", "custom_id": "feedback", "title": "Tell us",
            "application": {"id": "800", "name": "Formy", "icon": null,
                            "bot": {"id": "801", "username": "Formy Bot", "discriminator": "0", "bot": true}},
            "components": everything(),
            "resolved": {"users": {"301": {"id": "301", "username": "alice"}},
                         "members": {"301": {"nick": "Al", "roles": []}, "302": {"nick": "Bo", "roles": []}}},
        });
        let form = Modal::from_event(&event).unwrap();
        assert_eq!(form.nonce, None, "any modal opens, ours or not");
        assert_eq!(form.application.bot.as_ref().map(|b| b.id), Some(Snowflake(801)));
        let paths: Vec<&str> = form.components.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["0", "1", "2", "3", "6", "7", "8", "9", "10"], "dropped indexes still counted");
        let fields: Vec<&str> = form.fields().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(fields, ["0,0", "1,0", "2,0", "6,0", "7,0", "8,0", "9,0", "10,0"]);
        let NodeKind::Row(row) = &form.components[0].kind else { panic!() };
        let NodeKind::TextInput(name) = &row[0].kind else { panic!() };
        assert!(!name.required && !name.paragraph && name.label.as_deref() == Some("Name"));
        let NodeKind::Label { child, description, .. } = &form.components[1].kind else { panic!() };
        assert_eq!(description.as_deref(), Some("Say more"));
        assert!(
            matches!(&child.kind, NodeKind::TextInput(t) if t.paragraph && t.required && t.max_length == Some(200))
        );
        assert_eq!(form.components[3].kind, NodeKind::Text("Read **this**".into()));
        assert_eq!(form.resolved_users.len(), 1);
        assert_eq!(form.resolved_members.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [Snowflake(301)]);
        for key in ["id", "channel_id", "custom_id"] {
            event.as_object_mut().unwrap().remove(key);
            assert_eq!(Modal::from_event(&event), None, "{key}");
        }
    }

    #[test]
    fn a_field_starts_from_its_defaults_unless_the_draft_has_its_own() {
        let mut form = modal(everything());
        let NodeKind::Row(row) = &mut form.components[0].kind else { panic!() };
        let NodeKind::TextInput(name) = &mut row[0].kind else { panic!() };
        name.value = Some("Ann".into());
        let known = |_: DefaultKind, id: Snowflake| id == Snowflake(301);
        let mut draft = Draft::default();
        let fields = form.fields();
        let (name, pick, one, some, agree, who) = (fields[0], fields[2], fields[3], fields[4], fields[5], fields[7]);
        draft.set(pick, FieldState::Picks(vec!["a".into()]));
        // Another modal's state of another type at this path isn't the field's.
        draft.set(one, FieldState::Text("stray".into()));
        let started = draft.start(&form, known);
        assert_eq!(started, ["0,0", "6,0", "7,0", "10,0"]);
        assert_eq!(draft.state(name), Some(&FieldState::Text("Ann".into())));
        assert_eq!(draft.state(pick), Some(&FieldState::Picks(vec!["a".into()])), "stored wins");
        assert_eq!(draft.state(one), Some(&FieldState::Radio(Some("y".into()))));
        assert_eq!(draft.state(some), Some(&FieldState::Checks(vec!["p".into()])));
        assert_eq!(draft.state(agree), None, "no default given");
        assert_eq!(draft.state(who), Some(&FieldState::Entities(vec![Snowflake(301)])), "only what resolves");
        let checkbox =
            Node { path: "0".into(), kind: NodeKind::Checkbox { custom_id: "c".into(), default: Some(false) } };
        assert_eq!(initial(&checkbox, known), Some(FieldState::Check(false)));
    }

    fn text_input(min: Option<usize>, max: Option<usize>, required: bool) -> Node {
        Node {
            path: "0".into(),
            kind: NodeKind::TextInput(TextInput {
                custom_id: "t".into(),
                paragraph: false,
                label: None,
                value: None,
                placeholder: None,
                required,
                min_length: min,
                max_length: max,
            }),
        }
    }

    #[test]
    fn fields_are_checked_in_the_clients_words() {
        let text = |s: &str| FieldState::Text(s.into());
        let required = text_input(Some(2), Some(4), true);
        assert_eq!(validate(&required, None).as_deref(), Some("This field is required."));
        assert_eq!(validate(&required, Some(&text(""))).as_deref(), Some("This field is required."));
        assert_eq!(validate(&text_input(Some(2), None, false), Some(&text(""))), None, "optional and empty");
        assert_eq!(validate(&required, Some(&text("😀😀😀"))).as_deref(), Some("Must be between 2 and 4 in length."));
        assert_eq!(validate(&required, Some(&text("😀😀"))), None, "an emoji counts two");
        assert_eq!(
            validate(&text_input(None, Some(4000), false), Some(&text(&"a".repeat(4001)))).as_deref(),
            Some("Must be between 0 and 4,000 in length.")
        );
        assert_eq!(validate(&text_input(None, None, false), Some(&text(&"a".repeat(9000)))), None);

        let select = |required, min, max| Node {
            path: "0".into(),
            kind: NodeKind::StringSelect(StringSelect {
                custom_id: "s".into(),
                options: vec![],
                placeholder: None,
                min,
                max,
                required,
                disabled: false,
            }),
        };
        let picks = |n: usize| FieldState::Picks((0..n).map(|i| i.to_string()).collect());
        assert_eq!(validate(&select(false, Some(1), Some(3)), None), None, "an optional modal select may be empty");
        assert_eq!(validate(&select(true, Some(0), None), Some(&picks(0))).as_deref(), Some("This field is required."));
        assert_eq!(validate(&select(false, Some(2), Some(3)), Some(&picks(1))).as_deref(), Some("Select at least 2"));
        assert_eq!(validate(&select(false, Some(2), Some(3)), Some(&picks(4))).as_deref(), Some("Select at most 3"));
        assert_eq!(validate(&select(false, None, None), Some(&picks(30))), None);
        // The message rule is left as it was: empty only where min is 0.
        assert_eq!(pick_error(0, true, Some(1), Some(1)), None);
        assert_eq!(pick_error(0, false, Some(1), Some(3)).as_deref(), Some("This field is required."));

        let files = |required, min, max| Node {
            path: "0".into(),
            kind: NodeKind::FileUpload(FileUpload { custom_id: "f".into(), min, max, required, file_types: vec![] }),
        };
        let uploaded = |n: usize| FieldState::Files((0..n).map(|i| i.to_string()).collect());
        assert_eq!(validate(&files(true, None, None), None).as_deref(), Some("This field is required."));
        assert_eq!(validate(&files(false, None, None), None), None);
        assert_eq!(
            validate(&files(false, Some(2), None), Some(&uploaded(1))).as_deref(),
            Some("You need to upload at least 2 files.")
        );
        assert_eq!(
            validate(&files(false, None, Some(1)), Some(&uploaded(2))).as_deref(),
            Some("You can upload up to 1 file.")
        );
        assert_eq!(
            validate(&files(false, None, Some(3)), Some(&uploaded(4))).as_deref(),
            Some("You can upload up to 3 files.")
        );

        let radio = |required| Node {
            path: "0".into(),
            kind: NodeKind::RadioGroup { custom_id: "r".into(), options: vec![], required },
        };
        assert_eq!(validate(&radio(true), Some(&FieldState::Radio(None))).as_deref(), Some("This field is required."));
        assert_eq!(validate(&radio(false), None), None);
        assert_eq!(validate(&radio(true), Some(&FieldState::Radio(Some("x".into())))), None);
        let group = Node {
            path: "0".into(),
            kind: NodeKind::CheckboxGroup {
                custom_id: "g".into(),
                options: vec![],
                min: Some(2),
                max: Some(2),
                required: false,
            },
        };
        assert_eq!(validate(&group, Some(&FieldState::Checks(vec!["a".into()]))).as_deref(), Some("Select at least 2"));
        let checkbox = Node { path: "0".into(), kind: NodeKind::Checkbox { custom_id: "c".into(), default: None } };
        assert_eq!(validate(&checkbox, None), None);
    }

    #[test]
    fn typing_stops_at_the_limit_in_utf16_units() {
        assert_eq!(fit("abc", "defgh", Some(5)), "de");
        assert_eq!(fit("abc", "d😀", Some(5)), "d", "an emoji isn't split");
        assert_eq!(fit("abc", "😀", Some(5)), "😀");
        assert_eq!(fit("abcdef", "g", Some(5)), "", "a prefill over the limit takes nothing more");
        assert_eq!(fit("abc", "defgh", None), "defgh");
    }

    #[test]
    fn a_submit_sends_what_the_client_sends() {
        let form = modal(everything());
        let fields = form.fields();
        let mut draft = Draft::default();
        draft.set(fields[1], FieldState::Text(String::new()));
        draft.set(fields[2], FieldState::Picks(vec![]));
        draft.set(fields[6], FieldState::Files(vec!["u1".into(), "gone".into()]));
        draft.set(fields[7], FieldState::Entities(vec![Snowflake(301)]));
        let components = components(&form.components, &draft, &["u0".into(), "u1".into()]).unwrap();
        let submit = Submit {
            application: Snowflake(800),
            channel: Snowflake(201),
            guild: None,
            session: "s",
            modal: form.id,
            custom_id: &form.custom_id,
            components,
            attachments: vec![],
        };
        assert_eq!(
            submit.body(Snowflake(9)),
            json!({
                "type": 5, "application_id": "800", "channel_id": "201", "guild_id": null,
                "data": {"id": "70", "custom_id": "feedback", "components": [
                    {"type": 1, "components": [{"type": 4, "custom_id": "name", "value": null}]},
                    {"type": 18, "component": {"type": 4, "custom_id": "about", "value": ""}},
                    {"type": 18, "component": {"type": 3, "custom_id": "pick", "values": []}},
                    {"type": 10},
                    {"type": 18, "component": {"type": 21, "custom_id": "one", "value": null}},
                    {"type": 18, "component": {"type": 22, "custom_id": "some", "values": null}},
                    {"type": 18, "component": {"type": 23, "custom_id": "agree", "value": false}},
                    {"type": 18, "component": {"type": 19, "custom_id": "files", "values": [1, -1]}},
                    {"type": 18, "component": {"type": 5, "custom_id": "who", "values": ["301"]}},
                ]},
                "session_id": "s", "nonce": "9",
            })
        );
        let with_files = Submit { attachments: vec![json!({"id": "0"})], guild: Some(Snowflake(500)), ..submit };
        let body = with_files.body(Snowflake(9));
        assert_eq!((&body["guild_id"], &body["data"]["attachments"]), (&json!("500"), &json!([{"id": "0"}])));
    }

    #[test]
    fn a_form_with_a_button_can_never_be_sent() {
        let form = modal(json!([
            {"type": 1, "components": [{"type": 2, "custom_id": "b", "style": 1, "label": "Go"}]},
        ]));
        assert_eq!(components(&form.components, &Draft::default(), &[]), Err(Unsubmittable));
        // An empty section is dropped, so it doesn't stop the submit.
        let form =
            modal(json!([{"type": 9, "components": [], "accessory": {"type": 2, "custom_id": "b", "style": 1}}]));
        assert_eq!(form.components, []);
    }

    #[test]
    fn one_submit_at_a_time_and_only_its_answer_counts() {
        let mut submission = Submission::default();
        assert!(submission.start(Snowflake(1)));
        assert!(!submission.start(Snowflake(2)), "another is in flight");
        assert!(!submission.answered(Snowflake(2), true), "not ours");
        assert!(submission.answered(Snowflake(1), false));
        assert_eq!(submission, Submission::Errored(Snowflake(1)));
        assert!(submission.answered(Snowflake(1), true), "a late success still lands");
        assert!(submission.start(Snowflake(3)));
        assert!(submission.timed_out(Snowflake(3)));
        assert!(!submission.timed_out(Snowflake(3)));
        assert!(submission.start(Snowflake(4)), "again after an error, with a new nonce");
        assert!(!submission.answered(Snowflake(3), true), "the old nonce no longer counts");
    }

    #[test]
    fn the_header_names_the_app_as_the_client_does() {
        let mut form = modal(json!([]));
        assert_eq!(form.app_name(None), "Formy");
        form.application.bot = Some(
            serde_json::from_value(json!({"id": "801", "username": "Formy Bot", "discriminator": "0", "bot": true}))
                .unwrap(),
        );
        assert_eq!(form.app_name(None), "Formy Bot");
        assert_eq!(form.app_name(Some("Formy Nick")), "Formy Nick");
        // A bot that isn't a member of the selected server isn't asked about.
        assert_eq!(
            form.app_icon(None, 128).as_deref(),
            Some("https://cdn.discordapp.com/app-icons/800/f0.png?size=128")
        );
        assert_eq!(
            form.app_icon(Some((Snowflake(500), Some("m1"))), 128).as_deref(),
            Some("https://cdn.discordapp.com/guilds/500/users/801/avatars/m1.png?size=128")
        );
        // A member bot with no avatar: the app icon.
        assert_eq!(
            form.app_icon(Some((Snowflake(500), None)), 128).as_deref(),
            Some("https://cdn.discordapp.com/app-icons/800/f0.png?size=128")
        );
    }

    #[test]
    fn enter_submits_only_a_form_of_one_short_input() {
        let one = modal(json!([{"type": 1, "components": [{"type": 4, "custom_id": "a", "style": 1}]},
                               {"type": 18, "label": "L", "component": {"type": 4, "custom_id": "b", "style": 2}}]));
        assert!(one.enter_submits());
        assert_eq!(one.autofocus(), Some("0,0"));
        let two = modal(json!([{"type": 18, "label": "L", "component": {"type": 4, "custom_id": "a", "style": 1}},
                               {"type": 18, "label": "W", "component": {"type": 6, "custom_id": "w"}}]));
        assert!(!two.enter_submits(), "a select's search box is an input too");
        assert_eq!(two.autofocus(), Some("0,0"));
        let text_first = modal(json!([{"type": 10, "content": "hi"},
                                      {"type": 1, "components": [{"type": 4, "custom_id": "a", "style": 1}]}]));
        assert_eq!(text_first.autofocus(), None);
    }

    fn file_field(min: Option<usize>, max: Option<usize>, types: &[&str]) -> FileUpload {
        let file_types = types.iter().map(|t| (*t).to_owned()).collect();
        FileUpload { custom_id: "f".into(), min, max, required: false, file_types }
    }

    #[test]
    fn a_file_field_says_what_it_takes_in_the_clients_words() {
        const MIB: u64 = 1024 * 1024;
        let image = |min, max| file_field(min, max, &["image", ".pdf"]);
        assert_eq!(image(None, Some(1)).hint(20 * MIB), "Upload 1 image or .pdf file under 20 MB.");
        assert_eq!(image(Some(1), Some(3)).hint(20 * MIB), "Upload up to 3 image or .pdf files under 20 MB.");
        assert_eq!(image(Some(2), Some(2)).hint(20 * MIB), "Upload 2 image or .pdf files under 20 MB.");
        assert_eq!(image(Some(2), Some(4)).hint(20 * MIB), "Upload 2\u{2013}4 image or .pdf files under 20 MB.");
        let any = |min, max| file_field(min, max, &[]);
        assert_eq!(any(None, Some(1)).hint(500 * MIB), "Upload a file under 500 MB.");
        assert_eq!(any(None, Some(5)).hint(20 * MIB), "Upload up to 5 files under 20 MB.");
        assert_eq!(any(Some(3), Some(3)).hint(20 * MIB), "Upload 3 files under 20 MB.");
        assert_eq!(any(Some(2), Some(10)).hint(20 * MIB), "Upload 2\u{2013}10 files under 20 MB.");
        assert_eq!(any(None, Some(1)).hint(1024 * MIB), "Upload a file under 1 GB.");
        assert_eq!(any(None, Some(1)).hint(10 * MIB + 1), "Upload a file under 11 MB.", "rounded up");
        assert_eq!(any(None, None).hint(20 * MIB), "Upload a file under 20 MB.", "the API's default");
        assert_eq!(any(None, Some(1)).drop_words(), ("Drop file here or ", "browse"));
        assert_eq!(any(None, Some(2)).drop_words().0, "Drop files here or ");
        assert_eq!(any(None, None).drop_words().0, "Drop file here or ");
        assert!(any(None, Some(2)).picks_several() && !any(None, Some(1)).picks_several());
        assert!(!any(None, None).picks_several() && !any(None, None).full(50));
        assert!(any(None, Some(2)).full(2) && !any(None, Some(2)).full(1));
    }

    #[test]
    fn files_are_added_after_the_count_the_sizes_and_the_type() {
        const MIB: u64 = 1024 * 1024;
        let limits = Uploads { file_size: 20 * MIB, files: 10, total_size: 500 * MIB };
        let field = file_field(None, Some(2), &["image"]);
        let check = |already, added: &[(&str, u64)]| field.check(&limits, already, added);
        assert_eq!(check(0, &[("a.png", 1), ("b.png", 1)]), Ok(()));
        assert_eq!(check(1, &[("a.txt", 30 * MIB), ("b.png", 1)]), Err(FileRefusal::TooMany { max: 2 }), "count first");
        assert_eq!(check(0, &[("a.txt", 20 * MIB + 1)]), Err(FileRefusal::TooLarge { max: 20 * MIB }), "then size");
        assert_eq!(check(0, &[("a.txt", 0)]), Err(FileRefusal::Type { types: "image".into() }), "no empty check");
        assert_eq!(check(1, &[("b.PNG", 1)]), Ok(()));
        let many = file_field(None, None, &[]);
        let big = [("a", 20 * MIB); 26];
        assert_eq!(many.check(&limits, 40, &big), Err(FileRefusal::TotalTooLarge { max: 500 * MIB }));
        assert_eq!(many.check(&limits, 40, &big[..25]), Ok(()), "no max, no count; those there don't count");

        assert_eq!(FileRefusal::TooMany { max: 1 }.body(), "You can upload up to 1 file.");
        assert_eq!(FileRefusal::TooMany { max: 3 }.heading(), "Too many uploads!");
        assert_eq!(FileRefusal::TooMany { max: 3 }.body(), "You can upload up to 3 files.");
        assert_eq!(FileRefusal::TooLarge { max: 20 * MIB }.heading(), "Your files are too powerful");
        assert_eq!(FileRefusal::TooLarge { max: 20 * MIB }.body(), "Max file size is 20.00 MB please.");
        assert_eq!(
            FileRefusal::TotalTooLarge { max: 500 * MIB }.body(),
            "Total message size must be less than 500.00 MB"
        );
        let wrong = FileRefusal::Type { types: "image or .pdf".into() };
        assert_eq!((wrong.heading(), wrong.body().as_str()), ("Invalid file type", "File must be image or .pdf."));
    }
}
