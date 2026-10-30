//! Coop network: reliable and unreliable messages between the members of a
//! lobby, with no port to open.
//!
//! The Arcane desktop app runs the WebRTC links — direct first, through a TURN
//! relay when a direct path cannot be found — and hands the game two loopback
//! sockets: TCP for the reliable channel, UDP for the unreliable one. The game
//! never sees an address, an ICE candidate or a relay credential.
//!
//! The topology is a star around the lobby host: a member talks to the host,
//! the host talks to everyone. Peers are numbered by the desktop app; the
//! number arrives with [`NetEvent::PeerConnected`] and stays the same for the
//! whole session.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::desktop::{offline_only, post_json, DesktopCall, GAMES_PATH_PREFIX};
use crate::error::SdkError;
use crate::p2p::validate_id;

/// Largest message on [`Channel::Unreliable`], in bytes. Bigger ones are
/// refused before they leave the game.
pub const MAX_UNRELIABLE_MESSAGE: usize = 1200;
/// Largest message on [`Channel::Reliable`], in bytes.
pub const MAX_RELIABLE_MESSAGE: usize = 65535;

const FRAME_VERSION: u8 = 1;
const HEADER_LEN: usize = 8;
const KIND_HELLO: u8 = 1;
const KIND_SEND: u8 = 2;
const KIND_MESSAGE: u8 = 16;
const KIND_PEER_CONNECTED: u8 = 17;
const KIND_QUALITY: u8 = 18;
const KIND_PEER_DISCONNECTED: u8 = 19;
const KIND_RELAY_REFUSED: u8 = 20;
const PEER_HOST: u16 = 0xFFFE;
const PEER_ALL: u16 = 0xFFFF;

const SESSION_KEY_LEN: usize = 16;
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const UDP_HELLO_PERIOD: Duration = Duration::from_secs(2);
const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;

/// Which of the two channels a message travels on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Ordered and retransmitted until delivered. For state that must arrive:
    /// chat, inventory, match results.
    Reliable,
    /// Unordered and never retransmitted: a lost message stays lost. For state
    /// that the next one replaces: positions, inputs.
    Unreliable,
}

impl Channel {
    fn limit(self) -> usize {
        match self {
            Self::Reliable => MAX_RELIABLE_MESSAGE,
            Self::Unreliable => MAX_UNRELIABLE_MESSAGE,
        }
    }
}

/// Who a message is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Peer {
    /// The lobby host. What a member almost always sends to.
    Host,
    /// Every connected peer. Only the host can broadcast; from a member it
    /// reaches the host alone.
    All,
    /// One peer, by the number [`NetEvent::PeerConnected`] gave it. A member
    /// can only reach the host this way.
    Index(u16),
}

impl Peer {
    fn wire(self) -> u16 {
        match self {
            Self::Host => PEER_HOST,
            Self::All => PEER_ALL,
            Self::Index(index) => index,
        }
    }
}

/// How a link reaches the other player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Straight between the two machines.
    Direct,
    /// Through Arcane's TURN relay, because no direct path was found.
    Relayed,
}

impl Route {
    /// Stable wire string: `direct` or `relayed`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relayed => "relayed",
        }
    }
}

/// How to open the coop network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetOptions {
    /// Whether a link may fall back to Arcane's TURN relay when no direct
    /// path exists. `false` keeps every link direct: a pair of players that
    /// cannot connect directly gets [`NetEvent::RelayRefused`] instead.
    pub allow_relay: bool,
}

impl Default for NetOptions {
    fn default() -> Self {
        Self { allow_relay: true }
    }
}

/// One message received from a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetMessage {
    /// The sender's peer number.
    pub peer: u16,
    pub channel: Channel,
    pub data: Vec<u8>,
}

/// What happened to the network since the last [`Net::poll_events`].
#[derive(Debug, Clone, PartialEq)]
pub enum NetEvent {
    /// A link to `user_id` is up. `peer` is the number to send to.
    PeerConnected {
        peer: u16,
        user_id: String,
        route: Route,
        is_host: bool,
    },
    /// The link's measured quality moved: a new route, a round trip that
    /// changed by more than 20 %, or loss crossing 2 % or 10 %.
    Quality {
        peer: u16,
        user_id: String,
        route: Route,
        rtt_ms: Option<f64>,
        loss_pct: f64,
        jitter_ms: Option<f64>,
    },
    /// The link to `user_id` is gone: the player left, the lobby closed, or
    /// every attempt to connect failed.
    PeerDisconnected {
        peer: u16,
        user_id: String,
        reason: String,
    },
    /// No direct path to `user_id`, and the relay was refused by
    /// [`NetOptions::allow_relay`] on one side.
    RelayRefused { peer: u16, user_id: String },
    /// The desktop app closed the network: the play session ended, the lobby
    /// closed, the app quit, or [`Net::close`] was called.
    Closed { reason: String },
}

