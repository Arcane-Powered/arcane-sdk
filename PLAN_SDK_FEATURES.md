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

C'est tout pour : propriété, temps de jeu, FPS (si le jeu appelle `frame()`), présence
« en jeu » pour les amis. Le reste est opt-in, une ligne par usage :

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
| « Pas de thread, pas de timer » (`concepts/client.mdx`) | **Un** thread de session (heartbeat 60 s). La propriété, elle, n'est toujours jamais revalidée seule. |
| `ArcaneClient: Clone` par copie de valeurs | `Clone` partage la session (`Arc`). La session se termine quand le **dernier** clone est droppé, ou sur `shutdown()`. |
| Le desktop n'est contacté que pour rafraîchir un ticket | Le desktop reçoit aussi les heartbeats, les unlocks, les lectures amis. Le SDK **n'ouvre jamais le deep link** pour ces appels : si le desktop n'est pas là, la fonctionnalité est dégradée, jamais bloquante. |
| Erreurs : 13 codes | + `feature_unavailable` (desktop trop ancien / route 404), `unknown_achievement`, `tracking_unavailable` (réservé au JSON de diagnostic, jamais une `Err` de `init`). `ErrorCode` est `#[non_exhaustive]`, donc additif. |

### Décisions verrouillées

| Décision | Choix |
|---|---|
| Le tracking est-il bloquant pour `init` ? | **Non.** `init` réussit dès que la propriété est confirmée. Le démarrage de session est tenté en arrière-plan et réessayé à chaque tick. |
| Horloge du temps de jeu | `Instant` (monotone). Jamais l'horloge murale, donc insensible à `clock_rollback`. |
| Comptage FPS | `frame()` = `AtomicU64::fetch_add(1, Relaxed)`. Aucun lock, aucune allocation. Sans appel à `frame()`, le heartbeat envoie `fps: null` et le desktop ne remonte pas d'échantillon. |
| Perte de données | Si le desktop n'est jamais joignable pendant toute la session, le temps de jeu de cette session est perdu. Pas de fichier tampon côté SDK : `sdk_server.rs` interdit les fichiers comme bus d'API. |
| `ARCANE_OFFLINE_ONLY` | Désactive aussi le thread de session et tous les appels achievements / amis (`network_required`). |
| Anti-triche | **Hors périmètre.** Un unlock est une requête loopback qu'un process local peut forger (cf. `DESKTOP_CONTRACT.md` §6). La validation de forme et de plausibilité est côté backend. |
| Versioning | `0.5.0` (`feat:`) pour la phase 1. Chaque phase suivante est un `feat:` mineur. Le header C est regénéré par `.github/scripts/generate-header.sh`. |

## 1. Phases et ordre

| Phase | Contenu | Dépend de |
|---|---|---|
| 0 | Prérequis desktop (`session.json`, `user_id` dans `/v1/health` et dans la réponse de refresh) — **aucun changement SDK**, déjà spécifié dans `DESKTOP_CONTRACT.md` §1–2 | — |
| 1 | Session de jeu : temps joué + FPS par défaut dans `init` | 0, backend/desktop phase 1 |
| 2 | Achievements : `list`, `unlock`, `is_unlocked` | 1 (la session porte `user_id`/`game_id`) |
| 3 | Amis : `list` avec `online` / `in_game` | 1 (présence « en jeu » vient de la session) |
| 4 | P2P : invitations + échange de blobs de signalisation entre amis | 3 |

Chaque phase se livre seule (PR + bump), avec un desktop plus ancien qui dégrade en
`feature_unavailable` et jamais en échec d'`init`.

## 2. Contrat loopback (ajouts à `DESKTOP_CONTRACT.md`)

Toutes les routes sont sur `http://127.0.0.1:39284`, corps JSON, erreurs
`{ "error", "message", "details?" }` comme aujourd'hui. Le SDK mappe un `404` sans
corps JSON (route inconnue, desktop trop ancien) vers `feature_unavailable`.

### §8 Sessions de jeu (phase 1)

```
POST /v1/games/{public_key}/session/start
→ 200 { "session_id": "uuid", "user_id": "…", "game_id": "…" }
→ 401 not_authenticated · 403 not_owned · 404 game_not_found

POST /v1/games/{public_key}/session/heartbeat
{ "session_id": "uuid", "seconds": 120, "frames": 7180, "fps_avg": 59.8,
  "resolution": "2560x1440", "graphics_preset": "high" }
→ 200 { "ok": true }
→ 404 unknown_session (le desktop a expiré la session → le SDK redémarre une session)

POST /v1/games/{public_key}/session/end
{ "session_id": "uuid", "seconds": 1830, "frames": …, "fps_avg": … }
→ 200 { "ok": true }
```

- `seconds` est **cumulé depuis le début de session** (pas un delta) : un heartbeat
  perdu ou rejoué ne change rien, le desktop garde le max.
- `frames` et `fps_avg` sont cumulés sur la session ; `fps_avg` = frames / secondes
  pendant lesquelles `frame()` a été appelé au moins une fois. `null` si jamais appelé.
- `resolution` / `graphics_preset` : optionnels, posés par `set_graphics()`.
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

### §11 P2P (phase 4)

```
POST /v1/p2p/invites            { "to_user_id": "…", "payload": "<≤4 KiB opaque>" }
→ 200 { "invite_id": "…", "expires_at": "…" }

POST /v1/p2p/invites/{id}/accept   { "payload": "<≤4 KiB opaque>" }
→ 200 { "ok": true }

GET  /v1/p2p/events?after={cursor}
→ 200 { "events": [ { "id": …, "type": "invite|accepted|declined|expired",
         "invite_id": "…", "from_user_id": "…", "payload": "…|null" } ], "cursor": … }
```

