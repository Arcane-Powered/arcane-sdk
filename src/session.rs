//! Game session tracking: playtime, and FPS sampled in short windows.
//!
//! [`crate::ArcaneClient::init`] opens a session for the whole run of the game.
//! One background thread named `arcane-session` owns it: it wakes about once a
//! minute, sends a heartbeat carrying the cumulative playtime, and goes back to
//! sleep on a condition variable. Nothing else in the SDK polls or spins.
//!
//! FPS is sampled, never counted continuously. While the player has performance
//! sharing enabled in the Arcane desktop app, the thread opens a 30-second
//! window at T+60 s and then every 5 minutes. During a window
//! [`crate::ArcaneClient::frame`] is a relaxed atomic increment; outside one it
//! is a single relaxed load. A window with no frames produces no sample.
//!
//! A session that never reaches the desktop app stays `Pending` and keeps
//! retrying. It surfaces no error of its own: the last one is kept in the
//! session state, where `{client:?}` shows it.
//!
//! The same thread carries lobby event polling, but only once the game has
//! called [`crate::ArcaneClient::p2p`]: it then asks the desktop app for events
//! on every tick, every 5 seconds while a lobby is open. Heartbeats keep their
//! own 60-second schedule either way. See [`LobbyPollingState`].
//!
//! Playtime is measured with [`Instant`], so it is immune to wall-clock changes.
//! If the desktop app is never reachable, the session's playtime is lost — the
//! SDK keeps no buffer on disk.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::desktop::{post_json, DesktopCall, GAMES_PATH_PREFIX};
use crate::device::now_unix;
use crate::error::SdkError;
use crate::p2p::{poll_once, LobbyPollingState, P2pState};

const TICK: Duration = Duration::from_secs(60);
const FIRST_WINDOW_AT: Duration = Duration::from_secs(60);
const WINDOW_PERIOD: Duration = Duration::from_secs(300);
const WINDOW_LEN: Duration = Duration::from_secs(30);
const MIN_WAIT: Duration = Duration::from_millis(50);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const END_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_SAMPLES: usize = 16;
const THREAD_STACK_BYTES: usize = 64 * 1024;

pub(crate) const SESSION_TICK_ENV: &str = "ARCANE_SESSION_TICK_MS";

fn tick_period() -> Duration {
    std::env::var(SESSION_TICK_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(|ms| Duration::from_millis(ms).max(MIN_WAIT))
        .unwrap_or(TICK)
}

/// Whether the SDK is tracking this play session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingState {
    /// The Arcane desktop app acknowledged the session. Playtime and FPS samples
    /// are being reported.
    Active,
    /// A local session is open and playtime is accumulating, but the desktop app
    /// has not acknowledged it yet. The SDK retries every 60 seconds.
    Pending,
    /// Nothing is tracked: `ARCANE_OFFLINE_ONLY` is set, or DRM is disabled for
    /// the title and no Arcane account is known, so there is nobody to attribute
    /// playtime to.
    Disabled,
}

impl TrackingState {
    /// Stable wire string: `"active"`, `"pending"`, `"disabled"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Disabled => "disabled",
        }
    }
}

impl fmt::Display for TrackingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the session looks like right now — for logs, overlays and QA screens.
///
/// Read it with [`crate::ArcaneClient::session`]. It is a copy: nothing here
/// keeps the session alive or blocks the background thread.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// Session id issued by the Arcane desktop app, once it acknowledged the
    /// session.
    pub session_id: Option<String>,
    /// Whether the session is tracked, waiting for the desktop app, or off.
    pub tracking: TrackingState,
    /// Playtime since `init`, in seconds, measured with a monotonic clock.
    pub played_seconds: u64,
    /// Whether the player has performance sharing enabled in Arcane desktop.
    /// FPS windows only open while this is `true`.
    pub fps_sampling: bool,
    /// How many FPS samples this session has produced.
    pub samples_taken: u32,
    /// Average frame rate of the most recent sample.
    pub last_fps_avg: Option<f32>,
    /// Whether the session thread is polling Arcane for lobby events, and why
    /// not when it is not.
    pub lobby_events: LobbyPollingState,
}

