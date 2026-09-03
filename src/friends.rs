//! Friends: who this player has on Arcane, who is online, and who is in this
//! game right now.
//!
//! One call, [`Friends::list`], and it is **synchronous on the calling thread**
//! — a single local loopback round trip, on the order of a millisecond. Call it
//! when a menu opens or on a timer of your own, never from the render loop.
//!
//! The SDK keeps no list of its own: the Arcane desktop app caches for 15
//! seconds and reports [`FriendList::stale`] when the answer came from that
//! cache while it was offline. Friend requests, chat and the overlay stay in the
//! launcher — this is a read of presence, nothing else.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::desktop::{get_json, offline_only, DesktopCall, OFFLINE_ONLY_ENV};
use crate::error::SdkError;

const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const FRIENDS_PATH: &str = "/v1/friends";

/// One friend of the signed-in Arcane account, with their presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    /// The friend's Arcane account id — the value Arcane identifies them by.
    pub user_id: String,
    /// Their display name on Arcane.
    pub pseudo: String,
    /// Whether they are signed in to Arcane right now.
    pub online: bool,
    /// Whether they are playing **this** title right now. Always `false` when
    /// the SDK does not know this title's `game_id`.
    pub in_game: bool,
}

/// What the Arcane desktop app knows about this player's friends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendList {
    /// The friends, in the order the desktop app returned them.
    pub friends: Vec<Friend>,
    /// Whether the desktop app answered from its cache because it is offline.
    /// The list is still usable — presence may simply be a few minutes old.
    pub stale: bool,
}

#[derive(Debug, Deserialize)]
struct WireFriendList {
    #[serde(default)]
    friends: Vec<WireFriend>,
    #[serde(default)]
    stale: bool,
}

#[derive(Debug, Deserialize)]
struct WireFriend {
    user_id: String,
    #[serde(default)]
    pseudo: String,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    playing_game_id: Option<String>,
}

/// The friends accessor, borrowed from the client for one call.
///
/// Build it with [`crate::ArcaneClient::friends`]; it holds no state of its own,
/// so keeping one around buys nothing.
#[derive(Debug)]
pub struct Friends<'a> {
    game_id: Option<&'a str>,
}

impl<'a> Friends<'a> {
    pub(crate) fn new(game_id: Option<&'a str>) -> Self {
        Self { game_id }
    }

    /// This player's friends, each with `online` and `in_game`.
    ///
    /// One synchronous loopback round trip. The Arcane desktop app caches the
    /// list for 15 seconds and sets [`FriendList::stale`] when it served an
    /// older copy while offline, so calling this on a menu or a timer of a few
    /// seconds is fine — calling it per frame is not.
    ///
    /// # Errors
    ///
    /// `not_authenticated` when nobody is signed in, `network_required` under
    /// `ARCANE_OFFLINE_ONLY` (raised before any call), `arcane_unavailable` when
    /// the desktop app is not running, `feature_unavailable` when it predates
    /// the route.
    pub fn list(&self) -> Result<FriendList, SdkError> {
        self.guard_offline()?;

        let response: WireFriendList =
            get_json(FRIENDS_PATH, CALL_TIMEOUT).map_err(DesktopCall::into_sdk_error)?;

        Ok(map_list(response, self.game_id))
    }

    fn guard_offline(&self) -> Result<(), SdkError> {
        if !offline_only() {
            return Ok(());
        }
        Err(SdkError::network_required(
            "Listing friends needs the Arcane desktop app, and the SDK is running in \
             offline-only mode.",
        )
        .with_hint(format!(
            "Unset {OFFLINE_ONLY_ENV} to let the SDK contact the Arcane desktop app."
        ))
        .with_context("env", OFFLINE_ONLY_ENV))
    }
}

