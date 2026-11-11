//! Logging in with a password, and the two-factor step that may follow.
//!
//! Discord answers a suspicious login with a captcha instead; see
//! [`Rest::solving_captchas`].

use serde::Deserialize;
use serde_json::{Value, json};

use crate::endpoints::{Call, call};
use crate::{Error, Rest};

/// ApiErrorCode values the client's login tells apart (652215).
const ACCOUNT_SCHEDULED_FOR_DELETION: u64 = 20011;
const PHONE_VERIFICATION_REQUIRED: u64 = 70007;

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

    /// In the order the client's login store lists them (`ea` in 46653),
    /// after the security keys it puts first.
    pub fn methods(&self) -> Vec<MfaMethod> {
        [(self.totp, MfaMethod::Totp), (self.backup, MfaMethod::Backup), (self.sms, MfaMethod::Sms)]
            .into_iter()
            .filter_map(|(on, method)| on.then_some(method))
            .collect()
    }
}

/// What the email from [`Rest::forgot_password`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    PasswordReset,
    OneTimeLogin,
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
    /// `login` is an email address or an E.164 phone number. `undelete`
    /// cancels the account's scheduled deletion, which the client only
    /// sends after asking ([`Error::ScheduledForDeletion`]).
    pub async fn login(&self, login: &str, password: &str, undelete: bool) -> Result<Login, Error> {
        let response = self.auth(call!(LOGIN), login_body(login, password, undelete)).await?;
        // Sent here even when a second factor follows; that answer has none.
        let required: Vec<String> = serde_json::from_value(response["required_actions"].clone()).unwrap_or_default();
        check_required(&required)?;
        if let Some(token) = response["token"].as_str() {
            return Ok(Login::Token(token.to_owned()));
        }
        if response["mfa"].as_bool() == Some(true) {
            return Ok(Login::Mfa(Mfa::deserialize(&response)?));
        }
        Err(Error::Protocol("login answered with neither a token nor two-factor".into()))
    }

    /// Have Discord email a link for a new password, or for a one-time
    /// login when the account has no password (`forgotPassword` in 830215).
    pub async fn forgot_password(&self, login: &str) -> Result<Recovery, Error> {
        let response = self.auth(call!(FORGOT_PASSWORD), json!({ "login": login })).await?;
        Ok(match response["method"].as_str() {
            Some("one_time_login") => Recovery::OneTimeLogin,
            _ => Recovery::PasswordReset,
        })
    }

    /// Text a code to the account's phone. Returns the number, redacted.
    pub async fn send_mfa_sms(&self, mfa: &Mfa) -> Result<String, Error> {
        let response = self.auth(call!(SEND_MFA_SMS), json!({ "ticket": mfa.ticket })).await?;
        Ok(response["phone"].as_str().unwrap_or_default().to_owned())
    }

    pub async fn verify_mfa(&self, mfa: &Mfa, method: MfaMethod, code: &str) -> Result<String, Error> {
        let response = self.auth(call!(VERIFY_MFA, method = method.path()), mfa_body(mfa, code)).await?;
        token(&response)
    }

    /// Trade a QR login ticket for the token, encrypted to the session's key.
    pub(crate) async fn remote_auth_login(&self, ticket: &str) -> Result<String, Error> {
        let response = self.auth(call!(REMOTE_AUTH_LOGIN), json!({ "ticket": ticket })).await?;
        response["encrypted_token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("no encrypted token in the QR login answer".into()))
    }

    async fn auth(&self, call: Call, body: Value) -> Result<Value, Error> {
        let (status, response) = self.send(call, Some(body)).await?;
        let value = serde_json::from_slice(&response).unwrap_or(Value::Null);
        match status {
            200..=299 => Ok(value),
            _ => Err(refusal(&value)
                .unwrap_or_else(|| Error::Http { status, body: String::from_utf8_lossy(&response).into() })),
        }
    }
}

/// The client's login form sends `login_source` and `gift_code_sku_id` too
/// (`login` and `loginMFAv2` in 830215). They name the invite, gift or page
/// the login started from, and are `null` for a login that started on the
/// plain login page, which is the only one Neucord has.
fn login_body(login: &str, password: &str, undelete: bool) -> Value {
    json!({
        "login": login,
        "password": password,
        "undelete": undelete,
        "login_source": null,
        "gift_code_sku_id": null,
    })
}

