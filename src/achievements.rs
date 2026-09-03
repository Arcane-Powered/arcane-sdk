//! Achievements: list what the title defines, unlock one, ask what is unlocked.
//!
//! Both calls go to the Arcane desktop app over the loopback and are
//! **synchronous on the calling thread** — one local round trip, on the order of
//! a millisecond. Call them when the condition becomes true, never from the
//! render loop.
//!
//! [`Achievements::list`] fills a cache held by the client and shared by its
//! clones; [`Achievements::is_unlocked`] answers from that cache without any
//! I/O, and [`Achievements::unlock`] keeps it up to date. Nothing here runs in
//! the background: a game that never calls `achievements()` pays nothing.
//!
//! Unlocking is idempotent — the desktop app and the backend deduplicate — so a
//! game can call [`Achievements::unlock`] every time its condition holds without
//! guarding first. When the desktop app is offline it answers `queued`, which is
//! still a success: the unlock is stored and synchronised later.

use std::sync::RwLock;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::desktop::{
    get_json, offline_only, post_json, DesktopCall, GAMES_PATH_PREFIX, OFFLINE_ONLY_ENV,
};
use crate::device::now_unix;
use crate::error::SdkError;

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest achievement key the SDK will accept, in bytes.
pub const MAX_ACHIEVEMENT_KEY_LEN: usize = 64;

/// One achievement as the Arcane portal defines it, plus this player's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Achievement {
    /// Stable key from the Arcane portal, the value passed to
    /// [`Achievements::unlock`].
    pub key: String,
    /// Display name.
    pub title: String,
    /// Display description. Empty for a hidden achievement the player has not
    /// unlocked yet, if the portal is configured that way.
    pub description: String,
    /// Icon URL, when the title provides one.
    pub icon_url: Option<String>,
    /// Whether the portal marks this achievement as hidden until unlocked.
    pub hidden: bool,
    /// Unix timestamp of the unlock, or `None` while it is still locked.
    pub unlocked_at: Option<i64>,
}

/// The result of an [`Achievements::unlock`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlock {
    /// The achievement key that was unlocked.
    pub key: String,
    /// Unix timestamp Arcane recorded for the unlock. For an
    /// `already_unlocked` answer this is the original unlock time.
    pub unlocked_at: i64,
    /// Whether the player already had it — a repeated call is not an error.
    pub already_unlocked: bool,
    /// Whether the desktop app was offline and stored the unlock for later.
    /// Still a success: it is synchronised when the app reconnects.
    pub queued: bool,
}

/// Reject a malformed achievement key before any network call, so a typo
/// surfaces as `invalid_argument` instead of a `unknown_achievement` round trip.
///
/// The charset is the one that makes it safe to interpolate the key straight
/// into the loopback URL. `.` is allowed inside a key but a key made only of
/// dots is not: it would be a relative path segment, which an HTTP client
/// normalises away into a different route.
pub(crate) fn validate_key(key: &str) -> Result<(), SdkError> {
    if key.is_empty() {
        return Err(SdkError::invalid_argument("The achievement key is empty.")
            .with_hint("Pass the achievement key defined for this title in the Arcane portal."));
    }
    if key.len() > MAX_ACHIEVEMENT_KEY_LEN {
        return Err(
            SdkError::invalid_argument("The achievement key is too long.")
                .with_hint(
                    "Pass the achievement key from the Arcane portal, not its display title.",
                )
                .with_context("length", key.len())
                .with_context("max_length", MAX_ACHIEVEMENT_KEY_LEN),
        );
    }
    if let Some((index, bad)) = key
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        return Err(SdkError::invalid_argument(
            "The achievement key contains a character that is not allowed.",
        )
        .with_hint(
            "Allowed characters are ASCII letters, digits, and `_`, `-`, `.`. \
             Check for stray whitespace or quotes around the value.",
        )
        .with_context("index", index)
        .with_context("character", format!("{bad:?}")));
    }
    if key.bytes().all(|byte| byte == b'.') {
        return Err(SdkError::invalid_argument(
            "The achievement key is only dots, which is a relative path segment.",
        )
        .with_hint("Pass the achievement key from the Arcane portal, not `.` or `..`.")
        .with_context("key", key));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub(crate) struct AchievementCache {
    entries: RwLock<Option<Vec<Achievement>>>,
}

impl AchievementCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn store(&self, achievements: &[Achievement]) {
        let mut slot = self.entries.write().unwrap_or_else(|e| e.into_inner());
        *slot = Some(achievements.to_vec());
    }

    fn record(&self, key: &str, unlocked_at: i64) {
        let mut slot = self.entries.write().unwrap_or_else(|e| e.into_inner());
        let Some(entries) = slot.as_mut() else {
            return;
        };
        match entries.iter_mut().find(|entry| entry.key == key) {
            Some(entry) => {
                entry.unlocked_at.get_or_insert(unlocked_at);
            }
            None => entries.push(Achievement {
                key: key.to_string(),
                title: String::new(),
                description: String::new(),
                icon_url: None,
                hidden: false,
                unlocked_at: Some(unlocked_at),
            }),
        }
    }

    fn is_unlocked(&self, key: &str) -> Option<bool> {
        let slot = self.entries.read().unwrap_or_else(|e| e.into_inner());
        slot.as_ref()?
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.unlocked_at.is_some())
    }
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    achievements: Vec<WireAchievement>,
}