#[derive(Deserialize)]
struct WireOpened {
    udp_port: u16,
    tcp_port: u16,
    session_key: String,
}

#[derive(Deserialize)]
struct WireOk {
    #[allow(dead_code)]
    ok: bool,
}

#[derive(Deserialize)]
struct WirePeer {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    route: Option<Route>,
    #[serde(default)]
    is_host: bool,
    #[serde(default)]
    rtt_ms: Option<f64>,
    #[serde(default)]
    loss_pct: f64,
    #[serde(default)]
    jitter_ms: Option<f64>,
    #[serde(default)]
    reason: String,
}

#[derive(Default)]
struct Inbox {
    messages: VecDeque<NetMessage>,
    events: VecDeque<NetEvent>,
    queued_bytes: usize,
    closed: bool,
}

struct Link {
    tcp: Mutex<TcpStream>,
    udp: UdpSocket,
    desktop_udp: SocketAddr,
    udp_hello: Vec<u8>,
    inbox: Mutex<Inbox>,
    running: AtomicBool,
}

impl Link {
    fn push_message(&self, message: NetMessage) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if inbox.closed {
            return;
        }
        if inbox.queued_bytes + message.data.len() > MAX_QUEUED_BYTES {
            if message.channel == Channel::Unreliable {
                return;
            }
            drop(inbox);
            self.stop("receive_overflow");
            return;
        }
        inbox.queued_bytes += message.data.len();
        inbox.messages.push_back(message);
    }

    fn push_event(&self, event: NetEvent) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if !inbox.closed {
            inbox.events.push_back(event);
        }
    }

    fn stop(&self, reason: &str) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Ok(tcp) = self.tcp.lock() {
            let _ = tcp.shutdown(Shutdown::Both);
        }
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.events.push_back(NetEvent::Closed {
            reason: reason.to_string(),
        });
        inbox.closed = true;
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

pub(crate) struct NetState {
    link: Mutex<Option<Arc<Link>>>,
}

impl std::fmt::Debug for NetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetState")
            .field(
                "open",
                &self.current().is_some_and(|link| link.is_running()),
            )
            .finish()
    }
}

impl NetState {
    pub(crate) fn new() -> Self {
        Self {
            link: Mutex::new(None),
        }
    }

    fn current(&self) -> Option<Arc<Link>> {
        self.link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .cloned()
    }

    fn replace(&self, link: Option<Arc<Link>>) -> Option<Arc<Link>> {
        let mut slot = self.link.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *slot, link)
    }
}

/// The coop network of this game. Get it from
/// [`ArcaneClient::net`](crate::ArcaneClient::net).
pub struct Net<'a> {
    game_id: &'a str,
    state: &'a NetState,
}

impl<'a> Net<'a> {
    pub(crate) fn new(game_id: &'a str, state: &'a NetState) -> Self {
        Self { game_id, state }
    }

    /// Open the network on a lobby this player already created or joined.
    ///
    /// One synchronous loopback round trip, then two local connections. It
    /// returns once the game is attached to the desktop app; links to the
    /// other members come up afterwards and are announced by
    /// [`NetEvent::PeerConnected`]. Opening again replaces the previous
    /// network.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed lobby id (raised before any call),
    /// `not_in_lobby`, `no_open_session`, `lobby_not_found`, `lobby_closed`,
    /// `network_required` (offline, or under `ARCANE_OFFLINE_ONLY`),
    /// `arcane_unavailable`, `feature_unavailable` when the desktop app
    /// predates the coop network.
    pub fn open(&self, lobby_id: &str, options: NetOptions) -> Result<(), SdkError> {
        validate_id("lobby_id", lobby_id)?;
        if offline_only() {
            return Err(SdkError::network_required(
                "Opening the coop network needs the Arcane desktop app.",
            )
            .with_hint("Unset ARCANE_OFFLINE_ONLY to use the coop network."));
        }

        let body = json!({ "lobby_id": lobby_id, "allow_relay": options.allow_relay });
        let opened: WireOpened = post_json(&self.path("open"), Some(body), OPEN_TIMEOUT)
            .map_err(|call| call.into_sdk_error().with_context("lobby_id", lobby_id))?;

        let link = attach(&opened).map_err(|err| err.with_context("lobby_id", lobby_id))?;
        if let Some(previous) = self.state.replace(Some(Arc::clone(&link))) {
            previous.stop("replaced");
        }
        spawn_readers(link);
        Ok(())
    }

