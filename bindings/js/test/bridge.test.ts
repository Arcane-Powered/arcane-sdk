import { describe, expect, test } from "bun:test";
import { Arcane, ArcaneError } from "../src/index";

type Handler = (method: string, params: any) => { result?: unknown; error?: string; message?: string } | null;

const HOST = "tauri://localhost";

function frame(handler: Handler) {
	const listeners: ((event: any) => void)[] = [];
	const calls: { method: string; params: any; targetOrigin: string }[] = [];
	const self: any = {
		addEventListener: (_type: string, listener: (event: any) => void) => listeners.push(listener),
	};
	const parent: any = {
		postMessage(message: any, targetOrigin: string) {
			calls.push({ method: message.method, params: message.params, targetOrigin });
			const reply = handler(message.method, message.params);
			if (!reply) return;
			queueMicrotask(() => {
				for (const listener of listeners) listener({ source: parent, origin: HOST, data: { arcane: 1, id: message.id, ...reply } });
			});
		},
	};
	const emit = (data: unknown, source: unknown = parent, origin = HOST) => {
		for (const listener of listeners) listener({ source, origin, data });
	};
	return { self, parent, calls, emit };
}

const hello = (rest: Handler = () => null): Handler => (method, params) =>
	method === "hello" ? { result: { available: true, game_id: "neon-drift" } } : rest(method, params);

describe("init", () => {
	test("is unavailable when the game is not framed", async () => {
		const self: any = { addEventListener() {} };
		self.parent = self;
		const arcane = await Arcane.init({ self });
		expect(arcane.available).toBe(false);
		expect(await arcane.achievements.list()).toEqual([]);
		expect(await arcane.saves.read("slot-1")).toBeNull();
	});

	test("is unavailable when the host never answers", async () => {
		const { self, parent } = frame(() => null);
		const arcane = await Arcane.init({ self, parent, timeoutMs: 20 });
		expect(arcane.available).toBe(false);
		expect(await arcane.friends.list()).toEqual({ friends: [], stale: false });
	});

	test("reads the game id from the host", async () => {
		const { self, parent } = frame(hello());
		const arcane = await Arcane.init({ self, parent });
		expect(arcane.available).toBe(true);
		expect(arcane.gameId).toBe("neon-drift");
	});
});

describe("calls", () => {
	test("unlock remembers the achievement", async () => {
		const { self, parent, calls } = frame(
			hello((method, params) =>
				method === "achievements.unlock"
					? { result: { key: params.key, unlocked_at: "2026-10-30T10:00:00Z", already_unlocked: false, queued: false } }
					: null,
			),
		);
		const arcane = await Arcane.init({ self, parent });
		const unlock = await arcane.achievements.unlock("first-blood");
		expect(unlock?.key).toBe("first-blood");
		expect(arcane.achievements.isUnlocked("first-blood")).toBe(true);
		expect(calls.at(-1)).toEqual({ method: "achievements.unlock", params: { key: "first-blood" }, targetOrigin: HOST });
	});

	test("an error reply rejects with its code", async () => {
		const { self, parent } = frame(
			hello(() => ({ error: "not_owned", message: "You do not own this game." })),
		);
		const arcane = await Arcane.init({ self, parent });
		const error = await arcane.achievements.list().catch((e) => e);
		expect(error).toBeInstanceOf(ArcaneError);
		expect(error.code).toBe("not_owned");
	});

	test("ignores messages from another window", async () => {
		const { self, parent, emit } = frame(hello());
		const arcane = await Arcane.init({ self, parent });
		let paused = 0;
		arcane.on("pause", () => paused++);
		emit({ arcane: 1, event: "pause" }, {});
		emit({ arcane: 2, event: "pause" });
		expect(paused).toBe(0);
		emit({ arcane: 1, event: "pause" });
		expect(paused).toBe(1);
	});

	test("list reads unlocked from unlocked_at", async () => {
		const { self, parent } = frame(
			hello((method) =>
				method === "achievements.list"
					? {
							result: {
								achievements: [
									{ key: "a", title: "A", hidden: false, tier: "patina", unlocked_at: "2026-10-30T10:00:00Z" },
									{ key: "b", title: "B", hidden: false, tier: "patina", unlocked_at: null },
								],
							},
						}
					: null,
			),
		);
		const arcane = await Arcane.init({ self, parent });
		const list = await arcane.achievements.list();
		expect(list.map((a) => a.unlocked)).toEqual([true, false]);
		expect(arcane.achievements.isUnlocked("a")).toBe(true);
		expect(arcane.achievements.isUnlocked("b")).toBe(false);
	});
});