impl SessionSnapshot {
    pub(crate) fn to_json(&self) -> String {
        json!({
            "session_id": self.session_id,
            "tracking": self.tracking.as_str(),
            "played_seconds": self.played_seconds,
            "fps_sampling": self.fps_sampling,
            "samples_taken": self.samples_taken,
            "last_fps_avg": self.last_fps_avg,
            "lobby_events": self.lobby_events.as_str(),
        })
        .to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
struct FpsSample {
    sample_id: String,
    taken_at: i64,
    fps_avg: f32,
    window_seconds: u32,
    frames: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    graphics_preset: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    session_id: String,
    #[serde(default)]
    fps_sampling: bool,
}

#[derive(Debug, Deserialize)]
struct HeartbeatResponse {
    #[serde(default)]
    fps_sampling: bool,
}

#[derive(Debug)]
struct State {
    stop: bool,
    tracking: TrackingState,
    session_id: Option<String>,
    fps_sampling: bool,
    samples_taken: u32,
    last_fps_avg: Option<f32>,
    pending: Vec<FpsSample>,
    resolution: Option<String>,
    graphics_preset: Option<String>,
    next_window_at: Duration,
    window_open_at: Option<Duration>,
    last_error: Option<SdkError>,
}

#[derive(Debug)]
pub(crate) struct SessionInner {
    public_key: String,
    window_open: AtomicBool,
    frames: AtomicU64,
    started: Instant,
    state: Mutex<State>,
    wake: Condvar,
    p2p: Arc<P2pState>,
}

impl SessionInner {
    fn new(public_key: &str, p2p: Arc<P2pState>) -> Self {
        Self {
            public_key: public_key.to_string(),
            p2p,
            window_open: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            started: Instant::now(),
            state: Mutex::new(State {
                stop: false,
                tracking: TrackingState::Disabled,
                session_id: None,
                fps_sampling: false,
                samples_taken: 0,
                last_fps_avg: None,
                pending: Vec::new(),
                resolution: None,
                graphics_preset: None,
                next_window_at: FIRST_WINDOW_AT,
                window_open_at: None,
                last_error: None,
            }),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn frame(&self) {
        if self.window_open.load(Ordering::Relaxed) {
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn set_graphics(&self, resolution: &str, preset: &str) {
        let mut state = self.lock();
        state.resolution = non_empty(resolution);
        state.graphics_preset = non_empty(preset);
    }

    fn seconds_at(&self, elapsed: Duration) -> u64 {
        elapsed.as_secs()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let polling = self.p2p.polling();
        let state = self.lock();
        // No session thread runs while tracking is off, so nothing polls
        // whatever the game armed.
        let lobby_events = match state.tracking {
            TrackingState::Disabled => LobbyPollingState::Off,
            _ => polling,
        };
        let played_seconds = match state.tracking {
            TrackingState::Disabled => 0,
            _ => self.seconds_at(self.started.elapsed()),
        };
        SessionSnapshot {
            session_id: state.session_id.clone(),
            tracking: state.tracking,
            played_seconds,
            fps_sampling: state.fps_sampling,
            samples_taken: state.samples_taken,
            last_fps_avg: state.last_fps_avg,
            lobby_events,
        }
    }

    fn poll_window(&self, elapsed: Duration) {
        let mut state = self.lock();
        if !state.fps_sampling {
            self.close_window(&mut state, None);
            return;
        }
        match state.window_open_at {
            Some(open_at) => {
                if elapsed >= open_at + WINDOW_LEN {
                    self.close_window(&mut state, Some(elapsed));
                }
            }
            None => {
                if elapsed >= state.next_window_at {
                    state.window_open_at = Some(elapsed);
                    self.frames.store(0, Ordering::Relaxed);
                    self.window_open.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    fn close_window(&self, state: &mut State, closed_at: Option<Duration>) {
        let Some(open_at) = state.window_open_at.take() else {
            return;
        };
        self.window_open.store(false, Ordering::Relaxed);
        let frames = self.frames.swap(0, Ordering::Relaxed);
        state.next_window_at = open_at + WINDOW_PERIOD;

        let Some(closed_at) = closed_at else {
            return;
        };
        let window_seconds = closed_at.saturating_sub(open_at).as_secs_f64().round() as u32;
        if frames == 0 || window_seconds == 0 {
            return;
        }

        let sample = FpsSample {
            sample_id: uuid::Uuid::new_v4().to_string(),
            taken_at: now_unix(),
            fps_avg: frames as f32 / window_seconds as f32,
            window_seconds,
            frames,
            resolution: state.resolution.clone(),
            graphics_preset: state.graphics_preset.clone(),
        };
        state.last_fps_avg = Some(sample.fps_avg);
        state.samples_taken = state.samples_taken.saturating_add(1);
        if state.pending.len() >= MAX_PENDING_SAMPLES {
            state.pending.remove(0);
        }
        state.pending.push(sample);
    }

    fn set_fps_sampling(&self, enabled: bool) {
        let mut state = self.lock();
        state.fps_sampling = enabled;
        if !enabled {
            self.close_window(&mut state, None);
        }
    }

    fn next_window_deadline(&self, state: &State) -> Option<Duration> {
        if !state.fps_sampling {
            return None;
        }
        match state.window_open_at {
            Some(open_at) => Some(open_at + WINDOW_LEN),
            None => Some(state.next_window_at),
        }
    }

    fn acknowledge(&self, sent: &[FpsSample]) {
        if sent.is_empty() {
            return;
        }
        let mut state = self.lock();
        state
            .pending
            .retain(|kept| !sent.iter().any(|s| s.sample_id == kept.sample_id));
    }

    fn tick(&self) {
        let (session_id, pending) = {
            let state = self.lock();
            (state.session_id.clone(), state.pending.clone())
        };

        let Some(session_id) = session_id else {
            self.try_start();
            return;
        };

        let seconds = self.seconds_at(self.started.elapsed());
        match self.post_heartbeat(&session_id, seconds, &pending) {
            Ok(response) => {
                self.acknowledge(&pending);
                self.set_fps_sampling(response.fps_sampling);
                let mut state = self.lock();
                state.tracking = TrackingState::Active;
                state.last_error = None;
            }
            Err(call) => {
                let expired = call.desktop_error() == Some("unknown_session");
                self.record_error(call);
                if expired {
                    {
                        let mut state = self.lock();
                        state.session_id = None;
                        state.tracking = TrackingState::Pending;
                    }
                    self.try_start();
                }
            }
        }
    }

    fn try_start(&self) {
        let response = match self.post_start() {
            Ok(response) => response,
            Err(call) => {
                self.record_error(call);
                return;
            }
        };
        {
            let mut state = self.lock();
            state.session_id = Some(response.session_id);
            state.tracking = TrackingState::Active;
            state.last_error = None;
        }
        self.set_fps_sampling(response.fps_sampling);
    }

    fn poll_lobby_events(&self) {
        if let Some(err) = poll_once(&self.public_key, &self.p2p) {
            self.lock().last_error = Some(err);
        }
    }

    fn record_error(&self, call: DesktopCall) {
        let error = call.into_sdk_error();
        self.lock().last_error = Some(error);
    }

    fn post_start(&self) -> Result<StartResponse, DesktopCall> {
        post_json(&self.path("start"), None, CALL_TIMEOUT)
    }

    fn post_heartbeat(
        &self,
        session_id: &str,
        seconds: u64,
        samples: &[FpsSample],
    ) -> Result<HeartbeatResponse, DesktopCall> {
        let body = json!({
            "session_id": session_id,
            "seconds": seconds,
            "samples": samples,
        });
        post_json(&self.path("heartbeat"), Some(body), CALL_TIMEOUT)
    }

    fn post_end(&self, session_id: &str, seconds: u64, samples: &[FpsSample]) {
        let body = json!({
            "session_id": session_id,
            "seconds": seconds,
            "samples": samples,
        });
        let _: Result<serde_json::Value, DesktopCall> =
            post_json(&self.path("end"), Some(body), END_TIMEOUT);
    }

    fn path(&self, action: &str) -> String {
        format!(
            "{GAMES_PATH_PREFIX}/{}/session/{action}",
            self.public_key.as_str()
        )
    }

    fn end(&self) {
        let (session_id, pending) = {
            let mut state = self.lock();
            (state.session_id.take(), std::mem::take(&mut state.pending))
        };
        let Some(session_id) = session_id else {
            return;
        };
        let seconds = self.seconds_at(self.started.elapsed());
        self.post_end(&session_id, seconds, &pending);
    }

    pub(crate) fn wake_now(&self) {
        let _state = self.lock();
        self.wake.notify_all();
    }

    fn request_stop(&self) {
        {
            let mut state = self.lock();
            state.stop = true;
        }
        self.wake.notify_all();
    }

    fn stopped(&self) -> bool {
        self.lock().stop
    }

    fn wait(&self, timeout: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .wake
            .wait_timeout(state, timeout)
            .unwrap_or_else(|e| e.into_inner());
        state.stop
    }
}

fn run(inner: Arc<SessionInner>) {
    let tick = tick_period();
    let mut next_tick = Duration::ZERO;
    let mut next_poll = Duration::ZERO;
    loop {
        if inner.stopped() {
            return;
        }

        inner.poll_window(inner.started.elapsed());

        if inner.started.elapsed() >= next_tick {
            inner.tick();
            next_tick = inner.started.elapsed() + tick;
        }

        if inner.p2p.armed() && inner.started.elapsed() >= next_poll {
            inner.poll_lobby_events();
            next_poll = inner.started.elapsed() + inner.p2p.poll_period(tick);
        }

        if inner.stopped() {
            return;
        }

        let elapsed = inner.started.elapsed();
        let deadline = {
            let state = inner.lock();
            let mut deadline = inner
                .next_window_deadline(&state)
                .map_or(next_tick, |window| window.min(next_tick));
            if inner.p2p.armed() {
                deadline = deadline.min(next_poll);
            }
            deadline
        };
        if inner.wait(deadline.saturating_sub(elapsed).max(MIN_WAIT)) {
            return;
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug)]
pub(crate) struct Session {
    inner: Arc<SessionInner>,
    ended: AtomicBool,
}

impl Session {
    pub(crate) fn dormant(public_key: &str, p2p: Arc<P2pState>) -> Self {
        Self {
            inner: Arc::new(SessionInner::new(public_key, p2p)),
            ended: AtomicBool::new(false),
        }
    }

    pub(crate) fn begin(&self, tracking: TrackingState) {
        if tracking == TrackingState::Disabled {
            return;
        }
        self.inner.lock().tracking = tracking;
        self.inner.p2p.set_waker(Arc::downgrade(&self.inner));

        let worker = Arc::clone(&self.inner);
        let spawned = thread::Builder::new()
            .name("arcane-session".to_string())
            .stack_size(THREAD_STACK_BYTES)
            .spawn(move || run(worker));

        if spawned.is_err() {
            self.inner.lock().tracking = TrackingState::Disabled;
        }
    }

    pub(crate) fn frame(&self) {
        self.inner.frame();
    }

    pub(crate) fn set_graphics(&self, resolution: &str, preset: &str) {
        self.inner.set_graphics(resolution, preset);
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        self.inner.snapshot()
    }

    pub(crate) fn end(&self) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.request_stop();
        self.inner.end();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionInner {
        SessionInner::new("pk_test_title", Arc::new(P2pState::new()))
    }

    fn sampling_session() -> SessionInner {
        let inner = session();
        inner.set_fps_sampling(true);
        inner
    }

    fn render(inner: &SessionInner, frames: u64) {
        for _ in 0..frames {
            inner.frame();
        }
    }

    #[test]
    fn a_window_produces_one_sample_averaged_over_its_length() {
        let inner = sampling_session();

        inner.poll_window(FIRST_WINDOW_AT);
        assert!(inner.window_open.load(Ordering::Relaxed));
        render(&inner, 1794);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);

        let state = inner.lock();
        assert_eq!(state.pending.len(), 1);
        let sample = &state.pending[0];
        assert_eq!(sample.frames, 1794);
        assert_eq!(sample.window_seconds, 30);
        assert!((sample.fps_avg - 59.8).abs() < 0.01);
        assert_eq!(state.samples_taken, 1);
        assert_eq!(state.last_fps_avg, Some(sample.fps_avg));
        assert!(!inner.window_open.load(Ordering::Relaxed));
    }

    #[test]
    fn the_next_window_opens_five_minutes_after_the_last_one() {
        let inner = sampling_session();
        assert_eq!(inner.lock().next_window_at, FIRST_WINDOW_AT);

        inner.poll_window(FIRST_WINDOW_AT);
        render(&inner, 60);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);
        assert_eq!(inner.lock().next_window_at, FIRST_WINDOW_AT + WINDOW_PERIOD);

        inner.poll_window(FIRST_WINDOW_AT + WINDOW_PERIOD - Duration::from_secs(1));
        assert!(!inner.window_open.load(Ordering::Relaxed));

        inner.poll_window(FIRST_WINDOW_AT + WINDOW_PERIOD);
        assert!(inner.window_open.load(Ordering::Relaxed));
    }

    #[test]
    fn a_window_without_frames_produces_no_sample() {
        let inner = sampling_session();

        inner.poll_window(FIRST_WINDOW_AT);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);

        let state = inner.lock();
        assert!(state.pending.is_empty());
        assert_eq!(state.samples_taken, 0);
        assert_eq!(state.last_fps_avg, None);
    }

    #[test]
    fn no_window_opens_and_frames_are_not_counted_while_sampling_is_off() {
        let inner = session();

        inner.poll_window(FIRST_WINDOW_AT);
        assert!(!inner.window_open.load(Ordering::Relaxed));
        render(&inner, 500);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);

        assert_eq!(inner.frames.load(Ordering::Relaxed), 0);
        assert!(inner.lock().pending.is_empty());
    }

    #[test]
    fn turning_sampling_off_closes_the_open_window_without_a_sample() {
        let inner = sampling_session();

        inner.poll_window(FIRST_WINDOW_AT);
        render(&inner, 900);
        inner.set_fps_sampling(false);

        assert!(!inner.window_open.load(Ordering::Relaxed));
        let state = inner.lock();
        assert!(state.pending.is_empty());
        assert_eq!(state.samples_taken, 0);
        assert!(state.window_open_at.is_none());
    }

    #[test]
    fn an_unacknowledged_sample_is_resent_with_the_same_id() {
        let inner = sampling_session();
        inner.poll_window(FIRST_WINDOW_AT);
        render(&inner, 1500);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);

        let first: Vec<FpsSample> = inner.lock().pending.clone();
        let retry: Vec<FpsSample> = inner.lock().pending.clone();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sample_id, retry[0].sample_id);

        inner.acknowledge(&retry);
        assert!(inner.lock().pending.is_empty());
    }

    #[test]
    fn acknowledging_one_sample_keeps_the_ones_that_were_not_sent() {
        let inner = sampling_session();
        inner.poll_window(FIRST_WINDOW_AT);
        render(&inner, 60);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);
        let sent: Vec<FpsSample> = inner.lock().pending.clone();

        inner.poll_window(FIRST_WINDOW_AT + WINDOW_PERIOD);
        render(&inner, 30);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_PERIOD + WINDOW_LEN);
        inner.acknowledge(&sent);

        let state = inner.lock();
        assert_eq!(state.pending.len(), 1);
        assert_ne!(state.pending[0].sample_id, sent[0].sample_id);
        assert_eq!(state.samples_taken, 2);
    }

    #[test]
    fn the_tick_period_defaults_to_sixty_seconds() {
        assert_eq!(tick_period(), TICK);
        assert_eq!(TICK, Duration::from_secs(60));
        assert_eq!(FIRST_WINDOW_AT, Duration::from_secs(60));
        assert_eq!(WINDOW_PERIOD, Duration::from_secs(300));
        assert_eq!(WINDOW_LEN, Duration::from_secs(30));
    }

    #[test]
    fn cumulative_seconds_come_from_the_monotonic_clock() {
        let inner = session();
        let seconds: Vec<u64> = [0, 59, 60, 90, 3600]
            .iter()
            .map(|s| inner.seconds_at(Duration::from_secs(*s)))
            .collect();

        assert_eq!(seconds, vec![0, 59, 60, 90, 3600]);
        assert!(seconds.windows(2).all(|pair| pair[1] >= pair[0]));
    }

    #[test]
    fn a_sample_carries_the_current_graphics_settings() {
        let inner = sampling_session();
        inner.set_graphics("2560x1440", "high");

        inner.poll_window(FIRST_WINDOW_AT);
        render(&inner, 120);
        inner.poll_window(FIRST_WINDOW_AT + WINDOW_LEN);

        let state = inner.lock();
        assert_eq!(state.pending[0].resolution.as_deref(), Some("2560x1440"));
        assert_eq!(state.pending[0].graphics_preset.as_deref(), Some("high"));
    }

    #[test]
    fn the_snapshot_carries_the_lobby_polling_state() {
        let p2p = Arc::new(P2pState::new());
        let inner = SessionInner::new("pk_test_title", Arc::clone(&p2p));
        inner.lock().tracking = TrackingState::Pending;
        assert_eq!(inner.snapshot().lobby_events, LobbyPollingState::Off);

        crate::p2p::P2p::new("pk_test_title", &p2p);
        assert_eq!(inner.snapshot().lobby_events, LobbyPollingState::Active);
    }

    #[test]
    fn a_disabled_session_polls_nothing_however_armed_it_is() {
        let p2p = Arc::new(P2pState::new());
        let inner = SessionInner::new("pk_test_title", Arc::clone(&p2p));
        crate::p2p::P2p::new("pk_test_title", &p2p);

        assert_eq!(inner.lock().tracking, TrackingState::Disabled);
        assert_eq!(
            inner.snapshot().lobby_events,
            LobbyPollingState::Off,
            "no session thread runs, so nothing is polling"
        );
    }

    #[test]
    fn a_disabled_session_reports_no_playtime() {
        let inner = session();
        let snapshot = inner.snapshot();

        assert_eq!(snapshot.tracking, TrackingState::Disabled);
        assert_eq!(snapshot.played_seconds, 0);
        assert_eq!(snapshot.session_id, None);
        assert!(!snapshot.fps_sampling);
    }

    #[test]
    fn the_snapshot_json_carries_every_field() {
        let inner = session();
        inner.lock().tracking = TrackingState::Pending;
        let parsed: serde_json::Value =
            serde_json::from_str(&inner.snapshot().to_json()).expect("json");

        assert_eq!(parsed["tracking"], "pending");
        assert_eq!(parsed["session_id"], serde_json::Value::Null);
        assert_eq!(parsed["samples_taken"], 0);
        assert_eq!(parsed["fps_sampling"], false);
        assert_eq!(parsed["last_fps_avg"], serde_json::Value::Null);
        assert_eq!(parsed["lobby_events"], "off");
        assert!(parsed["played_seconds"].is_u64());
    }
}
