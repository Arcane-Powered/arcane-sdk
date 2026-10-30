const PROTOCOL = 1;
const HELLO_TIMEOUT_MS = 1000;

export type Visibility = "friends" | "code" | "friends_and_code" | "public";
export type Bytes = Uint8Array | string;

export interface Achievement {
	key: string;
	title: string;
	description?: string | null;
	icon_url?: string | null;
	hidden: boolean;
	tier: string;
	unlocked: boolean;
	unlocked_at?: string | null;
}

export interface Unlock {
	key: string;
	unlocked_at: string | null;
	already_unlocked: boolean;
	queued: boolean;
}

export interface Friend {
	user_id: string;
	pseudo: string;
	online: boolean;
	playing_game_id: string | null;
	joinable: boolean;
	lobby_id: string | null;
}

export interface FriendList {
	friends: Friend[];
	stale: boolean;
}

export interface LobbyMember {
	user_id: string;
	pseudo: string;
	payload: Uint8Array;
}

export interface Lobby {
	lobby_id: string;
	join_code: string | null;
	host_user_id: string;
	host_payload: Uint8Array;
	visibility: Visibility;
	max_players: number;
	metadata: Record<string, string>;
	members: LobbyMember[];
	expires_at: string;
}

export interface LobbyOptions {
	maxPlayers: number;
	visibility: Visibility;
	payload: Bytes;
	metadata?: Record<string, string>;
}

export interface LobbySummary {
	lobby_id: string;
	host_user_id: string;
	host_pseudo: string;
	members: number;
	max_players: number;
	metadata: Record<string, string>;
	host_is_friend: boolean;
}

export interface LobbyQuery {
	filters?: Record<string, string>;
	hasFreeSlot?: boolean;
	limit?: number;
}

export type LobbyEvent =
	| { type: "invite"; lobby_id: string; join_code: string | null; from_user_id: string; pseudo: string }
	| { type: "member_joined"; lobby_id: string; user_id: string; pseudo: string; payload: Uint8Array }
	| { type: "member_left"; lobby_id: string; user_id: string }
	| { type: "lobby_closed"; lobby_id: string }
	| { type: "lobby_updated"; lobby_id: string; metadata: Record<string, string> }
	| { type: "resync" };

export interface SaveSlot {
	slot: string;
	size_bytes: number;
	updated_at: string | null;
}

export type ArcaneEvent = "pause" | "resume";

export class ArcaneError extends Error {
	readonly code: string;

	constructor(code: string, message: string) {
		super(message);
		this.name = "ArcaneError";
		this.code = code;
	}
}

type Pending = { method: string; resolve: (value: unknown) => void; reject: (error: ArcaneError) => void };

interface Transport {
	call(method: string, params?: unknown): Promise<unknown>;
}

const encoder = new TextEncoder();

function toBytes(value: Bytes): Uint8Array {
	return typeof value === "string" ? encoder.encode(value) : value;
}

function toBase64(value: Bytes): string {
	const bytes = toBytes(value);
	let binary = "";
	for (let i = 0; i < bytes.length; i += 0x8000) {
		binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
	}
	return btoa(binary);
}

function fromBase64(value: unknown): Uint8Array {
	if (typeof value !== "string" || value === "") return new Uint8Array();
	try {
		const binary = atob(value);
		const bytes = new Uint8Array(binary.length);
		for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
		return bytes;
	} catch {
		return new Uint8Array();
	}
}

function mapLobby(raw: any): Lobby {
	return {
		...raw,
		metadata: raw.metadata ?? {},
		host_payload: fromBase64(raw.host_payload),
		members: (raw.members ?? []).map((member: any) => ({ ...member, payload: fromBase64(member.payload) })),
	};
}

function mapEvent(raw: any): LobbyEvent | null {
	switch (raw?.type) {
		case "invite":
			return {
				type: "invite",
				lobby_id: raw.lobby_id,
				join_code: raw.join_code || null,
				from_user_id: raw.from_user_id ?? "",
				pseudo: raw.pseudo ?? "",
			};
		case "member_joined":
			return {
				type: "member_joined",
				lobby_id: raw.lobby_id,
				user_id: raw.user_id ?? "",
				pseudo: raw.pseudo ?? "",
				payload: fromBase64(raw.payload),
			};
		case "member_left":
			return { type: "member_left", lobby_id: raw.lobby_id, user_id: raw.user_id ?? "" };
		case "lobby_closed":
			return { type: "lobby_closed", lobby_id: raw.lobby_id };
		case "lobby_updated":
			return { type: "lobby_updated", lobby_id: raw.lobby_id, metadata: raw.metadata ?? {} };
		default:
			return null;
	}
}