#[derive(Debug, Deserialize)]
struct WireAchievement {
    key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    unlocked_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireUnlock {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    unlocked_at: Option<String>,
    #[serde(default)]
    already_unlocked: bool,
    #[serde(default)]
    queued: bool,
}

impl From<WireAchievement> for Achievement {
    fn from(wire: WireAchievement) -> Self {
        Self {
            key: wire.key,
            title: wire.title,
            description: wire.description,
            icon_url: wire.icon_url,
            hidden: wire.hidden,
            unlocked_at: wire.unlocked_at.as_deref().and_then(parse_rfc3339),
        }
    }
}

/// The achievements accessor, borrowed from the client for one or two calls.
///
/// Build it with [`crate::ArcaneClient::achievements`]; it holds no state of its
/// own, so keeping one around buys nothing.
#[derive(Debug)]
pub struct Achievements<'a> {
    public_key: &'a str,
    cache: &'a AchievementCache,
}

impl<'a> Achievements<'a> {
    pub(crate) fn new(public_key: &'a str, cache: &'a AchievementCache) -> Self {
        Self { public_key, cache }
    }

    /// Every achievement this title defines, with this player's unlock state.
    ///
    /// One synchronous loopback round trip. It also fills the client's cache, so
    /// [`Achievements::is_unlocked`] can answer from memory afterwards — call it
    /// once, at launch or on the achievements screen, not per frame.
    ///
    /// # Errors
    ///
    /// `not_owned`, `not_authenticated`, `network_required` (including under
    /// `ARCANE_OFFLINE_ONLY`), `arcane_unavailable`, `feature_unavailable` when
    /// the Arcane desktop app predates the route.
    pub fn list(&self) -> Result<Vec<Achievement>, SdkError> {
        self.guard_offline("List achievements")?;

        let response: ListResponse = get_json(&self.list_path(), CALL_TIMEOUT)
            .map_err(|call| self.call_error(call, None))?;
        let achievements: Vec<Achievement> = response
            .achievements
            .into_iter()
            .map(Achievement::from)
            .collect();

        self.cache.store(&achievements);
        Ok(achievements)
    }

    /// Unlock `key` for the signed-in player.
    ///
    /// Idempotent: unlocking twice succeeds and returns `already_unlocked`, so a
    /// game can call this every time its condition holds. When the desktop app
    /// is offline the answer is `queued` — also a success — and the cache is
    /// updated either way.
    ///
    /// One synchronous loopback round trip, on the calling thread. Never call it
    /// from the render loop.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when `key` is empty, longer than
    /// [`MAX_ACHIEVEMENT_KEY_LEN`] or outside `A–Z a–z 0–9 _ - .` (raised before
    /// any network call), `unknown_achievement` when the title does not define
    /// it, plus the same codes as [`Achievements::list`].
    pub fn unlock(&self, key: &str) -> Result<Unlock, SdkError> {
        validate_key(key)?;
        self.guard_offline("Unlocking an achievement")?;

        let response: WireUnlock = post_json(&self.unlock_path(key), None, CALL_TIMEOUT)
            .map_err(|call| self.call_error(call, Some(key)))?;

        let unlock = Unlock {
            key: response.key.unwrap_or_else(|| key.to_string()),
            unlocked_at: response
                .unlocked_at
                .as_deref()
                .and_then(parse_rfc3339)
                .unwrap_or_else(now_unix),
            already_unlocked: response.already_unlocked,
            queued: response.queued,
        };

        self.cache.record(&unlock.key, unlock.unlocked_at);
        Ok(unlock)
    }

    /// Whether `key` is unlocked, from the cache [`Achievements::list`] filled.
    ///
    /// Reads memory only — no I/O, no failure, safe to call often. `None` means
    /// the SDK has nothing to answer with: `list` has never succeeded, or the
    /// key was not among the achievements it returned.
    pub fn is_unlocked(&self, key: &str) -> Option<bool> {
        self.cache.is_unlocked(key)
    }

    fn list_path(&self) -> String {
        format!("{GAMES_PATH_PREFIX}/{}/achievements", self.public_key)
    }

