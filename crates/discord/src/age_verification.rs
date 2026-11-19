//! Confirming the account's age group, as the official client's expressive
//! modal does (12247, with `initiateAgeVerificationV2` in 750481 and the
//! requests in 482876 and 295972): the methods Discord offers
//! (`GET /age-verification/methods/v2`), the one chosen
//! (`POST /age-verification/verify/v2` with its `method` and `vendor`), and
//! the vendor's page that answers, which the client shows in an iframe and
//! ends on the messages it posts.
//!
//! Not taken: the other modals the client picks by experiment (the v1
//! methods and `verify`, 962877, 297102 and 929180), Incode's in-page
//! session (`2026-06-age-verification-incode`), and the suspended-account
//! variants.

use serde_json::{Value, json};

use crate::endpoints::call;
use crate::{Error, Rest};

/// The experiment that gives the client its expressive modal, which is the
/// flow built here (`2026-07-expressive-modal-v2`, 462924): variations 1 and 2.
pub const EXPRESSIVE_MODAL: &str = "2026-07-expressive-modal-v2";
/// The experiment that gives the client Incode's in-page session instead
/// (`2026-06-age-verification-incode`, 871309): variation 1.
pub const INCODE: &str = "2026-06-age-verification-incode";

/// Discord's error code for a method it no longer offers
/// (`AGE_VERIFICATION_METHOD_UNAVAILABLE`, 652215).
pub const METHOD_UNAVAILABLE: u64 = 50603;

/// Where the vendor's page says it is done: `eventType` of the message it
/// posts to the window around it (662558). The result is for Discord to
/// act on, so the window only closes the way the client's does; the page
/// asks to close with the other.
pub const RESULT: &str = "Verification.Result";
pub const EXIT_REVIEW: &str = "Widget.ExitReview";

/// How to confirm an age, as Discord lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    /// What Discord named the method: a number or a string, sent back as it came.
    pub method: Value,
    pub vendor: String,
    pub title: String,
    pub description: String,
    pub provided_by: Option<String>,
}

impl Method {
    /// The body of the request that starts it (`{ method, vendor }`).
    pub fn request_body(&self) -> Value {
        json!({ "method": self.method, "vendor": self.vendor })
    }

    /// What names the row: the client's key (`${method}-${vendor}`).
    pub fn key(&self) -> String {
        let method = match &self.method {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        format!("{method}-{}", self.vendor)
    }
}

/// What `methods/v2` answers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Methods {
    pub methods: Vec<Method>,
    pub footer_message: Option<String>,
    /// Shown above the methods when Discord's providers have trouble.
    pub outage_banner_message: Option<String>,
}

impl Methods {
    pub fn from_json(value: &Value) -> Option<Methods> {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        let methods = value["methods"]
            .as_array()?
            .iter()
            .filter_map(|m| {
                Some(Method {
                    method: Some(m["method"].clone()).filter(|method| !method.is_null())?,
                    vendor: text(&m["vendor"])?,
                    title: text(&m["title"])?,
                    description: text(&m["description"])?,
                    provided_by: text(&m["provided_by"]),
                })
            })
            .collect();
        Some(Methods {
            methods,
            footer_message: text(&value["footer_message"]),
            outage_banner_message: text(&value["outage_banner_message"]),
        })
    }
}

/// What `verify/v2` answers: where the vendor's page is.
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    /// `verification_webview_url`.
    pub webview_url: Option<String>,
    /// The vendor wants its in-page session, which needs the vendor's code
    /// (`incode_parameters`).
    pub incode: bool,
}

impl Started {
    pub fn from_json(value: &Value) -> Started {
        Started {
            webview_url: value["verification_webview_url"].as_str().filter(|url| !url.is_empty()).map(str::to_owned),
            incode: value["verification_vendor_name"] == "INCODE" || !value["incode_parameters"].is_null(),
        }
    }

    /// The page to show: the vendor's, when it gave one on https.
    pub fn page(&self) -> Option<&str> {
        self.webview_url.as_deref().filter(|url| url.starts_with("https://"))
    }
}

/// Whether the account is shown the flow built here: the client's expressive
/// modal is in its experiment, and Incode's session, which takes the
/// verification before it, is not.
pub fn offered(expressive_variant: Option<i64>, incode_variant: Option<i64>) -> bool {
    matches!(expressive_variant, Some(1 | 2)) && incode_variant != Some(1)
}

/// The page the vendor's is shown in, over discord.com as in the client's
/// iframe (662558: `allow` names camera, microphone and the keys), and what
/// it posts to the window comes out through `webkit.messageHandlers.age`,
/// as the `eventType` alone. Only the frame's own messages count.
pub fn frame_page(url: &str) -> String {
    let url = url.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    format!(
        r#"<!doctype html>
<style>html, body, iframe {{ margin: 0; width: 100%; height: 100%; border: 0; }}</style>
<iframe id="frame" src="{url}" allow="camera; microphone; publickey-credentials-get; publickey-credentials-create"></iframe>
<script>
window.addEventListener("message", event => {{
  const frame = document.getElementById("frame");
  if (event.source === frame.contentWindow && event.data && typeof event.data.eventType === "string") {{
    webkit.messageHandlers.age.postMessage(event.data.eventType);
  }}
}});
</script>
"#
    )
}