class Bridge implements Transport {
	private nextId = 1;
	private readonly pending = new Map<number, Pending>();
	private readonly listeners = new Map<ArcaneEvent, Set<() => void>>();

	constructor(
		private readonly target: Window,
		private readonly source: Window,
		private hostOrigin: string,
	) {
		source.addEventListener("message", (event) => this.receive(event));
	}

	call(method: string, params?: unknown): Promise<unknown> {
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			this.pending.set(id, { method, resolve, reject });
			this.target.postMessage({ arcane: PROTOCOL, id, method, params: params ?? null }, this.hostOrigin);
		});
	}

	on(event: ArcaneEvent, listener: () => void): () => void {
		const set = this.listeners.get(event) ?? new Set();
		set.add(listener);
		this.listeners.set(event, set);
		return () => set.delete(listener);
	}

	forget(id: number) {
		this.pending.delete(id);
	}

	get lastId() {
		return this.nextId - 1;
	}

	private receive(event: MessageEvent) {
		if (event.source !== this.target) return;
		if (this.hostOrigin !== "*" && event.origin !== this.hostOrigin) return;
		const data = event.data;
		if (!data || typeof data !== "object" || data.arcane !== PROTOCOL) return;
		if (data.event === "pause" || data.event === "resume") {
			for (const listener of this.listeners.get(data.event) ?? []) listener();
			return;
		}
		const pending = this.pending.get(data.id);
		if (!pending) return;
		this.pending.delete(data.id);
		if (pending.method === "hello" && this.hostOrigin === "*" && event.origin) this.hostOrigin = event.origin;
		if (typeof data.error === "string") pending.reject(new ArcaneError(data.error, data.message ?? data.error));
		else pending.resolve(data.result);
	}
}

export interface InitOptions {
	timeoutMs?: number;
	hostOrigin?: string;
	parent?: Window;
	self?: Window;
}

export class Arcane {
	readonly achievements: Achievements;
	readonly friends: Friends;
	readonly p2p: P2p;
	readonly saves: Saves;

	private constructor(
		readonly available: boolean,
		readonly gameId: string | null,
		private readonly bridge: Bridge | null,
	) {
		const transport: Transport | null = bridge;
		this.achievements = new Achievements(transport);
		this.friends = new Friends(transport);
		this.p2p = new P2p(transport);
		this.saves = new Saves(transport);
	}

	static async init(options: InitOptions = {}): Promise<Arcane> {
		const self = options.self ?? (typeof window === "undefined" ? undefined : window);
		const parent = options.parent ?? self?.parent;
		if (!self || !parent || parent === self) return new Arcane(false, null, null);
		const bridge = new Bridge(parent, self, options.hostOrigin ?? "*");
		const hello = bridge.call("hello") as Promise<{ available?: boolean; game_id?: string }>;
		const id = bridge.lastId;
		const timeout = new Promise<null>((resolve) =>
			setTimeout(() => resolve(null), options.timeoutMs ?? HELLO_TIMEOUT_MS),
		);
		const answer = await Promise.race([hello.catch(() => null), timeout]);
		if (!answer?.available) {
			bridge.forget(id);
			return new Arcane(false, null, null);
		}
		return new Arcane(true, answer.game_id ?? null, bridge);
	}

	on(event: ArcaneEvent, listener: () => void): () => void {
		return this.bridge ? this.bridge.on(event, listener) : () => {};
	}
}

class Achievements {
	private readonly unlocked = new Map<string, boolean>();

	constructor(private readonly transport: Transport | null) {}

	async list(): Promise<Achievement[]> {
		if (!this.transport) return [];
		const reply = (await this.transport.call("achievements.list")) as { achievements?: Achievement[] };
		const list = (reply?.achievements ?? []).map((achievement) => ({
			...achievement,
			unlocked: achievement.unlocked ?? achievement.unlocked_at != null,
		}));
		for (const achievement of list) this.unlocked.set(achievement.key, achievement.unlocked);
		return list;
	}

	async unlock(key: string): Promise<Unlock | null> {
		if (!this.transport) return null;
		const reply = (await this.transport.call("achievements.unlock", { key })) as Unlock;
		this.unlocked.set(key, true);
		return reply;
	}

	isUnlocked(key: string): boolean | undefined {
		return this.unlocked.get(key);
	}
}

class Friends {
	private cursor: number | string | null = null;

	constructor(private readonly transport: Transport | null) {}

	async list(): Promise<FriendList> {
		if (!this.transport) return { friends: [], stale: false };
		return (await this.transport.call("friends.list")) as FriendList;
	}

	async sendRequest(userId: string): Promise<void> {
		if (!this.transport) return;
		await this.transport.call("friends.sendRequest", { user_id: userId });
	}

