//! What the gateway is told of whether the user is active: the `qos` every
//! heartbeat carries (op 40) and IDENTIFY's `qos_token`, as the client's
//! `GatewaySocket` (868) keeps them from `SessionHeartbeatScheduler` (903063).
//!
//! The socket keeps two payloads, the one the next heartbeat sends and the
//! state since. Going active is folded into the payload due at once, so a
//! heartbeat reports activity anywhere in its interval; the heartbeat after
//! it reports the state that followed.

use serde::Serialize;

use crate::protobuf::{self, Message, Value, Wire};

/// `ver` in every payload, the client's `client_heartbeat_version`.
const VERSION: u32 = 31;

/// What a heartbeat says of the user (`K` in 903063).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Payload {
    active: bool,
    ver: u32,
    reasons: Vec<&'static str>,
}

impl Payload {
    /// With a token and the window in front: "foregrounded". The client also
    /// needs to be on one of its app's pages, which Neucord always is, and
    /// adds "rtc_connected" in a call, which Neucord has none of.
    pub fn now(signed_in: bool, focused: bool) -> Self {
        let reasons = if signed_in && focused { vec!["foregrounded"] } else { Vec::new() };
        Payload { active: !reasons.is_empty(), ver: VERSION, reasons }
    }
}

/// The socket's `heartbeatQOSState`: the payload the next heartbeat sends,
/// and the state since.
#[derive(Debug, Default)]
pub struct State {
    current: Option<Payload>,
    upcoming: Option<Payload>,
}

impl State {
    /// The user's state is now `now` (`handleActiveStateChange`). Whether a
    /// heartbeat goes out at once, as it does once the session is
    /// established: on going active from a heartbeat that wasn't.
    pub fn changed(&mut self, now: Payload) -> bool {
        let was_inactive = !self.current.as_ref().is_some_and(|c| c.active);
        let current = self.current.get_or_insert_with(|| now.clone());
        let beat = now.active && was_inactive;
        if now.active {
            current.active = true;
            current.reasons.extend(&now.reasons);
            current.reasons.sort_unstable();
            current.reasons.dedup();
        }
        self.upcoming = Some(now);
        beat
    }

    /// What this heartbeat sends; the next sends the state since
    /// (`_consumeQOSPayload`).
    pub fn consume(&mut self) -> Option<Payload> {
        let sent = self.current.take();
        self.current = self.upcoming.take().or_else(|| sent.clone());
        sent
    }

    /// Whether IDENTIFY says the user is active (`getIsUserActive`).
    pub fn user_active(&self) -> bool {
        self.upcoming.as_ref().or(self.current.as_ref()).is_some_and(|p| p.active)
    }
}

/// IDENTIFY's `qos_token` (`_doIdentify`): base64 of a `QosToken` holding
/// whether the user is `active` and, as it came, the `DerivedQosData` the
/// last READY gave the account (`DerivedQosDataStore`, 193799). One that
/// can't be read is left out, as the client leaves it.
pub fn token(active: bool, derived: Option<&str>) -> String {
    let client_provided = protobuf::build(&[(1, Value::Varint(1))]);
    let client_provided = if active { client_provided } else { Vec::new() };
    let mut fields = vec![(1, Value::Bytes(client_provided))];
    if let Some(derived) = derived.and_then(read_derived) {
        fields.push((2, Value::Bytes(derived)));
    }
    openssl::base64::encode_block(&protobuf::build(&fields))
}