fn map_list(wire: WireFriendList, game_id: Option<&str>) -> FriendList {
    FriendList {
        friends: wire
            .friends
            .into_iter()
            .map(|friend| Friend {
                user_id: friend.user_id,
                pseudo: friend.pseudo,
                online: friend.online,
                in_game: is_in_game(friend.playing_game_id.as_deref(), game_id),
            })
            .collect(),
        stale: wire.stale,
    }
}

/// A friend is in this game when the title they are playing is this title. With
/// no `game_id` the SDK cannot tell, and says `false` rather than guessing.
fn is_in_game(playing_game_id: Option<&str>, game_id: Option<&str>) -> bool {
    match (playing_game_id, game_id) {
        (Some(playing), Some(game_id)) => !game_id.is_empty() && playing == game_id,
        _ => false,
    }
}

pub(crate) fn to_json(list: &FriendList) -> String {
    let friends: Vec<serde_json::Value> = list
        .friends
        .iter()
        .map(|friend| {
            json!({
                "user_id": friend.user_id,
                "pseudo": friend.pseudo,
                "online": friend.online,
                "in_game": friend.in_game,
            })
        })
        .collect();
    json!({ "friends": friends, "stale": list.stale }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE: &str = r#"{
        "friends": [
            {"user_id":"user-a","pseudo":"Ada","online":true,"playing_game_id":"game-canonical-id"},
            {"user_id":"user-b","pseudo":"Bo","online":true,"playing_game_id":"another-game"},
            {"user_id":"user-c","pseudo":"Cy","online":false,"playing_game_id":null}
        ],
        "stale": false
    }"#;

    fn parse(raw: &str) -> WireFriendList {
        serde_json::from_str(raw).expect("wire friend list")
    }

    #[test]
    fn the_wire_shape_maps_onto_the_public_struct() {
        let list = map_list(parse(WIRE), Some("game-canonical-id"));

        assert_eq!(list.friends.len(), 3);
        assert!(!list.stale);

        assert_eq!(list.friends[0].user_id, "user-a");
        assert_eq!(list.friends[0].pseudo, "Ada");
        assert!(list.friends[0].online);
        assert!(list.friends[0].in_game);

        assert!(list.friends[1].online);
        assert!(!list.friends[1].in_game, "another title is not this one");

        assert!(!list.friends[2].online);
        assert!(!list.friends[2].in_game);
    }

    #[test]
    fn without_a_game_id_nobody_is_in_game() {
        let list = map_list(parse(WIRE), None);

        assert!(list.friends.iter().all(|friend| !friend.in_game));
        assert!(list.friends[0].online, "presence still comes through");
    }

    #[test]
    fn stale_passes_through() {
        let list = map_list(parse(r#"{"friends":[],"stale":true}"#), Some("game"));

        assert!(list.stale);
        assert!(list.friends.is_empty());
    }

    #[test]
    fn a_missing_body_field_reads_as_absent_rather_than_failing() {
        let list = map_list(parse(r#"{"friends":[{"user_id":"user-a"}]}"#), Some("game"));

        assert!(!list.stale);
        assert_eq!(list.friends[0].pseudo, "");
        assert!(!list.friends[0].online);
        assert!(!list.friends[0].in_game);
    }

    #[test]
    fn an_empty_game_id_never_matches() {
        assert!(!is_in_game(Some(""), Some("")));
        assert!(!is_in_game(None, Some("game")));
        assert!(!is_in_game(Some("game"), None));
        assert!(is_in_game(Some("game"), Some("game")));
    }

    #[test]
    fn the_json_rendering_carries_every_field() {
        let rendered = to_json(&map_list(parse(WIRE), Some("game-canonical-id")));
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("json");

        assert_eq!(parsed["stale"], false);
        assert_eq!(parsed["friends"][0]["user_id"], "user-a");
        assert_eq!(parsed["friends"][0]["pseudo"], "Ada");
        assert_eq!(parsed["friends"][0]["online"], true);
        assert_eq!(parsed["friends"][0]["in_game"], true);
        assert_eq!(parsed["friends"][1]["in_game"], false);
    }
}