describe("origin", () => {
	test("hello goes to any origin, then calls are pinned to the host that answered", async () => {
		const { self, parent, calls, emit } = frame(hello(() => ({ result: { achievements: [] } })));
		const arcane = await Arcane.init({ self, parent });
		await arcane.achievements.list();
		expect(calls[0].targetOrigin).toBe("*");
		expect(calls.at(-1)?.targetOrigin).toBe(HOST);
		let paused = 0;
		arcane.on("pause", () => paused++);
		emit({ arcane: 1, event: "pause" }, parent, "https://evil.example");
		expect(paused).toBe(0);
	});

	test("hostOrigin pins every message from the start", async () => {
		const { self, parent, calls } = frame(hello());
		const arcane = await Arcane.init({ self, parent, hostOrigin: HOST });
		expect(arcane.available).toBe(true);
		expect(calls[0].targetOrigin).toBe(HOST);
		const other = frame(hello());
		const refused = await Arcane.init({ self: other.self, parent: other.parent, hostOrigin: "http://tauri.localhost", timeoutMs: 20 });
		expect(refused.available).toBe(false);
	});
});

describe("lobbies", () => {
	test("payloads go out as base64 and come back as bytes", async () => {
		const { self, parent, calls } = frame(
			hello((method, params) =>
				method === "p2p.create"
					? {
							result: {
								lobby_id: "l1",
								join_code: "K7P3QX",
								host_user_id: "u1",
								host_payload: params.payload,
								visibility: params.visibility,
								max_players: params.max_players,
								members: [{ user_id: "u1", pseudo: "ana", payload: params.payload, joined_at: "" }],
								expires_at: "",
							},
						}
					: null,
			),
		);
		const arcane = await Arcane.init({ self, parent });
		const lobby = await arcane.p2p.createLobby(4, "friends", "hi");
		expect(calls.at(-1)?.params.payload).toBe(btoa("hi"));
		expect(new TextDecoder().decode(lobby!.host_payload)).toBe("hi");
		expect(lobby!.metadata).toEqual({});
	});

	test("search sends metadata filters as meta.<key>", async () => {
		const { self, parent, calls } = frame(hello(() => ({ result: { lobbies: [] } })));
		const arcane = await Arcane.init({ self, parent });
		await arcane.p2p.search({ filters: { mode: "coop" }, limit: 5 });
		expect(calls.at(-1)?.params).toEqual({ has_free_slot: true, limit: 5, "meta.mode": "coop" });
	});

	test("events keep the cursor and flag a resync after a drop", async () => {
		let round = 0;
		const { self, parent, calls } = frame(
			hello(() => {
				round++;
				return round === 1
					? { result: { events: [{ type: "lobby_closed", lobby_id: "l1" }], cursor: 7, dropped: false } }
					: { result: { events: [{ type: "unknown" }], cursor: 9, dropped: true } };
			}),
		);
		const arcane = await Arcane.init({ self, parent });
		expect(await arcane.p2p.pollEvents()).toEqual([{ type: "lobby_closed", lobby_id: "l1" }]);
		expect(await arcane.p2p.pollEvents()).toEqual([{ type: "resync" }]);
		expect(calls.at(-1)?.params).toEqual({ after: 7 });
	});
});

describe("saves", () => {
	test("write sends bytes and readText decodes them", async () => {
		const stored = new Map<string, Uint8Array>();
		const { self, parent } = frame(
			hello((method, params) => {
				if (method === "saves.write") {
					stored.set(params.slot, params.data);
					return { result: { slot: params.slot, size_bytes: params.data.length, updated_at: "" } };
				}
				if (method === "saves.read") return { result: { slot: params.slot, data: stored.get(params.slot) ?? null } };
				return null;
			}),
		);
		const arcane = await Arcane.init({ self, parent });
		const slot = await arcane.saves.write("slot-1", "level 3");
		expect(slot?.size_bytes).toBe(7);
		expect(stored.get("slot-1")).toBeInstanceOf(Uint8Array);
		expect(await arcane.saves.readText("slot-1")).toBe("level 3");
		expect(await arcane.saves.read("missing")).toBeNull();
	});
});