    /// Send one message.
    ///
    /// Writes to a loopback socket and returns; it never waits on the other
    /// player. A message to a peer that is not connected is dropped by the
    /// desktop app without an error.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when `data` is longer than the channel allows
    /// ([`MAX_RELIABLE_MESSAGE`] or [`MAX_UNRELIABLE_MESSAGE`]),
    /// `lobby_closed` when the network is not open,
    /// `arcane_unavailable` when the local socket fails.
    pub fn send(&self, peer: Peer, channel: Channel, data: &[u8]) -> Result<(), SdkError> {
        if data.len() > channel.limit() {
            return Err(
                SdkError::invalid_argument("The message is too large for its channel.")
                    .with_hint(format!(
                        "Reliable messages are at most {MAX_RELIABLE_MESSAGE} bytes, unreliable \
                     ones at most {MAX_UNRELIABLE_MESSAGE}. Split larger state."
                    ))
                    .with_context("bytes", data.len()),
            );
        }
        let link = self.open_link()?;
        let frame = encode(KIND_SEND, peer.wire(), data);
        let sent = match channel {
            Channel::Reliable => {
                let mut tcp = link.tcp.lock().unwrap_or_else(|e| e.into_inner());
                tcp.write_all(&frame)
            }
            Channel::Unreliable => link.udp.send_to(&frame, link.desktop_udp).map(|_| ()),
        };
        sent.map_err(|e| {
            SdkError::arcane_unavailable(format!("The coop network socket failed: {e}"))
                .with_hint("Reopen the network; if it keeps failing, restart Arcane Powered.")
        })
    }

    /// Take the next received message, or `None` when there is none.
    ///
    /// Reads memory only. Drain it every frame: past 64 MiB waiting, new
    /// unreliable messages are dropped and a reliable one closes the network
    /// with the reason `receive_overflow`.
    pub fn receive(&self) -> Option<NetMessage> {
        let link = self.state.current()?;
        let mut inbox = link.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let message = inbox.messages.pop_front()?;
        inbox.queued_bytes -= message.data.len();
        Some(message)
    }

    /// Take every event since the last call. Reads memory only.
    pub fn poll_events(&self) -> Vec<NetEvent> {
        let Some(link) = self.state.current() else {
            return Vec::new();
        };
        let mut inbox = link.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.events.drain(..).collect()
    }

    pub(crate) fn receive_into(&self, buf: &mut [u8]) -> ReceiveInto {
        let Some(link) = self.state.current() else {
            return ReceiveInto::Empty;
        };
        let mut inbox = link.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let Some(front) = inbox.messages.front() else {
            return ReceiveInto::Empty;
        };
        if front.data.len() > buf.len() {
            return ReceiveInto::TooSmall;
        }
        let Some(message) = inbox.messages.pop_front() else {
            return ReceiveInto::Empty;
        };
        inbox.queued_bytes -= message.data.len();
        buf[..message.data.len()].copy_from_slice(&message.data);
        ReceiveInto::Received {
            peer: message.peer,
            channel: message.channel,
            len: message.data.len(),
        }
    }

    pub(crate) fn events_json(&self) -> (String, usize) {
        let Some(link) = self.state.current() else {
            return (render_events(std::iter::empty()), 0);
        };
        let inbox = link.inbox.lock().unwrap_or_else(|e| e.into_inner());
        (render_events(inbox.events.iter()), inbox.events.len())
    }

    pub(crate) fn discard_events(&self, count: usize) {
        if let Some(link) = self.state.current() {
            let mut inbox = link.inbox.lock().unwrap_or_else(|e| e.into_inner());
            let count = count.min(inbox.events.len());
            inbox.events.drain(..count);
        }
    }

    /// Whether the network is open and attached to the desktop app.
    pub fn is_open(&self) -> bool {
        self.state.current().is_some_and(|link| link.is_running())
    }

