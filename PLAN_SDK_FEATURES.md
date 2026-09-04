# Plan — SDK de base : sessions (temps de jeu + FPS), achievements, amis, P2P

> Côté **SDK** (`arcane-sdk`). Le miroir backend + desktop est dans
> `arcane-powered-mvp/PLAN_SDK_FEATURES.md`. Les deux plans partagent le contrat
> loopback décrit en §2 ; il sera recopié dans `DESKTOP_CONTRACT.md` à l'implémentation.
>
> Plan uniquement — rien n'est implémenté.

## 0. Principe : un one-liner, le reste est optionnel

L'intégration par défaut reste **une ligne**. `init` fait déjà la vérification de
propriété ; il ouvre désormais aussi une *session de jeu* qui mesure le temps joué et
les FPS sans autre appel du jeu.

```rust
let client = ArcaneClient::init("pk_...")?;
```

C'est tout pour : propriété, temps de jeu, échantillons FPS de temps en temps (si le
jeu appelle `frame()` et si le joueur n'a pas désactivé l'option dans le desktop),
présence « en jeu » pour les amis. Le reste est opt-in, une ligne par usage :

```rust
client.frame();                                   // dans la boucle de rendu → FPS
client.achievements().unlock("first_blood")?;     // achievement
client.friends().list()?;                         // amis + en ligne / en jeu
client.p2p().invite(friend_id, blob)?;            // phase 4
```

```c
arcane_sdk_init("pk_...", err, sizeof err);
arcane_sdk_frame();
arcane_sdk_achievement_unlock("first_blood", err, sizeof err);
arcane_sdk_friends_json(buf, sizeof buf);
```

### Ce que ça change dans le modèle actuel

| Aujourd'hui | Après |
|---|---|
| « Pas de thread, pas de timer » (`concepts/client.mdx`) | **Un** thread de session, endormi 59 s sur 60 (budget perf ci-dessous). La doc est réécrite en ce sens. La propriété, elle, n'est toujours jamais revalidée seule. |
| `ArcaneClient: Clone` par copie de valeurs | `Clone` partage la session (`Arc`). La session se termine quand le **dernier** clone est droppé, ou sur `shutdown()`. |
| Le desktop n'est contacté que pour rafraîchir un ticket | Le desktop reçoit aussi les heartbeats, les unlocks, les lectures amis. Le SDK **n'ouvre jamais le deep link** pour ces appels : si le desktop n'est pas là, la fonctionnalité est dégradée, jamais bloquante. |
| Erreurs : 13 codes | + `feature_unavailable` (desktop trop ancien / route 404), `unknown_achievement`, `tracking_unavailable` (réservé au JSON de diagnostic, jamais une `Err` de `init`). `ErrorCode` est `#[non_exhaustive]`, donc additif. |

### Décisions verrouillées

| Décision | Choix |
|---|---|
| Le tracking est-il bloquant pour `init` ? | **Non.** `init` réussit dès que la propriété est confirmée. Le démarrage de session est tenté en arrière-plan et réessayé à chaque tick. |
| Horloge du temps de jeu | `Instant` (monotone). Jamais l'horloge murale, donc insensible à `clock_rollback`. |
| FPS : échantillonnage, pas comptage continu | Le thread de session ouvre une **fenêtre de 30 s toutes les 5 min** (la première 60 s après le start, pour sauter le chargement). Pendant une fenêtre `frame()` incrémente un compteur ; en dehors, c'est un seul `AtomicBool::load`. Chaque fenêtre donne un échantillon `{ fps_avg, window_seconds, frames }` envoyé au heartbeat suivant. Sans appel à `frame()`, aucun échantillon. |
| FPS : option joueur | Activable / désactivable par le joueur dans **Arcane Powered desktop** (réglage « Share performance data », défaut activé). Le SDK ne décide rien : `session/start` et chaque réponse de heartbeat portent `fps_sampling: bool` ; à `false`, aucune fenêtre n'est ouverte et `frame()` reste un load. Le changement en cours de partie est pris en compte au heartbeat suivant. |
| FPS : usage | Moyenne par configuration matérielle, affichée sur la page store (déjà en place côté desktop : « fps average on this pc » / « similar PCs »). Le SDK ne fait que produire les échantillons. |
| Perte de données | Si le desktop n'est jamais joignable pendant toute la session, le temps de jeu de cette session est perdu. Pas de fichier tampon côté SDK : `sdk_server.rs` interdit les fichiers comme bus d'API. |
| `ARCANE_OFFLINE_ONLY` | Désactive aussi le thread de session et tous les appels achievements / amis (`network_required`). |
| Anti-triche | **Hors périmètre.** Un unlock est une requête loopback qu'un process local peut forger (cf. `DESKTOP_CONTRACT.md` §6). La validation de forme et de plausibilité est côté backend. |
| Versioning | `0.5.0` (`feat:`) pour la phase 1. Chaque phase suivante est un `feat:` mineur. Le header C est regénéré par `.github/scripts/generate-header.sh`. |
| P2P | Pas de relais, pas de transport. Arcane fournit le **lobby**, le **code d'invitation**, les invitations entre amis, le « rejoindre » depuis le launcher, et l'échange de blobs de connexion. Le trafic de jeu est au jeu. |

