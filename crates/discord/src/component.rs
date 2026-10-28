//! Message components: bots' buttons and menus, and the layouts of
//! messages built from components alone ("components v2"). Neucord shows
//! them but can't use them yet.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::model::Emoji;

#[derive(Clone, Debug, PartialEq)]
pub enum Component {
    /// Buttons or a menu, side by side.
    Row(Vec<Component>),
    Button(Button),
    /// A menu to pick from: of given options, or of users, roles or channels.
    Select {
        placeholder: Option<String>,
        options: Vec<SelectOption>,
        disabled: bool,
    },
    /// Text beside a button or thumbnail.
    Section {
        components: Vec<Component>,
        accessory: Option<Box<Component>>,
    },
    /// Markup.
    Text(String),
    Thumbnail(Media),
    Gallery(Vec<Media>),
    File {
        media: Media,
        name: Option<String>,
        size: Option<u64>,
    },
    Separator {
        divider: bool,
        large: bool,
    },
    /// A box around components, with a coloured edge like an embed's.
    Container {
        accent: Option<u32>,
        spoiler: bool,
        components: Vec<Component>,
    },
    /// Something newer, or only for forms.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub style: ButtonStyle,
    pub label: Option<String>,
    pub emoji: Option<Emoji>,
    /// Where a link button goes.
    pub url: Option<String>,
    pub disabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Success,
    Destructive,
    Link,
    /// Buys something.
    Premium,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SelectOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub emoji: Option<Emoji>,
    #[serde(default)]
    pub default: bool,
}

/// An image or file a component shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub description: Option<String>,
    pub spoiler: bool,
}

impl Component {
    /// The markup of every text in `components`, nested ones included, in
    /// the order they show.
    pub fn texts(components: &[Component]) -> Vec<&str> {
        let mut out = Vec::new();
        for component in components {
            match component {
                Component::Text(text) => out.push(text.as_str()),
                Component::Row(children) | Component::Container { components: children, .. } => {
                    out.extend(Component::texts(children))
                }
                Component::Section { components, accessory } => {
                    out.extend(Component::texts(components));
                    out.extend(Component::texts(accessory.as_deref().map(std::slice::from_ref).unwrap_or_default()));
                }
                _ => {}
            }
        }
        out
    }