impl Rest {
    /// The ways to confirm an age (`methods/v2`).
    pub async fn age_verification_methods(&self) -> Result<Methods, Error> {
        let value: Value = self.request(call!(AGE_VERIFICATION_METHODS_V2), None).await?;
        Methods::from_json(&value).ok_or_else(|| Error::Protocol("age verification methods without a list".into()))
    }

    /// Start `method` (`verify/v2`).
    pub async fn start_age_verification(&self, method: &Method) -> Result<Started, Error> {
        let value: Value = self.request(call!(VERIFY_AGE_V2), Some(method.request_body())).await?;
        Ok(Started::from_json(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Value {
        json!({
            "methods": [
                {"method": 1, "vendor": "K_ID", "title": "Video selfie via k-ID", "description": "Estimate your age.",
                 "provided_by": "Provided by k-ID", "icon": {"paths": [{"d": "M0 0", "fill_rule": "evenodd"}]},
                 "external_window": false},
                {"method": "GOOGLE_WALLET", "vendor": "GOOGLE_WALLET", "title": "Google Wallet", "description": "Use an ID."},
                {"method": 2, "title": "no vendor"},
            ],
            "footer_message": "Your data stays private.",
            "outage_banner_message": null
        })
    }

    #[test]
    fn reads_the_methods_and_what_goes_around_them() {
        let methods = Methods::from_json(&answer()).unwrap();
        assert_eq!(methods.methods.len(), 2, "one that names no vendor is no method");
        let first = &methods.methods[0];
        assert_eq!((first.method.clone(), first.vendor.as_str()), (json!(1), "K_ID"));
        assert_eq!(first.provided_by.as_deref(), Some("Provided by k-ID"));
        assert_eq!(methods.methods[1].provided_by, None);
        assert_eq!(methods.footer_message.as_deref(), Some("Your data stays private."));
        assert_eq!(methods.outage_banner_message, None);
        assert!(Methods::from_json(&json!({})).is_none());
        assert_eq!(Methods::from_json(&json!({"methods": []})).unwrap().methods, []);
    }

    #[test]
    fn the_request_names_the_method_and_vendor_as_they_came() {
        let methods = Methods::from_json(&answer()).unwrap();
        assert_eq!(methods.methods[0].request_body().to_string(), r#"{"method":1,"vendor":"K_ID"}"#);
        assert_eq!(
            methods.methods[1].request_body().to_string(),
            r#"{"method":"GOOGLE_WALLET","vendor":"GOOGLE_WALLET"}"#
        );
        assert_eq!(methods.methods[0].key(), "1-K_ID");
        assert_eq!(methods.methods[1].key(), "GOOGLE_WALLET-GOOGLE_WALLET");
    }

    #[test]
    fn the_vendors_page_is_shown_only_on_https() {
        let started = |v: Value| Started::from_json(&v);
        let page = started(json!({"verification_webview_url": "https://k.example/verify?id=1",
                                  "verification_request_id": "r", "verification_vendor_name": "K_ID"}));
        assert_eq!(page.page(), Some("https://k.example/verify?id=1"));
        assert!(!page.incode);
        assert_eq!(started(json!({"verification_webview_url": "http://k.example/"})).page(), None);
        assert_eq!(started(json!({"verification_webview_url": ""})).page(), None);
        assert_eq!(started(json!({})).page(), None);
        assert!(started(json!({"verification_vendor_name": "INCODE", "incode_parameters": {"api_url": "x"}})).incode);
    }

    #[test]
    fn the_flow_is_offered_where_the_client_shows_its_expressive_modal() {
        assert!(offered(Some(1), None));
        assert!(offered(Some(2), Some(0)));
        assert!(!offered(None, None), "not in the experiment");
        assert!(!offered(Some(0), None), "control");
        assert!(!offered(Some(1), Some(1)), "Incode's session comes first");
    }

    #[test]
    fn the_frame_page_escapes_the_url_and_listens_to_the_frame_alone() {
        let page = frame_page("https://k.example/v?a=1&b=\"x\"<");
        assert!(page.contains(r#"src="https://k.example/v?a=1&amp;b=&quot;x&quot;&lt;""#));
        assert!(page.contains("allow=\"camera; microphone; publickey-credentials-get; publickey-credentials-create\""));
        assert!(page.contains("event.source === frame.contentWindow"));
        assert!(page.contains("messageHandlers.age"));
    }
}
