// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>Which of the two coop channels a message travels on.</summary>
    public enum ArcaneNetChannel
    {
        /// <summary>Ordered and retransmitted until delivered, up to 65535 bytes.</summary>
        Reliable = 0,

        /// <summary>Unordered and never retransmitted, up to 1200 bytes. For positions and inputs.</summary>
        Unreliable = 1,
    }

    /// <summary>How a link reaches the other player.</summary>
    public enum ArcaneNetRoute
    {
        /// <summary>Straight between the two machines.</summary>
        Direct,

        /// <summary>Through Arcane's relay, because no direct path was found.</summary>
        Relayed,
    }

    /// <summary>What an <see cref="ArcaneNetEvent"/> reports.</summary>
    public enum ArcaneNetEventType
    {
        /// <summary>A link is up. <see cref="ArcaneNetEvent.Peer"/> is the number to send to.</summary>
        PeerConnected,

        /// <summary>The link's route, round trip or loss moved noticeably.</summary>
        Quality,

        /// <summary>The link is gone: the player left, the lobby closed, or connecting failed.</summary>
        PeerDisconnected,

        /// <summary>No direct path, and the relay was refused on one side.</summary>
        RelayRefused,

        /// <summary>The network closed. Open it again to reconnect.</summary>
        Closed,
    }

    /// <summary>One message received from a peer.</summary>
    public struct ArcaneNetMessage
    {
        /// <summary>The sender's peer number.</summary>
        public ushort Peer;

        /// <summary>The channel it arrived on.</summary>
        public ArcaneNetChannel Channel;

        /// <summary>The bytes, exactly as the sender passed them.</summary>
        public byte[] Data;
    }

    /// <summary>Something that happened to the coop network.</summary>
    public sealed class ArcaneNetEvent
    {
        private ArcaneNetEvent()
        {
        }

        /// <summary>What happened.</summary>
        public ArcaneNetEventType Type { get; private set; }

        /// <summary>The peer number of the player concerned. 0 on <see cref="ArcaneNetEventType.Closed"/>.</summary>
        public ushort Peer { get; private set; }

        /// <summary>Their Arcane account id, or empty on <see cref="ArcaneNetEventType.Closed"/>.</summary>
        public string UserId { get; private set; }

        /// <summary>The link's route on the first two event types.</summary>
        public ArcaneNetRoute Route { get; private set; }

        /// <summary>Whether that player hosts the lobby. Set on <see cref="ArcaneNetEventType.PeerConnected"/>.</summary>
        public bool IsHost { get; private set; }

        /// <summary>Round trip in milliseconds, or <see langword="null"/> until measured.</summary>
        public float? RttMs { get; private set; }

        /// <summary>Share of probes lost over the last 20, from 0 to 100.</summary>
        public float LossPercent { get; private set; }

        /// <summary>Variation of the round trip in milliseconds, or <see langword="null"/> until measured.</summary>
        public float? JitterMs { get; private set; }

        /// <summary>Why a link or the network ended, on the last two event types.</summary>
        public string Reason { get; private set; }

        internal static ArcaneNetEvent FromJson(ArcaneJson node)
        {
            return new ArcaneNetEvent
            {
                Type = ParseType(node["type"].AsString(string.Empty)),
                Peer = (ushort)node["peer"].AsLong(),
                UserId = node["user_id"].AsString(string.Empty),
                Route = node["route"].AsString(string.Empty) == "relayed" ? ArcaneNetRoute.Relayed : ArcaneNetRoute.Direct,
                IsHost = node["is_host"].AsBool(),
                RttMs = node["rtt_ms"].AsNullableFloat(),
                LossPercent = node["loss_pct"].AsNullableFloat() ?? 0f,
                JitterMs = node["jitter_ms"].AsNullableFloat(),
                Reason = node["reason"].AsString(string.Empty),
            };
        }

        private static ArcaneNetEventType ParseType(string type)
        {
            switch (type)
            {
                case "peer_connected": return ArcaneNetEventType.PeerConnected;
                case "quality": return ArcaneNetEventType.Quality;
                case "peer_disconnected": return ArcaneNetEventType.PeerDisconnected;
                case "relay_refused": return ArcaneNetEventType.RelayRefused;
                default: return ArcaneNetEventType.Closed;
            }
        }
    }

    /// <summary>
    /// The coop network: reliable and unreliable messages between the members
    /// of a lobby, with no port to open. Reached through <see cref="Arcane.Net"/>.
    /// </summary>
    /// <remarks>
    /// The Arcane desktop app connects the players — directly when it can,
    /// through Arcane's relay otherwise — and the game only reads and writes
    /// local sockets. The topology is a star around the lobby host: a member
    /// sends to <see cref="Host"/>, the host sends to a peer number or to
    /// <see cref="All"/>.
    /// <para>
    /// <see cref="Open"/> and <see cref="Close"/> make one loopback call — not
    /// from <c>Update</c>. <see cref="Send"/>, <see cref="Receive"/> and
    /// <see cref="PollEvents"/> never wait on the network: call them every
    /// frame. While the network is open, <see cref="ArcaneRuntime"/> drains the
    /// events every frame and raises the C# events below; calling
    /// <see cref="PollEvents"/> yourself as well means each event reaches only
    /// one of the two.
    /// </para>
    /// </remarks>
    public sealed class ArcaneNet
    {
        /// <summary>Send to the lobby host.</summary>
        public const ushort Host = 0xFFFE;

        /// <summary>Send to every connected peer. Only the host can broadcast.</summary>
        public const ushort All = 0xFFFF;

        /// <summary>Largest reliable message, in bytes.</summary>
        public const int MaxReliableBytes = 65535;

        /// <summary>Largest unreliable message, in bytes.</summary>
        public const int MaxUnreliableBytes = 1200;

        [ThreadStatic]
        private static byte[] _sendError;

        [ThreadStatic]
        private static byte[] _receiveBuffer;

        private volatile bool _opened;

        internal ArcaneNet()
        {
        }

        /// <summary>A link came up.</summary>
        public event Action<ArcaneNetEvent> PeerConnected;

        /// <summary>A link's quality moved.</summary>
        public event Action<ArcaneNetEvent> QualityChanged;

        /// <summary>A link went away.</summary>
        public event Action<ArcaneNetEvent> PeerDisconnected;

        /// <summary>No direct path to a player, and the relay was refused.</summary>
        public event Action<ArcaneNetEvent> RelayRefused;

        /// <summary>The network closed.</summary>
        public event Action<ArcaneNetEvent> Closed;

        /// <summary>Whether the network is open and attached to the desktop app.</summary>
        public bool IsOpen
        {
            get { return ArcaneNative.IsAvailable && ArcaneNative.arcane_sdk_net_is_open() == 1; }
        }

        // --- Open and close ------------------------------------------------

        /// <summary>
        /// Open the network on a lobby this player already created or joined,
        /// answering <see langword="false"/> instead of throwing.
        /// </summary>
        /// <param name="lobbyId">The lobby's id.</param>
        /// <param name="allowRelay">
        /// <see langword="false"/> keeps every link direct: a pair of players
        /// that cannot connect directly gets <see cref="RelayRefused"/> instead.
        /// </param>
        public bool TryOpen(string lobbyId, bool allowRelay, out ArcaneError error)
        {
            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            bool opened = ArcaneCall.Run(
                (buffer, length) => ArcaneNative.arcane_sdk_net_open(id, allowRelay ? 1 : 0, buffer, length),
                out error);
            if (opened)
            {
                _opened = true;
            }

            return opened;
        }

        /// <summary>Open the network, throwing <see cref="ArcaneException"/> on failure.</summary>
        public void Open(string lobbyId, bool allowRelay = true)
        {
            ArcaneError error;
            if (!TryOpen(lobbyId, allowRelay, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>Open the network on a background thread. Answers whether it opened.</summary>
        public Task<bool> OpenAsync(string lobbyId, bool allowRelay = true)
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryOpen(lobbyId, allowRelay, out ignored);
            });
        }

        /// <summary>Close the network for this player. Messages not yet received are discarded.</summary>
        public void Close()
        {
            _opened = false;
            if (ArcaneNative.IsAvailable)
            {
                ArcaneNative.arcane_sdk_net_close();
            }
        }

        // --- Messages ------------------------------------------------------

        /// <summary>
        /// Send the first <paramref name="length"/> bytes of
        /// <paramref name="data"/>, answering <see langword="false"/> instead of
        /// throwing. Allocates nothing on success.
        /// </summary>
        /// <param name="peer">A peer number, <see cref="Host"/> or <see cref="All"/>.</param>
        public bool TrySend(ushort peer, ArcaneNetChannel channel, byte[] data, int length, out ArcaneError error)
        {
            if (!ArcaneNative.IsAvailable)
            {
                error = ArcaneError.PluginMissing();
                return false;
            }

            if (data == null || length < 0 || length > data.Length)
            {
                error = ArcaneError.Argument(
                    "The message buffer or its length is invalid.",
                    "Pass a non-null array and a length between 0 and its size.");
                return false;
            }

            if (_sendError == null)
            {
                _sendError = ArcaneBuffer.ErrorBuffer();
            }

            int rc = ArcaneNative.arcane_sdk_net_send(
                peer,
                (int)channel,
                data,
                (UIntPtr)length,
                _sendError,
                (UIntPtr)_sendError.Length);

            if (rc == ArcaneNative.Ok)
            {
                error = null;
                return true;
            }

            error = rc == ArcaneNative.ErrArgument
                ? (ArcaneNative.arcane_sdk_is_initialized() == 1
                    ? ArcaneError.Argument("The Arcane SDK rejected the channel.", "Use ArcaneNetChannel.")
                    : ArcaneError.NotInitialized())
                : ArcaneError.Capture(_sendError);
            return false;
        }

        /// <summary>Send a whole array, answering <see langword="false"/> instead of throwing.</summary>
        public bool TrySend(ushort peer, ArcaneNetChannel channel, byte[] data, out ArcaneError error)
        {
            return TrySend(peer, channel, data, data == null ? 0 : data.Length, out error);
        }

        /// <summary>Send a whole array, throwing <see cref="ArcaneException"/> on failure.</summary>
        public void Send(ushort peer, ArcaneNetChannel channel, byte[] data)
        {
            ArcaneError error;
            if (!TrySend(peer, channel, data, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>
        /// Copy the next received message into <paramref name="buffer"/>.
        /// Allocates nothing. Call it in a loop every frame until it answers -1.
        /// </summary>
        /// <returns>
        /// The message length (it can be 0), or -1 when nothing is waiting. A
        /// buffer of <see cref="MaxReliableBytes"/> always fits; a smaller one
        /// that does not fit throws <see cref="ArgumentException"/> and keeps
        /// the message.
        /// </returns>
        public int Receive(byte[] buffer, out ushort peer, out ArcaneNetChannel channel)
        {
            peer = 0;
            channel = ArcaneNetChannel.Reliable;

            if (buffer == null)
            {
                throw new ArgumentNullException("buffer");
            }

            if (!ArcaneNative.IsAvailable)
            {
                return -1;
            }

            ushort from;
            int rawChannel;
            int written = ArcaneNative.arcane_sdk_net_receive(out from, out rawChannel, buffer, (UIntPtr)buffer.Length);

            if (written == ArcaneNative.ErrBufferTooSmall)
            {
                throw new ArgumentException(
                    "The next message is larger than the buffer. Use one of MaxReliableBytes.",
                    "buffer");
            }

            if (written < 0)
            {
                return -1;
            }

            peer = from;
            channel = rawChannel == (int)ArcaneNetChannel.Unreliable
                ? ArcaneNetChannel.Unreliable
                : ArcaneNetChannel.Reliable;
            return written;
        }

        /// <summary>
        /// Take the next received message as a fresh array, or answer
        /// <see langword="false"/> when nothing is waiting.
        /// </summary>
        public bool TryReceive(out ArcaneNetMessage message)
        {
            message = default(ArcaneNetMessage);

            if (_receiveBuffer == null)
            {
                _receiveBuffer = new byte[MaxReliableBytes];
            }

            ushort peer;
            ArcaneNetChannel channel;
            int length = Receive(_receiveBuffer, out peer, out channel);
            if (length < 0)
            {
                return false;
            }

            var data = new byte[length];
            Buffer.BlockCopy(_receiveBuffer, 0, data, 0, length);
            message = new ArcaneNetMessage { Peer = peer, Channel = channel, Data = data };
            return true;
        }

        // --- Events --------------------------------------------------------

        /// <summary>Take the network events collected since the last call, oldest first.</summary>
        public ArcaneNetEvent[] PollEvents()
        {
            ArcaneJson root;
            ArcaneError error;
            if (!ArcaneCall.ReadJson(
                    ArcaneNative.arcane_sdk_net_events_json,
                    ArcaneBuffer.ListBufferSize,
                    out root,
                    out error))
            {
                return Array.Empty<ArcaneNetEvent>();
            }

            var events = new List<ArcaneNetEvent>();
            foreach (var node in root["events"].Items)
            {
                events.Add(ArcaneNetEvent.FromJson(node));
            }

            return events.ToArray();
        }

        /// <summary>
        /// Drain the queue and raise the C# events. Called by
        /// <see cref="ArcaneRuntime"/> on the main thread, once the network
        /// has been opened.
        /// </summary>
        internal void PumpEvents()
        {
            if (!_opened)
            {
                return;
            }

            foreach (var netEvent in PollEvents())
            {
                switch (netEvent.Type)
                {
                    case ArcaneNetEventType.PeerConnected:
                        Raise(PeerConnected, netEvent);
                        break;
                    case ArcaneNetEventType.Quality:
                        Raise(QualityChanged, netEvent);
                        break;
                    case ArcaneNetEventType.PeerDisconnected:
                        Raise(PeerDisconnected, netEvent);
                        break;
                    case ArcaneNetEventType.RelayRefused:
                        Raise(RelayRefused, netEvent);
                        break;
                    case ArcaneNetEventType.Closed:
                        _opened = false;
                        Raise(Closed, netEvent);
                        break;
                }
            }
        }

        private static void Raise(Action<ArcaneNetEvent> handler, ArcaneNetEvent netEvent)
        {
            if (handler == null)
            {
                return;
            }

            try
            {
                handler(netEvent);
            }
            catch (Exception exception)
            {
                ArcaneLog.Exception(exception);
            }
        }
    }
}
