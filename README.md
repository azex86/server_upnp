# server_upnp

Serveur UPnP/DLNA (MediaServer) minimal écrit en Rust : il expose **un
dossier et son arborescence, tels quels**, aux clients du réseau local —
VLC, TV connectées (Samsung, LG, Sony…), et tout lecteur DLNA.

Philosophie : **aucune indexation, aucune base de données, aucun
prétraitement**. Chaque navigation relit simplement le dossier sur le disque ;
un fichier ajouté apparaît immédiatement, sans scan.

## Utilisation

```
server_upnp <port> <dossier> [nom]
```

| Argument  | Rôle                                                                  |
|-----------|-----------------------------------------------------------------------|
| `port`    | Port HTTP d'écoute (ex. `8200`)                                       |
| `dossier` | Racine exposée — seul ce dossier et ses sous-dossiers sont visibles   |
| `nom`     | *(optionnel)* Nom affiché chez les clients ; défaut : nom du dossier  |

Exemples :

```
server_upnp 8200 D:\Videos
server_upnp 8200 D:\Videos "Mes films"
```

Le serveur apparaît ensuite :

- dans **VLC** : *Vue → Liste de lecture* (Ctrl+L), puis *Réseau local →
  Universal Plug'n'Play* ;
- sur une **TV** : dans la liste des sources / serveurs multimédia DLNA.

`Ctrl+C` arrête proprement le serveur (annonces `ssdp:byebye`).

## Compilation

Nécessite [Rust](https://rustup.rs/) (édition 2021).

```
cargo build --release
```

Le binaire est produit dans `target/release/server_upnp` (`.exe` sous
Windows).

## Fonctionnalités

- Découverte **SSDP** sur toutes les interfaces réseau, y compris celles qui
  apparaissent après le démarrage (Wi-Fi qui se connecte, câble branché,
  reprise de veille) — utile sur les machines avec cartes virtuelles
  (WSL, Hyper-V, VPN).
- **ContentDirectory** (Browse) et **ConnectionManager** UPnP, DIDL-Lite
  conforme, pagination stable.
- Streaming HTTP avec requêtes **Range** : l'avance/retour rapide fonctionne
  sur TV et dans VLC. Types MIME déduits de l'extension.
- Tri alphabétique insensible à la casse **et aux accents** (« Été » se
  classe à E), dossiers d'abord.
- Seuls les fichiers audio, vidéo et image sont listés : les `Thumbs.db`,
  `desktop.ini`, sous-titres et autres fichiers annexes ne polluent pas les
  menus des TV. Tous les dossiers restent visibles.
- Identité UPnP stable (UUID dérivé du triplet dossier/port/nom) : les
  clients retrouvent le même serveur d'un lancement à l'autre.

## Notes

- **Pare-feu** : le programme doit être autorisé en entrée (TCP sur le port
  choisi et UDP 1900), sinon les clients ne le découvriront pas. Windows
  propose l'autorisation au premier lancement — accepter pour les réseaux
  privés.
- Le serveur ne transcode pas : la TV ne lit que les formats qu'elle prend
  en charge nativement (VLC lit à peu près tout).
- Développé et testé sous Windows ; le code n'utilise rien de spécifique à
  Windows et devrait fonctionner sous Linux/macOS.

## Licence

[MIT](LICENSE)