Le loopback n'a pas de push : le thread de session interroge `/v1/p2p/events` à
chaque tick **uniquement** si le jeu a appelé `p2p()` au moins une fois.

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
    pub frames: u64,
    pub fps_avg: Option<f32>,
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
| `src/session.rs` (nouveau) | `SessionInner { frames: AtomicU64, started: Instant, state: Mutex<…>, stop: Condvar }`, thread `arcane-session` : tick 60 s → start si nécessaire, sinon heartbeat ; calcul `fps_avg` ; gestion `unknown_session` (redémarrage) ; `end()` |
| `src/desktop.rs` | Factoriser `post_json<T>` / `get_json<T>` + mapping d'erreur commun (`SdkErrorBody` → `SdkError`, 404 sans JSON → `feature_unavailable`). `refresh_ownership_via_desktop` réécrit dessus. |
| `src/client.rs` | `session: Arc<SessionInner>` ; `init` démarre le thread après la vérification de propriété ; `frame`, `set_graphics`, `session`, `shutdown` ; `Clone` partage l'`Arc` |
| `src/error.rs` | `ErrorCode::FeatureUnavailable` (`feature_unavailable`, retryable = false, hint « update Arcane desktop ») |
| `src/ffi.rs` | 3 symboles ci-dessus ; `arcane_sdk_shutdown` appelle `end()` |
| `src/lib.rs` | `pub use session::{SessionSnapshot, TrackingState}` |
| `include/arcane_sdk.h` | regénéré |
| `examples/client_init.rs` | ajoute `frame()` dans une boucle factice et `shutdown()` |
| `documentation/quickstart.mdx` | étape « boucle de rendu : `client.frame()` » ; `Note` mise à jour (un thread de session, pas de revalidation de propriété) |
| `documentation/concepts/client.mdx` | tableau des accesseurs + section « Session » ; `Warning` réécrit |
| `documentation/concepts/session.mdx` (nouveau) | temps joué, FPS, états `Active/Pending/Disabled`, ce qui est perdu hors ligne |
| `documentation/concepts/errors.mdx`, `reference/rust-api.mdx`, `reference/c-abi.mdx`, `docs.json` | nouveaux codes, nouvelles fonctions, nouvelle page dans la nav |
| `DESKTOP_CONTRACT.md` | §8 |
| `Cargo.toml` | `0.5.0` |

### Tests (à la fin de la phase)

- `tests/loopback.rs` : le stub sert `session/start`, `heartbeat`, `end` ; vérifie
  que `init` réussit quand `session/start` renvoie 500 (tracking `Pending`), que
  `unknown_session` redéclenche un `start`, que `shutdown` envoie `end` avec les
  `seconds` cumulés.
- `src/session.rs` : `fps_avg` sur fenêtre partielle, `frames == 0 → None`, secondes
  monotones après un `Instant` simulé.
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

## 6. Phase 4 — P2P (cadrage, décisions à prendre avant implémentation)

Périmètre proposé : **invitations + échange de blobs de signalisation entre amis**,
transport laissé au jeu. Le SDK ne fait ni NAT traversal ni relais.

```rust
impl ArcaneClient { pub fn p2p(&self) -> P2p<'_>; }

impl P2p<'_> {
    pub fn invite(&self, to_user_id: &str, payload: &[u8]) -> Result<InviteId, SdkError>;
    pub fn accept(&self, invite: &InviteId, payload: &[u8]) -> Result<(), SdkError>;
    pub fn poll_events(&self) -> Vec<P2pEvent>;
}

pub enum P2pEvent {
    Invite   { invite_id: String, from_user_id: String, payload: Vec<u8> },
    Accepted { invite_id: String, from_user_id: String, payload: Vec<u8> },
    Declined { invite_id: String },
    Expired  { invite_id: String },
}
```

- `payload` opaque, ≤ 4 KiB, base64 sur le fil. C'est au jeu d'y mettre une offre
  (adresse publique, SDP, ticket de relais tiers…).
- `poll_events()` vide une file remplie par le thread de session (§2 §11) ; aucun
  callback, aucun thread supplémentaire.
- TTL d'une invitation : 2 min côté backend.

Questions ouvertes, à trancher avant de commencer la phase 4 :

1. « P2P » = uniquement invitations + signalisation (ce plan), ou aussi un transport
   fourni par Arcane (WebRTC data channel, relais TURN) ? Le second point est un
   projet à part entière.
2. Invitations limitées aux amis, ou aussi par code de session (lobby public) ?
3. Faut-il que l'invitation apparaisse dans le launcher (toast « X vous invite ») en
   plus d'être livrée au jeu ? Le WS desktop la reçoit de toute façon.

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
- Tracking des jeux sans SDK par la durée du process lancé par le desktop.
- Pause du chrono (menu, alt-tab) : le temps de jeu est le temps process.
- Attestation du process appelant sur le loopback (`DESKTOP_CONTRACT.md` §6).

## 9. Règles de réalisation

- Une PR par phase, titre `feat(scope): …`, bump dans la PR.
- Pas de commentaires dans le code ; les explications vont dans la doc Mintlify.
- Lint, `cargo test`, `generate-header.sh` : une seule fois, à la fin de chaque phase.
- `DESKTOP_CONTRACT.md` est mis à jour dans la **même** PR que le code qui en dépend.
