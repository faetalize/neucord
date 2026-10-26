//! The token lives in the desktop keyring (Secret Service), never on disk.

use std::collections::HashMap;

use libsecret::{Schema, SchemaAttributeType, SchemaFlags};

use crate::Error;

const SCHEMA_NAME: &str = "dev.faetalize.neucord.Token";
const ATTRIBUTE: &str = "account";
/// One account for now; multi-account will key on the user id.
const ACCOUNT: &str = "default";

fn schema() -> Schema {
    Schema::new(SCHEMA_NAME, SchemaFlags::NONE, HashMap::from([(ATTRIBUTE, SchemaAttributeType::String)]))
}

fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([(ATTRIBUTE, ACCOUNT)])
}

/// Bridges libsecret's callback API to `async`.
async fn once<T: 'static>(start: impl FnOnce(async_channel::Sender<T>)) -> T {
    let (tx, rx) = async_channel::bounded(1);
    start(tx);
    rx.recv().await.expect("libsecret always invokes its callback")
}

pub async fn load_token() -> Result<Option<String>, Error> {
    once(|tx| {
        libsecret::password_lookup(Some(&schema()), attributes(), gio::Cancellable::NONE, move |r| {
            let _ = tx.try_send(r);
        })
    })
    .await
    .map(|token| token.map(String::from))
    .map_err(Error::Keyring)
}

pub async fn store_token(token: &str, label: &str) -> Result<(), Error> {
    let (token, label) = (token.to_owned(), label.to_owned());
    once(move |tx| {
        libsecret::password_store(
            Some(&schema()),
            attributes(),
            None,
            &label,
            &token,
            gio::Cancellable::NONE,
            move |r| {
                let _ = tx.try_send(r);
            },
        )
    })
    .await
    .map_err(Error::Keyring)
}

pub async fn clear_token() -> Result<(), Error> {
    once(|tx| {
        libsecret::password_clear(Some(&schema()), attributes(), gio::Cancellable::NONE, move |r| {
            let _ = tx.try_send(r);
        })
    })
    .await
    .map_err(Error::Keyring)
}
