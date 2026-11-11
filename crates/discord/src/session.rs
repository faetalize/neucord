//! Tokens live in the desktop keyring (Secret Service), never on disk: one
//! entry per account, keyed by its user id, as the client keeps one per user
//! id (247775). Two more entries share the schema: the list of accounts
//! ([`crate::accounts::Roster`], which holds names and avatar hashes and no
//! token) and, from before accounts were keyed, the one token Neucord used
//! to keep.

use std::collections::HashMap;

use libsecret::{Schema, SchemaAttributeType, SchemaFlags};

use crate::accounts::Roster;
use crate::{Error, Snowflake};

const SCHEMA_NAME: &str = "dev.faetalize.neucord.Token";
const ATTRIBUTE: &str = "account";
const ROSTER: &str = "roster";
/// Where the single token of earlier versions is: it moves to its user id's
/// entry once READY says whose it is.
const LEGACY: &str = "default";

fn schema() -> Schema {
    Schema::new(SCHEMA_NAME, SchemaFlags::NONE, HashMap::from([(ATTRIBUTE, SchemaAttributeType::String)]))
}

fn attributes(account: &str) -> HashMap<&str, &str> {
    HashMap::from([(ATTRIBUTE, account)])
}

/// Bridges libsecret's callback API to `async`.
async fn once<T: 'static>(start: impl FnOnce(async_channel::Sender<T>)) -> T {
    let (tx, rx) = async_channel::bounded(1);
    start(tx);
    rx.recv().await.expect("libsecret always invokes its callback")
}

async fn lookup(account: String) -> Result<Option<String>, Error> {
    once(move |tx| {
        libsecret::password_lookup(Some(&schema()), attributes(&account), gio::Cancellable::NONE, move |r| {
            let _ = tx.try_send(r);
        })
    })
    .await
    .map(|secret| secret.map(String::from))
    .map_err(Error::Keyring)
}

async fn store(account: String, secret: &str, label: &str) -> Result<(), Error> {
    let (secret, label) = (secret.to_owned(), label.to_owned());
    once(move |tx| {
        libsecret::password_store(
            Some(&schema()),
            attributes(&account),
            None,
            &label,
            &secret,
            gio::Cancellable::NONE,
            move |r| {
                let _ = tx.try_send(r);
            },
        )
    })
    .await
    .map_err(Error::Keyring)
}

async fn clear(account: String) -> Result<(), Error> {
    once(move |tx| {
        libsecret::password_clear(Some(&schema()), attributes(&account), gio::Cancellable::NONE, move |r| {
            let _ = tx.try_send(r);
        })
    })
    .await
    .map_err(Error::Keyring)
}

pub async fn load_token(account: Snowflake) -> Result<Option<String>, Error> {
    lookup(account.to_string()).await
}

pub async fn store_token(account: Snowflake, token: &str, label: &str) -> Result<(), Error> {
    store(account.to_string(), token, label).await
}

pub async fn clear_token(account: Snowflake) -> Result<(), Error> {
    clear(account.to_string()).await
}

/// The token an earlier version kept, if it is still there.
pub async fn load_legacy_token() -> Result<Option<String>, Error> {
    lookup(LEGACY.into()).await
}

pub async fn clear_legacy_token() -> Result<(), Error> {
    clear(LEGACY.into()).await
}

pub async fn load_roster() -> Result<Roster, Error> {
    Ok(lookup(ROSTER.into()).await?.map(|json| Roster::from_json(&json)).unwrap_or_default())
}

pub async fn store_roster(roster: &Roster) -> Result<(), Error> {
    store(ROSTER.into(), &roster.to_json(), "Neucord accounts").await
}