    /// Close the network: every link drops, for this player only.
    ///
    /// Does nothing when the network was never opened. Otherwise one loopback round
    /// trip of at most 2 seconds, best effort: the local sockets are closed
    /// whatever the desktop app answers. Messages not yet received are
    /// discarded.
    pub fn close(&self) {
        let Some(link) = self.state.replace(None) else {
            return;
        };
        link.stop("closed");
        if !offline_only() {
            let _: Result<WireOk, DesktopCall> =
                post_json(&self.path("close"), None, CLOSE_TIMEOUT);
        }
    }

    fn open_link(&self) -> Result<Arc<Link>, SdkError> {
        match self.state.current() {
            Some(link) if link.is_running() => Ok(link),
            _ => Err(SdkError::lobby_closed("The coop network is not open.")
                .with_hint("Call Net::open on a lobby first; reopen it after NetEvent::Closed.")),
        }
    }

    fn path(&self, action: &str) -> String {
        format!("{GAMES_PATH_PREFIX}/{}/net/{action}", self.game_id)
    }
}

pub(crate) enum ReceiveInto {
    Empty,
    TooSmall,
    Received {
        peer: u16,
        channel: Channel,
        len: usize,
    },
}

pub(crate) fn event_json(event: &NetEvent) -> serde_json::Value {
    match event {
        NetEvent::PeerConnected {
            peer,
            user_id,
            route,
            is_host,
        } => json!({
            "type": "peer_connected",
            "peer": peer,
            "user_id": user_id,
            "route": route.as_str(),
            "is_host": is_host,
        }),
        NetEvent::Quality {
            peer,
            user_id,
            route,
            rtt_ms,
            loss_pct,
            jitter_ms,
        } => json!({
            "type": "quality",
            "peer": peer,
            "user_id": user_id,
            "route": route.as_str(),
            "rtt_ms": rtt_ms,
            "loss_pct": loss_pct,
            "jitter_ms": jitter_ms,
        }),
        NetEvent::PeerDisconnected {
            peer,
            user_id,
            reason,
        } => json!({
            "type": "peer_disconnected",
            "peer": peer,
            "user_id": user_id,
            "reason": reason,
        }),
        NetEvent::RelayRefused { peer, user_id } => json!({
            "type": "relay_refused",
            "peer": peer,
            "user_id": user_id,
        }),
        NetEvent::Closed { reason } => json!({ "type": "closed", "reason": reason }),
    }
}

fn render_events<'a>(events: impl Iterator<Item = &'a NetEvent>) -> String {
    let events: Vec<serde_json::Value> = events.map(event_json).collect();
    json!({ "events": events }).to_string()
}

fn attach(opened: &WireOpened) -> Result<Arc<Link>, SdkError> {
    let key = decode_key(&opened.session_key)?;
    let local_failure = |what: &str, e: std::io::Error| {
        SdkError::arcane_unavailable(format!("Could not {what}: {e}"))
            .with_hint("Restart the Arcane Powered desktop app, then retry.")
    };

    let tcp_addr = SocketAddr::from(([127, 0, 0, 1], opened.tcp_port));
    let mut tcp = TcpStream::connect_timeout(&tcp_addr, CONNECT_TIMEOUT)
        .map_err(|e| local_failure("reach the coop network", e))?;
    let _ = tcp.set_nodelay(true);
    tcp.write_all(&encode(KIND_HELLO, 0, &key))
        .map_err(|e| local_failure("attach to the coop network", e))?;

    let udp = UdpSocket::bind(("127.0.0.1", 0))
        .map_err(|e| local_failure("open a local UDP socket", e))?;
    udp.set_read_timeout(Some(UDP_HELLO_PERIOD))
        .map_err(|e| local_failure("configure the local UDP socket", e))?;
    let desktop_udp = SocketAddr::from(([127, 0, 0, 1], opened.udp_port));
    let udp_hello = encode(KIND_HELLO, 0, &key);
    udp.send_to(&udp_hello, desktop_udp)
        .map_err(|e| local_failure("attach to the coop network", e))?;

    Ok(Arc::new(Link {
        tcp: Mutex::new(tcp),
        udp,
        desktop_udp,
        udp_hello,
        inbox: Mutex::new(Inbox::default()),
        running: AtomicBool::new(true),
    }))
}