	async pollEvents(): Promise<unknown[]> {
		if (!this.transport) return [];
		const reply = (await this.transport.call(
			"friends.events",
			this.cursor === null ? {} : { after: this.cursor },
		)) as { events?: unknown[]; cursor?: number | string | null };
		if (reply?.cursor !== undefined && reply.cursor !== null) this.cursor = reply.cursor;
		return reply?.events ?? [];
	}
}

class P2p {
	private cursor: number | string | null = null;

	constructor(private readonly transport: Transport | null) {}

	private async lobby(method: string, params: Record<string, unknown>): Promise<Lobby | null> {
		if (!this.transport) return null;
		return mapLobby(await this.transport.call(method, params));
	}

	createLobby(maxPlayers: number, visibility: Visibility, payload: Bytes): Promise<Lobby | null> {
		return this.create({ maxPlayers, visibility, payload });
	}

	create(options: LobbyOptions): Promise<Lobby | null> {
		return this.lobby("p2p.create", {
			max_players: options.maxPlayers,
			visibility: options.visibility,
			payload: toBase64(options.payload),
			metadata: options.metadata ?? {},
		});
	}

	joinByCode(joinCode: string, payload: Bytes): Promise<Lobby | null> {
		return this.lobby("p2p.joinByCode", { join_code: joinCode, payload: toBase64(payload) });
	}

	join(lobbyId: string, payload: Bytes): Promise<Lobby | null> {
		return this.lobby("p2p.join", { lobby_id: lobbyId, payload: toBase64(payload) });
	}

	getLobby(lobbyId: string): Promise<Lobby | null> {
		return this.lobby("p2p.getLobby", { lobby_id: lobbyId });
	}

	setMetadata(lobbyId: string, metadata: Record<string, string>): Promise<Lobby | null> {
		return this.lobby("p2p.setMetadata", { lobby_id: lobbyId, metadata });
	}

	async search(query: LobbyQuery = {}): Promise<LobbySummary[]> {
		if (!this.transport) return [];
		const params: Record<string, unknown> = {
			has_free_slot: query.hasFreeSlot ?? true,
			limit: query.limit ?? 20,
		};
		for (const [key, value] of Object.entries(query.filters ?? {})) params[`meta.${key}`] = value;
		const reply = (await this.transport.call("p2p.search", params)) as { lobbies?: LobbySummary[] };
		return reply?.lobbies ?? [];
	}

	async invite(lobbyId: string, toUserId: string): Promise<void> {
		if (!this.transport) return;
		await this.transport.call("p2p.invite", { lobby_id: lobbyId, to_user_id: toUserId });
	}

	async leave(lobbyId: string): Promise<void> {
		if (!this.transport) return;
		await this.transport.call("p2p.leave", { lobby_id: lobbyId });
	}

	async close(lobbyId: string): Promise<void> {
		if (!this.transport) return;
		await this.transport.call("p2p.close", { lobby_id: lobbyId });
	}

	async launchJoinCode(): Promise<string | null> {
		if (!this.transport) return null;
		const reply = (await this.transport.call("p2p.launchContext")) as { join_code?: string | null };
		return reply?.join_code || null;
	}

	async pollEvents(): Promise<LobbyEvent[]> {
		if (!this.transport) return [];
		const reply = (await this.transport.call(
			"p2p.events",
			this.cursor === null ? {} : { after: this.cursor },
		)) as { events?: unknown[]; cursor?: number | string | null; dropped?: boolean };
		if (reply?.cursor !== undefined && reply.cursor !== null) this.cursor = reply.cursor;
		const events = (reply?.events ?? []).map(mapEvent).filter((event): event is LobbyEvent => event !== null);
		return reply?.dropped ? [{ type: "resync" }, ...events] : events;
	}
}

class Saves {
	constructor(private readonly transport: Transport | null) {}

	async list(): Promise<SaveSlot[]> {
		if (!this.transport) return [];
		const reply = (await this.transport.call("saves.list")) as { slots?: SaveSlot[] };
		return reply?.slots ?? [];
	}

	async read(slot: string): Promise<Uint8Array | null> {
		if (!this.transport) return null;
		const reply = (await this.transport.call("saves.read", { slot })) as { data?: Uint8Array | null };
		return reply?.data ?? null;
	}

	async readText(slot: string): Promise<string | null> {
		const data = await this.read(slot);
		return data === null ? null : new TextDecoder().decode(data);
	}

	async write(slot: string, data: Bytes): Promise<SaveSlot | null> {
		if (!this.transport) return null;
		return (await this.transport.call("saves.write", { slot, data: toBytes(data) })) as SaveSlot;
	}

	async delete(slot: string): Promise<boolean> {
		if (!this.transport) return false;
		const reply = (await this.transport.call("saves.delete", { slot })) as { deleted?: boolean };
		return reply?.deleted ?? false;
	}
}

export default Arcane;