### Budget perf du thread de session

Le tracking est actif par défaut, donc il doit être invisible pour le jeu :

| Chemin | Coût | Règle |
|---|---|---|
| `frame()` | un `AtomicBool::load(Relaxed)` ; pendant une fenêtre d'échantillonnage (30 s / 5 min), un `fetch_add` relaxed en plus | pas de lock, pas d'allocation, pas de lecture d'horloge ; 90 % du temps c'est un seul load |
| `set_graphics()` | un `Mutex` court, appelé rarement | jamais dans la boucle de rendu |
| thread `arcane-session` | endormi sur `Condvar::wait_timeout(60 s)`, réveil ~1/min, un `POST` loopback de ~200 octets | aucune boucle active, aucun `sleep` fin, pile 64 KiB |
| `p2p` events | polling loopback à chaque tick **uniquement** si `p2p()` a été appelé | zéro coût pour un jeu qui ne s'en sert pas |
| `achievements()` / `friends()` | synchrones sur le thread appelant, un aller-retour loopback local (~1 ms) | à appeler hors du thread de rendu ou en acceptant ~1 ms ; jamais par frame |
| `shutdown()` | un `POST session/end` synchrone, timeout 2 s | le seul appel bloquant du cycle de vie |

Mesure de sortie de phase 1 : un jeu qui appelle `frame()` à 1000 fps ne doit pas
voir de différence mesurable (< 0,1 % CPU attribué au SDK). Test `criterion` non
requis ; un `cargo bench` simple sur `frame()` suffit pour vérifier l'absence de lock.

## 1. Phases et ordre

| Phase | Contenu | Dépend de |
|---|---|---|
| 0 | Prérequis desktop (`session.json`, `user_id` dans `/v1/health` et dans la réponse de refresh) — **aucun changement SDK**, déjà spécifié dans `DESKTOP_CONTRACT.md` §1–2 | — |
| 1 | Session de jeu : temps joué + FPS par défaut dans `init` | 0, backend/desktop phase 1 |
| 2 | Achievements : `list`, `unlock`, `is_unlocked` | 1 (la session porte `user_id`/`game_id`) |
| 3 | Amis : `list` avec `online` / `in_game` | 1 (présence « en jeu » vient de la session) |
| 4 | P2P : lobbies, codes d'invitation, invitations entre amis, « rejoindre » depuis le launcher, échange de blobs de connexion | 3 |

Chaque phase se livre seule (PR + bump), avec un desktop plus ancien qui dégrade en
`feature_unavailable` et jamais en échec d'`init`.

## 2. Contrat loopback (ajouts à `DESKTOP_CONTRACT.md`)

Toutes les routes sont sur `http://127.0.0.1:39284`, corps JSON, erreurs
`{ "error", "message", "details?" }` comme aujourd'hui. Le SDK mappe un `404` sans
corps JSON (route inconnue, desktop trop ancien) vers `feature_unavailable`.

### §8 Sessions de jeu (phase 1)

