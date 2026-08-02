# server_upnp

Serveur UPnP/DLNA (MediaServer) minimal : il expose un dossier et son
arborescence, telle quelle, aux clients du réseau local (VLC, TV…).
Aucune indexation, aucune base de données, aucun prétraitement : chaque
navigation relit simplement le dossier sur le disque.

## Utilisation

```
server_upnp <port> <dossier> [nom]
```

- `nom` (optionnel) : nom du serveur affiché chez les clients ; par défaut,
  le nom du dossier exposé.

Exemples :

```
server_upnp 8200 D:\Videos
server_upnp 8200 D:\Videos "Mes films"
```

Le serveur apparaît ensuite :

- dans **VLC** : *Vue > Liste de lecture > Réseau local > Universal Plug'n'Play* ;
- sur une **TV** : dans la liste des sources / serveurs multimédia DLNA.

`Ctrl+C` arrête proprement le serveur (annonces `ssdp:byebye`).

## Compilation

```
cargo build --release
```

Le binaire est produit dans `target/release/server_upnp.exe`.

## Notes

- Le pare-feu Windows doit autoriser le programme (TCP sur le port choisi et
  UDP 1900 en entrée), sinon la TV ne découvrira pas le serveur. Windows
  propose l'autorisation au premier lancement.
- Le seek (avance/retour rapide) est pris en charge via les requêtes HTTP
  `Range`.
- Les fichiers sont servis avec leur type MIME déduit de l'extension. Seuls
  les fichiers audio, vidéo et image sont listés (les `Thumbs.db`,
  `desktop.ini`, sous-titres, etc. pollueraient les menus des TV) ; tous les
  dossiers restent visibles.