fn mfa_body(mfa: &Mfa, code: &str) -> Value {
    let mut body = json!({ "ticket": mfa.ticket, "code": code, "login_source": null, "gift_code_sku_id": null });
    if let Some(id) = &mfa.login_instance_id {
        body["login_instance_id"] = id.as_str().into();
    }
    body
}

/// Turn down a login that came with steps Neucord can't take. Discord sends
/// the token all the same, but the official client won't let the account in
/// until they're done: closing its forced password change logs out.
fn check_required(actions: &[String]) -> Result<(), Error> {
    if actions.iter().any(|a| a == "update_password") {
        return Err(Error::Rejected(
            "Discord requires a new password for this account. Change it in the official app \
             or at discord.com, then log in here with the new one."
                .into(),
        ));
    }
    Ok(())
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
    if let Some(token) = body["suspended_user_token"].as_str() {
        return Some(Error::Suspended(token.to_owned()));
    }
    match body["code"].as_u64() {
        Some(ACCOUNT_SCHEDULED_FOR_DELETION) => return Some(Error::ScheduledForDeletion),
        Some(PHONE_VERIFICATION_REQUIRED) => {
            return Some(Error::Rejected(
                "Discord wants to verify a phone number first, which Neucord can't do. \
                 Do it at discord.com, then try again."
                    .into(),
            ));
        }
        _ => {}
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
    fn accounts_the_client_stops_at() {
        let suspended = serde_json::json!({"code": 40002, "message": "x", "suspended_user_token": "tok"});
        assert!(matches!(refusal(&suspended), Some(Error::Suspended(t)) if t == "tok"));
        let deleting = serde_json::json!({"code": 20011, "message": "This account is scheduled for deletion."});
        assert!(matches!(refusal(&deleting), Some(Error::ScheduledForDeletion)));
        let phone = serde_json::json!({"code": 70007, "message": "x"});
        assert!(matches!(refusal(&phone), Some(Error::Rejected(why)) if why.contains("phone")));
        // Disabled accounts get Discord's own words.
        let disabled = serde_json::json!({"code": 20013, "message": "This account is disabled."});
        assert_eq!(refusal(&disabled).unwrap().to_string(), "This account is disabled.");
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
    fn logins_carry_the_fields_the_client_sends() {
        let mfa: Mfa =
            serde_json::from_str(r#"{"ticket":"t","mfa":true,"totp":true,"login_instance_id":"i"}"#).unwrap();
        assert_eq!(
            login_body("a@b.c", "pw", false),
            json!({"login": "a@b.c", "password": "pw", "undelete": false, "login_source": null, "gift_code_sku_id": null})
        );
        assert_eq!(
            mfa_body(&mfa, "123456"),
            json!({"ticket": "t", "code": "123456", "login_source": null, "gift_code_sku_id": null, "login_instance_id": "i"})
        );
        let without_instance: Mfa = serde_json::from_str(r#"{"ticket":"t","mfa":true,"totp":true}"#).unwrap();
        assert!(mfa_body(&without_instance, "1").get("login_instance_id").is_none());
    }

    #[test]
    fn forced_password_change() {
        assert!(check_required(&["update_password".into()]).is_err());
        assert!(check_required(&[]).is_ok());
    }

    #[test]
    fn mfa_methods_in_preference_order() {
        let mfa: Mfa = serde_json::from_str(r#"{"ticket":"t","mfa":true,"sms":true,"backup":true}"#).unwrap();
        assert_eq!(mfa.methods(), [MfaMethod::Backup, MfaMethod::Sms]);
        // Texting a code is never the first thing offered when a code the user already holds will do.
        let all: Mfa =
            serde_json::from_str(r#"{"ticket":"t","mfa":true,"totp":true,"sms":true,"backup":true}"#).unwrap();
        assert_eq!(all.methods(), [MfaMethod::Totp, MfaMethod::Backup, MfaMethod::Sms]);
        let keys_only: Mfa = serde_json::from_str(r#"{"ticket":"t","mfa":true,"webauthn":"{}"}"#).unwrap();
        assert_eq!(keys_only.preferred(), None);
    }
}