```
POST /v1/games/{public_key}/session/start
→ 200 { "session_id": "uuid", "user_id": "…", "game_id": "…", "fps_sampling": true }
→ 401 not_authenticated · 403 not_owned · 404 game_not_found

POST /v1/games/{public_key}/session/heartbeat
{ "session_id": "uuid", "seconds": 120,
  "samples": [ { "sample_id": "uuid", "taken_at": 1786480000, "fps_avg": 59.8,
                 "window_seconds": 30, "frames": 1794,
                 "resolution": "2560x1440", "graphics_preset": "high" } ] }
→ 200 { "ok": true, "fps_sampling": true }
→ 404 unknown_session (le desktop a expiré la session → le SDK redémarre une session)

POST /v1/games/{public_key}/session/end
{ "session_id": "uuid", "seconds": 1830, "samples": [ … ] }
→ 200 { "ok": true }
```

- `seconds` est **cumulé depuis le début de session** (pas un delta) : un heartbeat
  perdu ou rejoué ne change rien, le desktop garde le max.
- `samples` : les fenêtres closes depuis le dernier heartbeat **acquitté** (en général
  0 ou 1). Chaque échantillon a un `sample_id` généré par le SDK pour que le desktop
  et le backend dédoublonnent un renvoi. `[]` si `frame()` n'est jamais appelé ou si
  `fps_sampling` est `false`.
- `resolution` / `graphics_preset` : optionnels, valeur courante de `set_graphics()`
  au moment de la fenêtre.
- `fps_sampling` reflète le réglage du joueur dans le desktop ; le SDK l'applique dès
  la réponse.
- Le desktop expire une session après 3 heartbeats manqués (180 s) et flush ce qu'il a.

### §9 Achievements (phase 2)

```
GET /v1/games/{public_key}/achievements
→ 200 { "achievements": [ { "key": "first_blood", "title": "…", "description": "…",
         "icon_url": "…|null", "hidden": false, "unlocked_at": "2026-…|null" } ] }

POST /v1/games/{public_key}/achievements/{key}/unlock
→ 200 { "key": "first_blood", "unlocked_at": "…", "already_unlocked": false, "queued": false }
→ 404 unknown_achievement · 403 not_owned
```

`queued: true` = desktop hors ligne, l'unlock est en file et sera synchronisé ; le SDK
le traite comme un succès et met son cache à jour.

### §10 Amis (phase 3)

```
GET /v1/friends
→ 200 { "friends": [ { "user_id": "…", "pseudo": "…", "online": true,
         "playing_game_id": "…|null" } ], "stale": false }
```

`stale: true` = desktop hors ligne, liste issue de son cache. Le SDK dérive
`in_game = playing_game_id == client.game_id()`.

### §11 Lobbies P2P (phase 4)

```
POST /v1/games/{public_key}/lobbies
{ "max_players": 4, "visibility": "friends" | "code" | "friends_and_code", "payload": "<≤4 KiB opaque>" }
→ 200 { "lobby_id": "…", "join_code": "K7P3QX", "expires_at": "…" }

POST /v1/games/{public_key}/lobbies/join      { "join_code": "K7P3QX", "payload": "…" }
POST /v1/games/{public_key}/lobbies/{id}/join { "payload": "…" }          (invité ou ami « rejoindre »)
→ 200 { "lobby_id": "…", "host_user_id": "…", "host_payload": "…",
         "members": [ { "user_id": "…", "pseudo": "…", "payload": "…" } ] }
→ 404 lobby_not_found · 409 lobby_full · 410 lobby_closed

POST /v1/games/{public_key}/lobbies/{id}/invite  { "to_user_id": "…" }   → 200 { "ok": true }
POST /v1/games/{public_key}/lobbies/{id}/leave                             → 200 { "ok": true }
DELETE /v1/games/{public_key}/lobbies/{id}                                  → 200 { "ok": true }  (hôte)

GET  /v1/games/{public_key}/lobbies/events?after={cursor}
→ 200 { "events": [ { "id": …, "type": "invite | member_joined | member_left | lobby_closed",
         "lobby_id": "…", "join_code": "…|null", "from_user_id": "…|null",
         "user_id": "…|null", "pseudo": "…|null", "payload": "…|null" } ], "cursor": … }

GET  /v1/games/{public_key}/launch-context
→ 200 { "join_code": "K7P3QX" | null }
```