    fn from_value(value: &Value) -> Component {
        let text = |key: &str| value[key].as_str().map(String::from);
        let flag = |key: &str| value[key].as_bool().unwrap_or(false);
        let children = |key: &str| value[key].as_array().into_iter().flatten().map(Component::from_value).collect();
        match value["type"].as_u64().unwrap_or(0) {
            1 => Component::Row(children("components")),
            2 => Component::Button(Button {
                style: match value["style"].as_u64() {
                    Some(1) => ButtonStyle::Primary,
                    Some(3) => ButtonStyle::Success,
                    Some(4) => ButtonStyle::Destructive,
                    Some(5) => ButtonStyle::Link,
                    Some(6) => ButtonStyle::Premium,
                    _ => ButtonStyle::Secondary,
                },
                label: text("label"),
                emoji: Emoji::deserialize(&value["emoji"]).ok(),
                url: text("url"),
                disabled: flag("disabled"),
            }),
            3 | 5..=8 => Component::Select {
                placeholder: text("placeholder"),
                options: value["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| SelectOption::deserialize(o).ok())
                    .collect(),
                disabled: flag("disabled"),
            },
            9 => Component::Section {
                components: children("components"),
                accessory: value.get("accessory").map(|a| Box::new(Component::from_value(a))),
            },
            10 => Component::Text(text("content").unwrap_or_default()),
            11 => Component::Thumbnail(Media::from_item(value, "media")),
            12 => Component::Gallery(
                value["items"].as_array().into_iter().flatten().map(|item| Media::from_item(item, "media")).collect(),
            ),
            13 => Component::File {
                media: Media::from_item(value, "file"),
                name: text("name"),
                size: value["size"].as_u64(),
            },
            // Spacing 2 is large; a divider unless told otherwise.
            14 => Component::Separator {
                divider: value["divider"].as_bool().unwrap_or(true),
                large: value["spacing"].as_u64() == Some(2),
            },
            17 => Component::Container {
                accent: value["accent_color"].as_u64().map(|c| c as u32),
                spoiler: flag("spoiler"),
                components: children("components"),
            },
            _ => Component::Unknown,
        }
    }
}

impl Media {
    /// The item under `key` of a thumbnail, gallery item or file, with its
    /// description and spoiler beside it.
    fn from_item(value: &Value, key: &str) -> Media {
        let media = &value[key];
        let dimension = |key: &str| media[key].as_u64().map(|n| n as u32);
        Media {
            // The proxy sizes images to fit, as for attachments.
            url: media["proxy_url"].as_str().or(media["url"].as_str()).unwrap_or_default().to_owned(),
            width: dimension("width"),
            height: dimension("height"),
            description: value["description"].as_str().map(String::from),
            spoiler: value["spoiler"].as_bool().unwrap_or(false),
        }
    }
}

impl<'de> Deserialize<'de> for Component {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Component::from_value(&Value::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Component> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn buttons_and_menus() {
        let row = parse(
            r#"[{"type":1,"components":[
                {"type":2,"style":1,"label":"Yes","custom_id":"y"},
                {"type":2,"style":5,"label":"Docs","url":"https://example.com","emoji":{"name":"📖"}},
                {"type":3,"custom_id":"m","placeholder":"Pick","options":[{"label":"A","value":"a","default":true}]},
                {"type":5,"custom_id":"u","disabled":true}]}]"#,
        );
        let Component::Row(row) = &row[0] else { panic!("a row") };
        assert!(
            matches!(&row[0], Component::Button(b) if b.style == ButtonStyle::Primary && b.label.as_deref() == Some("Yes"))
        );
        assert!(matches!(&row[1], Component::Button(b) if b.style == ButtonStyle::Link
            && b.url.as_deref() == Some("https://example.com") && b.emoji.as_ref().is_some_and(|e| e.name.as_deref() == Some("📖"))));
        assert!(
            matches!(&row[2], Component::Select { placeholder: Some(p), options, .. } if p == "Pick" && options[0].default)
        );
        assert!(matches!(&row[3], Component::Select { disabled: true, options, .. } if options.is_empty()));
    }

    #[test]
    fn layouts() {
        let parsed = parse(
            r#"[{"type":17,"accent_color":16711680,"components":[
                {"type":9,"components":[{"type":10,"content":"**Hi**"}],
                 "accessory":{"type":11,"media":{"url":"https://cdn.example/a.png","width":80,"height":80}}},
                {"type":14},
                {"type":12,"items":[{"media":{"url":"u","proxy_url":"p"},"spoiler":true,"description":"d"}]},
                {"type":13,"file":{"url":"f"},"name":"a.txt","size":12},
                {"type":99}]}]"#,
        );
        let Component::Container { accent: Some(0xFF0000), spoiler: false, components } = &parsed[0] else {
            panic!("a container")
        };
        assert!(matches!(&components[0], Component::Section { components, accessory: Some(accessory) }
            if components == &[Component::Text("**Hi**".into())]
            && matches!(accessory.as_ref(), Component::Thumbnail(m) if m.width == Some(80))));
        assert_eq!(components[1], Component::Separator { divider: true, large: false });
        assert_eq!(
            components[2],
            Component::Gallery(vec![Media {
                url: "p".into(),
                description: Some("d".into()),
                spoiler: true,
                ..Default::default()
            }])
        );
        assert!(matches!(&components[3], Component::File { name: Some(n), size: Some(12), .. } if n == "a.txt"));
        assert_eq!(components[4], Component::Unknown);
    }

    #[test]
    fn texts_in_order() {
        let parsed = parse(
            r#"[{"type":10,"content":"a"},
                {"type":17,"components":[
                    {"type":9,"components":[{"type":10,"content":"b"},{"type":10,"content":"c"}],
                     "accessory":{"type":2,"style":1,"label":"x"}},
                    {"type":10,"content":"d"}]}]"#,
        );
        assert_eq!(Component::texts(&parsed), ["a", "b", "c", "d"]);
    }
}