fn decode_key(hex_key: &str) -> Result<Vec<u8>, SdkError> {
    match hex::decode(hex_key.trim()) {
        Ok(key) if key.len() == SESSION_KEY_LEN => Ok(key),
        _ => Err(SdkError::arcane_unavailable(
            "The Arcane desktop app answered an unreadable network key.",
        )
        .with_hint("The Arcane desktop app may be older than this SDK — update it.")),
    }
}

fn spawn_readers(link: Arc<Link>) {
    let tcp = {
        let guard = link.tcp.lock().unwrap_or_else(|e| e.into_inner());
        guard.try_clone()
    };

    let tcp_link = Arc::clone(&link);
    let spawned = match tcp {
        Ok(stream) => thread::Builder::new()
            .name("arcane-net-tcp".into())
            .spawn(move || read_tcp(tcp_link, stream))
            .is_ok(),
        Err(_) => false,
    };
    if !spawned {
        link.stop("socket_error");
        return;
    }

    let udp_link = Arc::clone(&link);
    if thread::Builder::new()
        .name("arcane-net-udp".into())
        .spawn(move || read_udp(udp_link))
        .is_err()
    {
        link.stop("socket_error");
    }
}

fn read_tcp(link: Arc<Link>, mut stream: TcpStream) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    while link.is_running() {
        loop {
            match decode(&buf) {
                Decoded::Frame {
                    kind,
                    peer,
                    data,
                    used,
                } => {
                    handle_frame(&link, kind, peer, data, Channel::Reliable);
                    buf.drain(..used);
                }
                Decoded::Incomplete => break,
                Decoded::Invalid => {
                    link.stop("protocol_error");
                    return;
                }
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    link.stop("desktop_closed");
}

fn read_udp(link: Arc<Link>) {
    let mut buf = vec![0u8; 2048];
    while link.is_running() {
        match link.udp.recv_from(&mut buf) {
            Ok((len, from)) => {
                if from != link.desktop_udp {
                    continue;
                }
                if let Decoded::Frame {
                    kind, peer, data, ..
                } = decode(&buf[..len])
                {
                    handle_frame(&link, kind, peer, data, Channel::Unreliable);
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                let _ = link.udp.send_to(&link.udp_hello, link.desktop_udp);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => {}
        }
    }
}

fn handle_frame(link: &Link, kind: u8, peer: u16, data: &[u8], channel: Channel) {
    if kind == KIND_MESSAGE {
        link.push_message(NetMessage {
            peer,
            channel,
            data: data.to_vec(),
        });
        return;
    }
    let Ok(body) = serde_json::from_slice::<WirePeer>(data) else {
        return;
    };
    let route = body.route.unwrap_or(Route::Direct);
    let event = match kind {
        KIND_PEER_CONNECTED => NetEvent::PeerConnected {
            peer,
            user_id: body.user_id,
            route,
            is_host: body.is_host,
        },
        KIND_QUALITY => NetEvent::Quality {
            peer,
            user_id: body.user_id,
            route,
            rtt_ms: body.rtt_ms,
            loss_pct: body.loss_pct,
            jitter_ms: body.jitter_ms,
        },
        KIND_PEER_DISCONNECTED => NetEvent::PeerDisconnected {
            peer,
            user_id: body.user_id,
            reason: body.reason,
        },
        KIND_RELAY_REFUSED => NetEvent::RelayRefused {
            peer,
            user_id: body.user_id,
        },
        _ => return,
    };
    link.push_event(event);
}

fn encode(kind: u8, peer: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + data.len());
    out.push(FRAME_VERSION);
    out.push(kind);
    out.extend_from_slice(&peer.to_be_bytes());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

enum Decoded<'a> {
    Frame {
        kind: u8,
        peer: u16,
        data: &'a [u8],
        used: usize,
    },
    Incomplete,
    Invalid,
}

fn decode(buf: &[u8]) -> Decoded<'_> {
    if buf.len() < HEADER_LEN {
        return Decoded::Incomplete;
    }
    if buf[0] != FRAME_VERSION {
        return Decoded::Invalid;
    }
    let kind = buf[1];
    let peer = u16::from_be_bytes([buf[2], buf[3]]);
    let len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if len > MAX_RELIABLE_MESSAGE {
        return Decoded::Invalid;
    }
    if buf.len() < HEADER_LEN + len {
        return Decoded::Incomplete;
    }
    Decoded::Frame {
        kind,
        peer,
        data: &buf[HEADER_LEN..HEADER_LEN + len],
        used: HEADER_LEN + len,
    }
}