- `payload` est le blob de connexion du jeu (adresse publique, ticket de son propre
  netcode, ce qu'il veut). Arcane le transporte, ne le lit pas.
- `launch-context` : quand le launcher démarre le jeu depuis « Rejoindre » d'un ami,
  il stocke le code pour ce lancement et le jeu le récupère au premier appel.
- Le loopback n'a pas de push : le thread de session interroge `/lobbies/events` à
  chaque tick (et à 5 s au lieu de 60 s tant qu'un lobby est ouvert) **uniquement**
  si le jeu a appelé `p2p()` au moins une fois.

## 3. Phase 1 — Session de jeu (temps joué + FPS)

### API publique

```rust
impl ArcaneClient {
    pub fn frame(&self);
    pub fn set_graphics(&self, resolution: &str, preset: &str);
    pub fn session(&self) -> SessionSnapshot;
    pub fn shutdown(self);
}

pub struct SessionSnapshot {
    pub session_id: Option<String>,
    pub tracking: TrackingState,
    pub played_seconds: u64,
    pub fps_sampling: bool,
    pub samples_taken: u32,
    pub last_fps_avg: Option<f32>,
}

pub enum TrackingState { Active, Pending, Disabled }
```

- `Pending` = session locale ouverte, desktop pas encore joignable (les secondes
  s'accumulent quand même). `Disabled` = `ARCANE_OFFLINE_ONLY` ou `DrmDisabled` sans
  compte connecté (`user_id == None` → rien à attribuer).
- `shutdown(self)` envoie `session/end` de façon synchrone (timeout 2 s) puis droppe.
  `Drop` du dernier `Arc` fait la même chose en best-effort. Les moteurs natifs passent
  par `arcane_sdk_shutdown()` qui existe déjà.

### C ABI (additif)

```c
void arcane_sdk_frame(void);
int  arcane_sdk_set_graphics(const char *resolution, const char *preset);
int  arcane_sdk_session_json(char *buf, size_t len);
```

### Fichiers

| Fichier | Changement |
|---|---|
| `src/session.rs` (nouveau) | `SessionInner { sampling: AtomicBool, frames: AtomicU64, started: Instant, state: Mutex<…>, stop: Condvar }`, thread `arcane-session` : tick 60 s → start si nécessaire, sinon heartbeat ; planification des fenêtres (ouvre à T+60 s puis toutes les 5 min si `fps_sampling`, ferme après 30 s, produit un échantillon) ; file `pending_samples` vidée à l'acquittement ; gestion `unknown_session` (redémarrage) ; `end()` |
| `src/desktop.rs` | Factoriser `post_json<T>` / `get_json<T>` + mapping d'erreur commun (`SdkErrorBody` → `SdkError`, 404 sans JSON → `feature_unavailable`). `refresh_ownership_via_desktop` réécrit dessus. |
| `src/client.rs` | `session: Arc<SessionInner>` ; `init` démarre le thread après la vérification de propriété ; `frame`, `set_graphics`, `session`, `shutdown` ; `Clone` partage l'`Arc` |
| `src/error.rs` | `ErrorCode::FeatureUnavailable` (`feature_unavailable`, retryable = false, hint « update Arcane desktop ») |
| `src/ffi.rs` | 3 symboles ci-dessus ; `arcane_sdk_shutdown` appelle `end()` |
| `src/lib.rs` | `pub use session::{SessionSnapshot, TrackingState}` |
| `include/arcane_sdk.h` | regénéré |
| `examples/client_init.rs` | ajoute `frame()` dans une boucle factice et `shutdown()` |
| `documentation/quickstart.mdx` | étape « boucle de rendu : `client.frame()` » ; `Note` mise à jour (un thread de session, pas de revalidation de propriété) |
| `documentation/concepts/client.mdx` | tableau des accesseurs + section « Session » ; `Warning` réécrit |
| `documentation/concepts/session.mdx` (nouveau) | temps joué ; FPS : fenêtres de 30 s / 5 min, option joueur côté desktop, ce que la page store en fait ; états `Active/Pending/Disabled` ; ce qui est perdu hors ligne |
| `documentation/concepts/errors.mdx`, `reference/rust-api.mdx`, `reference/c-abi.mdx`, `docs.json` | nouveaux codes, nouvelles fonctions, nouvelle page dans la nav |
| `DESKTOP_CONTRACT.md` | §8 |
| `Cargo.toml` | `0.5.0` |

### Tests (à la fin de la phase)

- `tests/loopback.rs` : le stub sert `session/start`, `heartbeat`, `end` ; vérifie
  que `init` réussit quand `session/start` renvoie 500 (tracking `Pending`), que
  `unknown_session` redéclenche un `start`, que `shutdown` envoie `end` avec les
  `seconds` cumulés.
- `src/session.rs` : une fenêtre produit un échantillon avec `fps_avg = frames /
  window_seconds`, `frames == 0 → pas d'échantillon`, `fps_sampling: false` → aucune
  fenêtre et `frame()` n'incrémente pas, bascule `true → false` au heartbeat ferme la
  fenêtre en cours sans échantillon, échantillons non acquittés renvoyés avec le même
  `sample_id`, secondes monotones après un `Instant` simulé.
- Test de non-régression existant : `init` sans desktop et ticket valide → toujours
  `Ok`, thread en `Pending`, aucun deep link ouvert.

## 4. Phase 2 — Achievements

### API publique

```rust
impl ArcaneClient { pub fn achievements(&self) -> Achievements<'_>; }

impl Achievements<'_> {
    pub fn list(&self) -> Result<Vec<Achievement>, SdkError>;
    pub fn unlock(&self, key: &str) -> Result<Unlock, SdkError>;
    pub fn is_unlocked(&self, key: &str) -> Option<bool>;
}

pub struct Achievement { pub key: String, pub title: String, pub description: String,
                         pub icon_url: Option<String>, pub hidden: bool,
                         pub unlocked_at: Option<i64> }
pub struct Unlock { pub key: String, pub unlocked_at: i64, pub already_unlocked: bool, pub queued: bool }
```

- `list()` remplit un cache dans le client ; `is_unlocked` lit ce cache (`None` si
  `list` n'a jamais été appelé) ; `unlock` met le cache à jour.
- `unlock` est idempotent côté desktop et backend : le jeu peut l'appeler à chaque fois
  que la condition est vraie sans garde côté jeu.
- Validation de `key` : même charset que la clé publique (`[A-Za-z0-9_.-]{1,64}`) →
  `invalid_argument` avant tout réseau. Nouveau code `invalid_argument` (remplace le
  cas particulier `invalid_public_key` pour les nouvelles entrées, sans le retirer).

### C ABI

```c
int arcane_sdk_achievements_json(char *buf, size_t len);
int arcane_sdk_achievement_unlock(const char *key, char *err_buf, size_t err_len);
int arcane_sdk_achievement_is_unlocked(const char *key);   /* 1, 0, -4 si liste jamais chargée */
```

### Fichiers

`src/achievements.rs` (nouveau), `src/client.rs` (accessor + cache `RwLock<Option<Vec<Achievement>>>`),
`src/error.rs` (`UnknownAchievement`, `InvalidArgument`), `src/ffi.rs`, header,
`documentation/concepts/achievements.mdx` (nouveau), `errors.mdx`, `rust-api.mdx`,
`c-abi.mdx`, `docs.json`, `DESKTOP_CONTRACT.md` §9. Tests dans `tests/loopback.rs`
(200, `queued`, 404 → `unknown_achievement`, route absente → `feature_unavailable`).

## 5. Phase 3 — Amis

### API publique

```rust
impl ArcaneClient { pub fn friends(&self) -> Friends<'_>; }

impl Friends<'_> {
    pub fn list(&self) -> Result<FriendList, SdkError>;
}

pub struct FriendList { pub friends: Vec<Friend>, pub stale: bool }
pub struct Friend { pub user_id: String, pub pseudo: String, pub online: bool, pub in_game: bool }
```

- Pas de cache SDK : le desktop cache déjà (15 s) et pose `stale`.
- Pas d'envoi de demande d'ami, pas d'overlay : ces flux restent dans le launcher.
- `in_game` = `playing_game_id == game_id()` ; la présence « en jeu » du joueur courant
  est posée par le **desktop** au `session/start` de la phase 1, donc gratuite ici.

### C ABI

```c
int arcane_sdk_friends_json(char *buf, size_t len);
```

### Fichiers

`src/friends.rs` (nouveau), `src/client.rs`, `src/ffi.rs`, header,
`documentation/concepts/friends.mdx`, références, `DESKTOP_CONTRACT.md` §10, tests loopback.

## 6. Phase 4 — Lobbies P2P (liens entre joueurs, sans relais)

### Comment font les autres plateformes

Steam, Epic Online Services et Discord font tous la même chose à la base : la
plateforme héberge un **lobby** (un objet « partie » avec un hôte, des membres, une
capacité), on y entre par **invitation d'ami**, par **code** ou par « Rejoindre » depuis
la liste d'amis quand un ami est dans un lobby ouvert, et la plateforme sert de
**boîte aux lettres** pour que les membres s'échangent leurs infos de connexion. Le
transport du trafic de jeu est la partie lourde (Steam Datagram Relay, EOS P2P avec
relais) et elle est optionnelle : beaucoup de jeux utilisent le lobby Steam avec leur
propre netcode.

Arcane commence par la première moitié seulement : lobby, code, invitations,
« rejoindre », échange de blobs. Aucun transport, aucun NAT traversal, aucun relais.

### API publique

```rust
impl ArcaneClient { pub fn p2p(&self) -> P2p<'_>; }

impl P2p<'_> {
    pub fn create_lobby(&self, max_players: u8, visibility: Visibility, payload: &[u8]) -> Result<Lobby, SdkError>;
    pub fn join_by_code(&self, join_code: &str, payload: &[u8]) -> Result<Lobby, SdkError>;
    pub fn join(&self, lobby_id: &str, payload: &[u8]) -> Result<Lobby, SdkError>;
    pub fn invite(&self, lobby_id: &str, to_user_id: &str) -> Result<(), SdkError>;
    pub fn leave(&self, lobby_id: &str) -> Result<(), SdkError>;
    pub fn close(&self, lobby_id: &str) -> Result<(), SdkError>;
    pub fn launch_join_code(&self) -> Option<String>;
    pub fn poll_events(&self) -> Vec<LobbyEvent>;
}

pub enum Visibility { Friends, Code, FriendsAndCode }

pub struct Lobby { pub lobby_id: String, pub join_code: Option<String>, pub host_user_id: String,
                   pub host_payload: Vec<u8>, pub members: Vec<LobbyMember>, pub max_players: u8 }
pub struct LobbyMember { pub user_id: String, pub pseudo: String, pub payload: Vec<u8> }

pub enum LobbyEvent {
    Invite       { lobby_id: String, join_code: Option<String>, from_user_id: String, pseudo: String },
    MemberJoined { lobby_id: String, user_id: String, pseudo: String, payload: Vec<u8> },
    MemberLeft   { lobby_id: String, user_id: String },
    LobbyClosed  { lobby_id: String },
}
```

Scénario type, côté jeu :

```rust
let lobby = client.p2p().create_lobby(4, Visibility::FriendsAndCode, my_endpoint)?;
show_code(lobby.join_code.as_deref());                 // "K7P3QX" à l'écran
client.p2p().invite(&lobby.lobby_id, friend_id)?;      // ou depuis le launcher

for ev in client.p2p().poll_events() {                 // une fois par seconde suffit
    if let LobbyEvent::MemberJoined { payload, .. } = ev { connect_to(payload) }
}

if let Some(code) = client.p2p().launch_join_code() {  // lancé via « Rejoindre »
    let lobby = client.p2p().join_by_code(&code, my_endpoint)?;
    connect_to(lobby.host_payload);
}
```

- `payload` opaque, ≤ 4 KiB, base64 sur le fil. Le jeu y met son adresse publique,
  un ticket de son netcode, ce qu'il veut. Arcane ne l'interprète jamais.
- `join_code` : 6 caractères `[A-HJ-NP-Z2-9]` (sans ambiguïtés), unique parmi les
  lobbies ouverts d'un jeu, généré côté backend.
- Le lobby se ferme quand l'hôte part, `close()`, ou quand sa session de jeu (phase 1)
  expire. Pas de migration d'hôte.
- Présence : un membre d'un lobby `Friends*` non plein apparaît « Playing · Join »
  chez ses amis dans le launcher ; « Rejoindre » lance le jeu avec le code
  (`launch-context`) ou, si le jeu tourne déjà, pousse un `Invite` dans ses events.
- `poll_events()` vide une file remplie par le thread de session ; aucun callback,
  aucun thread supplémentaire.

### C ABI

```c
int arcane_sdk_lobby_create(uint8_t max_players, int visibility, const char *payload_b64, char *buf, size_t len);
int arcane_sdk_lobby_join_code(const char *join_code, const char *payload_b64, char *buf, size_t len);
int arcane_sdk_lobby_join(const char *lobby_id, const char *payload_b64, char *buf, size_t len);
int arcane_sdk_lobby_invite(const char *lobby_id, const char *to_user_id, char *err_buf, size_t err_len);
int arcane_sdk_lobby_leave(const char *lobby_id, char *err_buf, size_t err_len);
int arcane_sdk_lobby_close(const char *lobby_id, char *err_buf, size_t err_len);
int arcane_sdk_launch_join_code(char *buf, size_t len);
int arcane_sdk_lobby_events_json(char *buf, size_t len);   /* vide la file */
```

Les réponses `Lobby` sont écrites en JSON dans `buf` (même convention que
`arcane_sdk_last_error_json`).

### Fichiers

`src/p2p.rs` (nouveau), `src/session.rs` (tick à 5 s tant qu'un lobby est ouvert,
polling des events), `src/client.rs`, `src/error.rs` (`LobbyNotFound`, `LobbyFull`,
`LobbyClosed`), `src/ffi.rs`, header, `documentation/concepts/lobbies.mdx` (nouveau,
avec le scénario ci-dessus), références, `DESKTOP_CONTRACT.md` §11, tests loopback
(create → code → join par code, events consommés une seule fois, `launch-context`).

Reste ouvert, sans bloquer la phase : faut-il un toast « X vous invite » dans le
launcher en plus de l'événement livré au jeu ? Le WS desktop reçoit l'invitation de
toute façon ; c'est une décision d'UI launcher.

## 7. Compatibilité et rollout

| SDK | Desktop | Résultat |
|---|---|---|
| 0.4 | nouveau | Inchangé : le desktop ne reçoit jamais de session. |
| 0.5+ | ancien (sans §8) | `init` OK, `session/start` → 404 → `feature_unavailable`, `TrackingState::Pending` puis réessais silencieux toutes les 60 s. `achievements()` / `friends()` → `Err(feature_unavailable)`. |
| 0.5+ | nouveau | Tout actif. |

Ordre de livraison : backend phase N → desktop phase N → SDK phase N. Le SDK d'une
phase peut être publié avant le desktop (dégradation propre), l'inverse aussi.

## 8. Hors périmètre (explicitement)

- Statistiques / achievements à progression (`set_progress`), leaderboards.
- Cloud saves (annoncé dans le README, plan séparé).
- Overlay in-game, demandes d'ami depuis le jeu, chat.
- Transport réseau, NAT traversal, relais, migration d'hôte, matchmaking public
  (liste de lobbies ouverts à tous).
- Tracking des jeux sans SDK par la durée du process lancé par le desktop.
- Pause du chrono (menu, alt-tab) : le temps de jeu est le temps process.
- Attestation du process appelant sur le loopback (`DESKTOP_CONTRACT.md` §6).

## 9. Règles de réalisation

- Une PR par phase, titre `feat(scope): …`, bump dans la PR.
- Pas de commentaires dans le code ; les explications vont dans la doc Mintlify.
- Lint, `cargo test`, `generate-header.sh` : une seule fois, à la fin de chaque phase.
- `DESKTOP_CONTRACT.md` est mis à jour dans la **même** PR que le code qui en dépend.
