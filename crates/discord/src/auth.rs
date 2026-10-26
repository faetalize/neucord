//! Logging in with a password, and the two-factor step that may follow.
//!
//! Discord answers a suspicious login with a captcha instead, which needs a
//! browser to solve; that surfaces as [`Error::Captcha`].

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Error, Rest};

pub enum Login {
    Token(String),
    /// The password was right; now a second factor.
    Mfa(Mfa),
}

/// A pending two-factor login.
#[derive(Clone, Debug, Deserialize)]
pub struct Mfa {
    ticket: String,
    #[serde(default)]
    login_instance_id: Option<String>,
    #[serde(default)]
    pub totp: bool,
    #[serde(default)]
    pub sms: bool,
    #[serde(default)]
    pub backup: bool,
}

impl Mfa {
    /// The method to offer first, or `None` when the account only has
    /// security keys, which need WebAuthn.
    pub fn preferred(&self) -> Option<MfaMethod> {
        self.methods().first().copied()
    }

    pub fn methods(&self) -> Vec<MfaMethod> {
        [(self.totp, MfaMethod::Totp), (self.sms, MfaMethod::Sms), (self.backup, MfaMethod::Backup)]
            .into_iter()
            .filter_map(|(on, method)| on.then_some(method))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MfaMethod {
    /// A code from an authenticator app.
    Totp,
    /// A code texted to the account's phone, after [`Rest::send_mfa_sms`].
    Sms,
    /// One of the account's single-use backup codes.
    Backup,
}

impl MfaMethod {
    fn path(self) -> &'static str {
        match self {
            MfaMethod::Totp => "totp",
            MfaMethod::Sms => "sms",
            MfaMethod::Backup => "backup",
        }
    }
}

impl Rest {
    /// `login` is an email address or an E.164 phone number.
    pub async fn login(&self, login: &str, password: &str) -> Result<Login, Error> {
        let body = json!({ "login": login, "password": password, "undelete": false });
        let response = self.auth("/auth/login", body).await?;
        if let Some(token) = response["token"].as_str() {
            return Ok(Login::Token(token.to_owned()));
        }
        if response["mfa"].as_bool() == Some(true) {
            return Ok(Login::Mfa(Mfa::deserialize(&response)?));
        }
        Err(Error::Protocol("login answered with neither a token nor two-factor".into()))
    }

    /// Text a code to the account's phone. Returns the number, redacted.
    pub async fn send_mfa_sms(&self, mfa: &Mfa) -> Result<String, Error> {
        let response = self.auth("/auth/mfa/sms/send", json!({ "ticket": mfa.ticket })).await?;
        Ok(response["phone"].as_str().unwrap_or_default().to_owned())
    }

    pub async fn verify_mfa(&self, mfa: &Mfa, method: MfaMethod, code: &str) -> Result<String, Error> {
        let mut body = json!({ "ticket": mfa.ticket, "code": code });
        if let Some(id) = &mfa.login_instance_id {
            body["login_instance_id"] = id.as_str().into();
        }
        let response = self.auth(&format!("/auth/mfa/{}", method.path()), body).await?;
        token(&response)
    }

    /// Trade a QR login ticket for the token, encrypted to the session's key.
    pub(crate) async fn remote_auth_login(&self, ticket: &str) -> Result<String, Error> {
        let response = self.auth("/users/@me/remote-auth/login", json!({ "ticket": ticket })).await?;
        response["encrypted_token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("no encrypted token in the QR login answer".into()))
    }

    async fn auth(&self, path: &str, body: Value) -> Result<Value, Error> {
        let (status, response) = self.send("POST", path, Some(body)).await?;
        let value = serde_json::from_slice(&response).unwrap_or(Value::Null);
        match status {
            200..=299 => Ok(value),
            _ => Err(refusal(&value)
                .unwrap_or_else(|| Error::Http { status, body: String::from_utf8_lossy(&response).into() })),
        }
    }
}

fn token(response: &Value) -> Result<String, Error> {
    response["token"].as_str().map(str::to_owned).ok_or_else(|| Error::Protocol("no token in the answer".into()))
}

/// Why Discord turned an auth request down, in its own words where it has
/// some. Form errors nest by field: `{"errors": {"login": {"_errors":
/// [{"message": …}]}}}`; the first one found is the one to show.
fn refusal(body: &Value) -> Option<Error> {
    if crate::rest::is_captcha(body) {
        return Some(Error::Captcha);
    }
    if body.get("suspended_user_token").is_some() {
        return Some(Error::Rejected("Discord has suspended this account.".into()));
    }
    fn first_form_error(errors: &Value) -> Option<&str> {
        if let Some(message) = errors["_errors"][0]["message"].as_str() {
            return Some(message);
        }
        errors.as_object()?.values().find_map(first_form_error)
    }
    let message = first_form_error(&body["errors"]).or(body["message"].as_str())?;
    Some(Error::Rejected(message.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(body: &str) -> String {
        refusal(&serde_json::from_str(body).unwrap()).unwrap().to_string()
    }

    #[test]
    fn form_errors_win_over_the_summary() {
        let body = r#"{"code":50035,"message":"Invalid Form Body","errors":{"login":{"_errors":
            [{"code":"INVALID_LOGIN","message":"Login or password is invalid."}]}}}"#;
        assert_eq!(refused(body), "Login or password is invalid.");
    }

    #[test]
    fn plain_messages_and_captchas() {
        assert_eq!(refused(r#"{"code":60008,"message":"Invalid two-factor code"}"#), "Invalid two-factor code");
        assert!(matches!(
            refusal(&serde_json::json!({"captcha_key": ["captcha-required"], "captcha_service": "hcaptcha"})),
            Some(Error::Captcha)
        ));
        assert!(refusal(&Value::Null).is_none());
    }

    #[test]
    fn mfa_methods_in_preference_order() {
        let mfa: Mfa = serde_json::from_str(r#"{"ticket":"t","mfa":true,"sms":true,"backup":true}"#).unwrap();
        assert_eq!(mfa.methods(), [MfaMethod::Sms, MfaMethod::Backup]);
        let keys_only: Mfa = serde_json::from_str(r#"{"ticket":"t","mfa":true,"webauthn":"{}"}"#).unwrap();
        assert_eq!(keys_only.preferred(), None);
    }
}