/// `DerivedQosData`, decoded from its base64 and written back as protobuf-ts
/// writes it: `claims`, `signature` and `key_id` where set, then any field it
/// doesn't know, as read.
fn read_derived(base64: &str) -> Option<Vec<u8>> {
    // The client's decoder takes the URL-safe alphabet too (65412).
    let standard: String = base64
        .chars()
        .map(|c| {
            if c == '-' {
                '+'
            } else if c == '_' {
                '/'
            } else {
                c
            }
        })
        .collect();
    let bytes = openssl::base64::decode_block(&standard).ok()?;
    let message = Message::parse(&bytes)?;
    let (mut claims, mut signature, mut key_id) = (None, None, 0u64);
    let mut unknown = Vec::new();
    for &(number, wire) in message.fields() {
        match (number, wire) {
            (1, Wire::Bytes(b)) => claims = Some(b),
            (2, Wire::Bytes(b)) => signature = Some(b),
            (3, Wire::Varint(v)) => key_id = v & u64::from(u32::MAX),
            (1..=3, _) => return None,
            _ => unknown.push((number, wire)),
        }
    }
    let mut known = Vec::new();
    for (number, bytes) in [(1, claims), (2, signature)] {
        if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
            known.push((number, Wire::Bytes(bytes)));
        }
    }
    if key_id != 0 {
        known.push((3, Wire::Varint(key_id)));
    }
    known.extend(unknown);
    Some(protobuf::write(&known))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Payload {
        Payload::now(true, true)
    }

    fn inactive() -> Payload {
        Payload::now(true, false)
    }

    #[test]
    fn payloads_as_the_client_sends_them() {
        let json = |p: Payload| serde_json::to_string(&p).unwrap();
        assert_eq!(json(active()), r#"{"active":true,"ver":31,"reasons":["foregrounded"]}"#);
        assert_eq!(json(inactive()), r#"{"active":false,"ver":31,"reasons":[]}"#);
        assert_eq!(Payload::now(false, true), inactive(), "nobody signed in");
    }

    #[test]
    fn a_heartbeat_reports_activity_anywhere_in_its_interval() {
        let mut state = State::default();
        assert_eq!(state.consume(), None, "nothing known yet");
        assert!(!state.user_active());

        assert!(state.changed(active()), "going active beats at once");
        assert!(state.user_active());
        assert!(!state.changed(active()), "staying active doesn't");
        assert_eq!(state.consume(), Some(active()));

        assert!(!state.changed(inactive()));
        assert!(!state.user_active(), "IDENTIFY goes by the state now");
        assert_eq!(state.consume(), Some(active()), "the heartbeat due was active");
        assert_eq!(state.consume(), Some(inactive()), "the next says what followed");
        assert_eq!(state.consume(), Some(inactive()), "and so on, until it changes");

        assert!(!state.changed(inactive()));
        assert!(state.changed(active()), "active again, from an inactive heartbeat");
        assert!(!state.changed(inactive()), "back out before the heartbeat");
        assert_eq!(state.consume(), Some(active()), "still reports it");
        assert_eq!(state.consume(), Some(inactive()));
    }

    #[test]
    fn the_first_state_is_the_one_due() {
        let mut state = State::default();
        assert!(!state.changed(inactive()));
        assert_eq!(state.consume(), Some(inactive()));
        let mut state = State::default();
        assert!(state.changed(active()), "the first may beat at once");
        assert_eq!(state.consume(), Some(active()));
    }

    #[test]
    fn tokens_as_the_client_writes_them() {
        let decode = |t: String| openssl::base64::decode_block(&t).unwrap();
        assert_eq!(decode(token(false, None)), [0x0a, 0x00], "an empty ClientProvidedQosData");
        assert_eq!(decode(token(true, None)), [0x0a, 0x02, 0x08, 0x01]);
        assert_eq!(token(true, None), "CgIIAQ==", "standard base64, padded");

        // Made-up claims and signature, key 7, and a field 9 nothing names.
        let derived = [0x0a, 0x02, 0xaa, 0xbb, 0x12, 0x01, 0xcc, 0x18, 0x07, 0x48, 0x05];
        let encoded = openssl::base64::encode_block(&derived);
        let mut expected = vec![0x0a, 0x02, 0x08, 0x01, 0x12, derived.len() as u8];
        expected.extend(derived);
        assert_eq!(decode(token(true, Some(&encoded))), expected, "the derived data as it came");

        let reordered = [0x48, 0x05, 0x18, 0x07, 0x12, 0x01, 0xcc, 0x0a, 0x02, 0xaa, 0xbb];
        let written = decode(token(false, Some(&openssl::base64::encode_block(&reordered))));
        assert_eq!(written[4..], derived, "written in field order, the unknown last");

        let claims = [0x0a, 0x03, 0xfb, 0xff, 0xbf];
        let url_safe = openssl::base64::encode_block(&claims).replace('+', "-").replace('/', "_");
        assert!(url_safe.contains('_'));
        assert_eq!(read_derived(&url_safe), Some(claims.to_vec()), "the URL-safe alphabet is read too");
        assert_eq!(read_derived(&openssl::base64::encode_block(&[0x18, 0x00])), Some(Vec::new()), "defaults leave");
        assert_eq!(decode(token(false, Some("not base64!"))), [0x0a, 0x00], "unreadable is left out");
        assert_eq!(decode(token(false, Some(&openssl::base64::encode_block(&[0x18, 0x00])))), [0x0a, 0x00, 0x12, 0x00]);
        let claims_as_a_number = openssl::base64::encode_block(&[0x08, 0x01]);
        assert_eq!(decode(token(false, Some(&claims_as_a_number))), [0x0a, 0x00], "a known field of the wrong type");
        assert_eq!(decode(token(false, Some(&openssl::base64::encode_block(&[0x0a])))), [0x0a, 0x00], "cut short");
    }
}