    fn unlock_path(&self, key: &str) -> String {
        format!(
            "{GAMES_PATH_PREFIX}/{}/achievements/{key}/unlock",
            self.public_key
        )
    }

    fn guard_offline(&self, action: &str) -> Result<(), SdkError> {
        if !offline_only() {
            return Ok(());
        }
        Err(SdkError::network_required(format!(
            "{action} needs the Arcane desktop app, and the SDK is running in offline-only mode."
        ))
        .with_hint(format!(
            "Unset {OFFLINE_ONLY_ENV} to let the SDK contact the Arcane desktop app."
        ))
        .with_context("env", OFFLINE_ONLY_ENV))
    }

    fn call_error(&self, call: DesktopCall, key: Option<&str>) -> SdkError {
        let error = call.into_sdk_error();
        match key {
            Some(key) => error.with_context("achievement_key", key),
            None => error,
        }
    }
}

pub(crate) fn to_json(achievements: &[Achievement]) -> String {
    let entries: Vec<serde_json::Value> = achievements
        .iter()
        .map(|entry| {
            json!({
                "key": entry.key,
                "title": entry.title,
                "description": entry.description,
                "icon_url": entry.icon_url,
                "hidden": entry.hidden,
                "unlocked_at": entry.unlocked_at,
            })
        })
        .collect();
    json!({ "achievements": entries }).to_string()
}

/// Turn the RFC 3339 timestamp the desktop app sends into a unix timestamp.
///
/// Deliberately small: the loopback only ever sends timestamps Arcane itself
/// produced, and the SDK has no date-time dependency. Anything it cannot read
/// becomes `None`, which reads as "still locked" rather than a wrong date.
fn parse_rfc3339(value: &str) -> Option<i64> {
    let raw = value.trim();
    if raw.len() < 19 || !raw.is_ascii() {
        return None;
    }
    let (date, rest) = raw.split_at(10);
    let (year, month, day) = {
        let mut parts = date.splitn(3, '-');
        (
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
        )
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let (time, rest) = rest.split_at(8);
    let mut parts = time.splitn(3, ':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let rest = match rest.strip_prefix('.') {
        Some(fraction) => fraction.trim_start_matches(|c: char| c.is_ascii_digit()),
        None => rest,
    };
    let offset = parse_offset(rest)?;

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// Seconds to subtract from a local timestamp to reach UTC.
fn parse_offset(raw: &str) -> Option<i64> {
    if raw.is_empty() || raw == "Z" || raw == "z" {
        return Some(0);
    }
    let sign = match raw.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hours, minutes) = raw[1..].split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours = hours.parse::<i64>().ok()?;
    let minutes = minutes.parse::<i64>().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// Days between 1970-01-01 and a proleptic Gregorian date, by Howard Hinnant's
/// `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn achievement(key: &str, unlocked_at: Option<i64>) -> Achievement {
        Achievement {
            key: key.to_string(),
            title: "First blood".into(),
            description: "Win a duel.".into(),
            icon_url: None,
            hidden: false,
            unlocked_at,
        }
    }

    #[test]
    fn accepts_portal_shaped_keys() {
        for key in ["first_blood", "boss.01", "level-3", "A", "0"] {
            assert!(validate_key(key).is_ok(), "rejected {key}");
        }
    }

    #[test]
    fn rejects_an_empty_key() {
        let err = validate_key("").unwrap_err();
        assert_eq!(err.code(), "invalid_argument");
        assert!(err.hint().is_some());
    }

    #[test]
    fn rejects_an_oversized_key() {
        let err = validate_key(&"a".repeat(MAX_ACHIEVEMENT_KEY_LEN + 1)).unwrap_err();
        assert_eq!(err.code(), "invalid_argument");
        assert!(err
            .context()
            .iter()
            .any(|(k, v)| k == "length" && v == &(MAX_ACHIEVEMENT_KEY_LEN + 1).to_string()));
        assert!(validate_key(&"a".repeat(MAX_ACHIEVEMENT_KEY_LEN)).is_ok());
    }

    #[test]
    fn rejects_characters_that_would_escape_the_url() {
        for key in [
            "first blood",
            "first/../blood",
            "first?x=1",
            "first#frag",
            "first\n",
            " first",
            "prémier",
            ".",
            "..",
            "...",
        ] {
            let err = validate_key(key).unwrap_err();
            assert_eq!(err.code(), "invalid_argument", "accepted {key:?}");
        }
    }

    #[test]
    fn reports_where_the_bad_character_is() {
        let err = validate_key("first/blood").unwrap_err();
        let context: Vec<_> = err
            .context()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(context.contains(&("index", "5")));
        assert!(context.contains(&("character", "'/'")));
    }

    #[test]
    fn an_empty_cache_answers_nothing() {
        let cache = AchievementCache::new();
        assert_eq!(cache.is_unlocked("first_blood"), None);
    }

    #[test]
    fn a_stored_list_answers_locked_and_unlocked() {
        let cache = AchievementCache::new();
        cache.store(&[
            achievement("first_blood", Some(1_786_480_000)),
            achievement("boss.01", None),
        ]);

        assert_eq!(cache.is_unlocked("first_blood"), Some(true));
        assert_eq!(cache.is_unlocked("boss.01"), Some(false));
        assert_eq!(cache.is_unlocked("never_defined"), None);
    }

    #[test]
    fn an_unlock_updates_a_loaded_cache_and_keeps_the_first_timestamp() {
        let cache = AchievementCache::new();
        cache.store(&[achievement("boss.01", None)]);

        cache.record("boss.01", 1_786_480_000);
        assert_eq!(cache.is_unlocked("boss.01"), Some(true));

        cache.record("boss.01", 1_786_490_000);
        let stored = cache.entries.read().unwrap();
        assert_eq!(stored.as_ref().unwrap()[0].unlocked_at, Some(1_786_480_000));
    }

    #[test]
    fn an_unlock_of_a_key_the_list_did_not_carry_is_still_recorded() {
        let cache = AchievementCache::new();
        cache.store(&[achievement("boss.01", None)]);

        cache.record("secret", 1_786_480_000);

        assert_eq!(cache.is_unlocked("secret"), Some(true));
    }

    #[test]
    fn an_unlock_before_any_list_leaves_the_cache_empty() {
        let cache = AchievementCache::new();
        cache.record("first_blood", 1_786_480_000);
        assert_eq!(cache.is_unlocked("first_blood"), None);
    }

    #[test]
    fn rfc3339_timestamps_become_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-05-01T12:34:56Z"), Some(1_777_638_896));
        assert_eq!(
            parse_rfc3339("2026-05-01T12:34:56.789Z"),
            Some(1_777_638_896)
        );
        assert_eq!(
            parse_rfc3339("2026-05-01T14:34:56+02:00"),
            Some(1_777_638_896)
        );
        assert_eq!(
            parse_rfc3339("2026-05-01T10:34:56-02:00"),
            Some(1_777_638_896)
        );
        assert_eq!(
            parse_rfc3339("  2026-05-01t12:34:56z  "),
            Some(1_777_638_896)
        );
    }

    #[test]
    fn a_leap_day_lands_on_the_right_second() {
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    #[test]
    fn an_unreadable_timestamp_is_none_rather_than_a_wrong_date() {
        for raw in [
            "",
            "2026-05-01",
            "not-a-date",
            "2026-13-01T00:00:00Z",
            "2026-05-01T25:00:00Z",
            "2026-05-01T00:00:00+2:00",
            "2026-05-01 00:00:00 CET",
        ] {
            assert_eq!(parse_rfc3339(raw), None, "parsed {raw:?}");
        }
    }

    #[test]
    fn the_wire_shape_maps_onto_the_public_struct() {
        let wire: WireAchievement = serde_json::from_str(
            r#"{"key":"first_blood","title":"First blood","description":"Win a duel.",
                "icon_url":"https://cdn/first.png","hidden":false,
                "unlocked_at":"2026-05-01T12:34:56Z"}"#,
        )
        .expect("wire achievement");
        let parsed = Achievement::from(wire);

        assert_eq!(parsed.key, "first_blood");
        assert_eq!(parsed.icon_url.as_deref(), Some("https://cdn/first.png"));
        assert_eq!(parsed.unlocked_at, Some(1_777_638_896));
    }

    #[test]
    fn a_locked_achievement_has_no_timestamp() {
        let wire: WireAchievement =
            serde_json::from_str(r#"{"key":"boss.01","unlocked_at":null}"#).expect("wire");
        let parsed = Achievement::from(wire);

        assert_eq!(parsed.unlocked_at, None);
        assert_eq!(parsed.title, "");
        assert!(!parsed.hidden);
    }

    #[test]
    fn the_json_rendering_carries_every_field() {
        let rendered = to_json(&[achievement("first_blood", Some(1_786_480_000))]);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("json");

        assert_eq!(parsed["achievements"][0]["key"], "first_blood");
        assert_eq!(parsed["achievements"][0]["title"], "First blood");
        assert_eq!(parsed["achievements"][0]["unlocked_at"], 1_786_480_000);
        assert_eq!(
            parsed["achievements"][0]["icon_url"],
            serde_json::Value::Null
        );
        assert_eq!(parsed["achievements"][0]["hidden"], false);
    }
}
