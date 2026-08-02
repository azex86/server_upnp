//! Traitement SOAP : ContentDirectory (Browse) et ConnectionManager.
//!
//! Aucune indexation : chaque Browse relit le dossier demandé sur le disque.
//! Les identifiants d'objets encodent directement le chemin relatif
//! (percent-encoding), la racine étant l'ID « 0 » imposé par UPnP.

use std::sync::Arc;

use axum::http::StatusCode;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::AppState;

const CD_SERVICE: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
const CM_SERVICE: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";

/// Caractères conservés tels quels dans les IDs et URLs ('/' garde la
/// structure de l'arborescence lisible).
const ID_CHARS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// 4e champ du protocolInfo DLNA : seek par plage d'octets autorisé, streaming.
pub fn dlna_features() -> &'static str {
    "DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"
}

pub const PROTOCOL_INFO: &str = "http-get:*:audio/mpeg:*,http-get:*:audio/mp4:*,http-get:*:audio/x-wav:*,http-get:*:audio/flac:*,http-get:*:audio/ogg:*,http-get:*:video/mp4:*,http-get:*:video/x-matroska:*,http-get:*:video/x-msvideo:*,http-get:*:video/mpeg:*,http-get:*:video/quicktime:*,http-get:*:video/webm:*,http-get:*:image/jpeg:*,http-get:*:image/png:*,http-get:*:image/gif:*,http-get:*:*:*";

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// ID d'objet UPnP pour un chemin relatif. Le préfixe '/' évite toute
/// collision avec l'ID racine « 0 » (cas d'un dossier nommé « 0 »).
fn id_for(rel: &str) -> String {
    if rel.is_empty() {
        "0".to_string()
    } else {
        format!("/{}", utf8_percent_encode(rel, ID_CHARS))
    }
}

fn rel_for_id(id: &str) -> Option<String> {
    if id == "0" || id.is_empty() {
        return Some(String::new());
    }
    let raw = id.strip_prefix('/').unwrap_or(id);
    percent_decode_str(raw)
        .decode_utf8()
        .ok()
        .map(|c| c.into_owned())
}

fn parent_of(rel: &str) -> String {
    match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    }
}

/// Extrait la valeur texte d'un argument SOAP (`<Tag>valeur</Tag>`).
/// Les requêtes des clients UPnP n'ont ni attributs ni namespaces sur les
/// arguments : une extraction textuelle suffit.
fn arg(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    loop {
        let start = body[from..].find(&open)? + from;
        let after = &body[start + open.len()..];
        let next = after.chars().next()?;
        if next != '>' && next != ' ' && next != '/' {
            from = start + open.len();
            continue;
        }
        let gt = after.find('>')?;
        if after[..gt].ends_with('/') {
            return Some(String::new());
        }
        let rest = &after[gt + 1..];
        let end = rest.find(&format!("</{tag}"))?;
        return Some(unescape_xml(&rest[..end]));
    }
}

fn envelope(service: &str, action: &str, inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><u:{action}Response xmlns:u=\"{service}\">{inner}</u:{action}Response></s:Body>\
         </s:Envelope>"
    )
}

fn fault(code: u32, descr: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>\
         <detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">\
         <errorCode>{code}</errorCode><errorDescription>{}</errorDescription>\
         </UPnPError></detail></s:Fault></s:Body></s:Envelope>",
        escape_xml(descr)
    )
}

/// Seuls les fichiers audio/vidéo/image sont listés (comportement minidlna) :
/// les Thumbs.db, desktop.ini, .srt, .txt… pollueraient les menus des TV.
fn is_media_file(name: &str) -> bool {
    mime_guess::from_path(name)
        .first()
        .map(|m| matches!(m.type_().as_str(), "audio" | "video" | "image"))
        .unwrap_or(false)
}

/// Clé de tri : minuscules + repli des diacritiques latins, sans quoi la
/// comparaison par code points classe « Été » ou « À bout de souffle »
/// après la lettre Z.
fn sort_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.to_lowercase().chars() {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => out.push('a'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'î' | 'ï' | 'í' | 'ì' => out.push('i'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => out.push('o'),
            'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
            'ç' => out.push('c'),
            'ñ' => out.push('n'),
            'ý' | 'ÿ' => out.push('y'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            _ => out.push(c),
        }
    }
    out
}

fn upnp_class(mime: &str) -> &'static str {
    if mime.starts_with("video/") {
        "object.item.videoItem"
    } else if mime.starts_with("audio/") {
        "object.item.audioItem.musicTrack"
    } else if mime.starts_with("image/") {
        "object.item.imageItem.photo"
    } else {
        "object.item"
    }
}

fn container_didl(id: &str, parent: &str, title: &str) -> String {
    format!(
        "<container id=\"{id}\" parentID=\"{parent}\" restricted=\"1\" searchable=\"0\">\
         <dc:title>{}</dc:title>\
         <upnp:class>object.container.storageFolder</upnp:class>\
         </container>",
        escape_xml(title)
    )
}

fn item_didl(base: &str, id: &str, parent: &str, name: &str, rel: &str, size: u64) -> String {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let mime = mime.essence_str();
    let url = format!("{base}/media/{}", utf8_percent_encode(rel, ID_CHARS));
    format!(
        "<item id=\"{id}\" parentID=\"{parent}\" restricted=\"1\">\
         <dc:title>{}</dc:title>\
         <upnp:class>{}</upnp:class>\
         <res size=\"{size}\" protocolInfo=\"http-get:*:{mime}:{}\">{}</res>\
         </item>",
        escape_xml(name),
        upnp_class(mime),
        dlna_features(),
        escape_xml(&url)
    )
}

struct Child {
    name: String,
    rel: String,
    is_dir: bool,
    size: u64,
    key: String,
}

async fn browse(st: &AppState, base: &str, body: &str) -> Result<String, (u32, &'static str)> {
    let object_id = arg(body, "ObjectID").ok_or((402u32, "Invalid Args"))?;
    let flag = arg(body, "BrowseFlag").unwrap_or_else(|| "BrowseDirectChildren".to_string());
    let start: usize = arg(body, "StartingIndex")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let count: usize = arg(body, "RequestedCount")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);

    let rel = rel_for_id(object_id.trim()).ok_or((701u32, "No such object"))?;
    let full = st.resolve(&rel).ok_or((701u32, "No such object"))?;
    let meta = tokio::fs::metadata(&full)
        .await
        .map_err(|_| (701u32, "No such object"))?;

    let (inner_didl, returned, total) = if flag == "BrowseMetadata" {
        let entry = if rel.is_empty() {
            container_didl("0", "-1", &st.friendly_name)
        } else {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let pid = id_for(&parent_of(&rel));
            if meta.is_dir() {
                container_didl(&id_for(&rel), &pid, &name)
            } else {
                item_didl(base, &id_for(&rel), &pid, &name, &rel, meta.len())
            }
        };
        (entry, 1, 1)
    } else {
        if !meta.is_dir() {
            return Err((701, "No such object"));
        }
        let mut children: Vec<Child> = Vec::new();
        let mut rd = tokio::fs::read_dir(&full)
            .await
            .map_err(|_| (701u32, "No such object"))?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let Ok(md) = e.metadata().await else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if !md.is_dir() && !is_media_file(&name) {
                continue;
            }
            let rel_child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            children.push(Child {
                key: sort_key(&name),
                name,
                rel: rel_child,
                is_dir: md.is_dir(),
                size: md.len(),
            });
        }
        // Ordre stable (dossiers d'abord, puis alphabétique) pour que la
        // pagination des clients reste cohérente entre deux requêtes.
        children.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.key.cmp(&b.key))
                .then_with(|| a.name.cmp(&b.name))
        });
        let total = children.len();
        let end = if count == 0 {
            total
        } else {
            (start.saturating_add(count)).min(total)
        };
        let slice: &[Child] = if start < total {
            &children[start..end]
        } else {
            &[]
        };
        let pid = id_for(&rel);
        let mut out = String::new();
        for c in slice {
            if c.is_dir {
                out.push_str(&container_didl(&id_for(&c.rel), &pid, &c.name));
            } else {
                out.push_str(&item_didl(
                    base,
                    &id_for(&c.rel),
                    &pid,
                    &c.name,
                    &c.rel,
                    c.size,
                ));
            }
        }
        (out, slice.len(), total)
    };

    let didl = format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">{inner_didl}</DIDL-Lite>"
    );
    let inner = format!(
        "<Result>{}</Result><NumberReturned>{returned}</NumberReturned>\
         <TotalMatches>{total}</TotalMatches><UpdateID>1</UpdateID>",
        escape_xml(&didl)
    );
    Ok(envelope(CD_SERVICE, "Browse", &inner))
}

fn base_url(st: &AppState, host: Option<String>) -> String {
    match host {
        Some(h) if !h.is_empty() => format!("http://{h}"),
        _ => {
            let ip = crate::local_ip_towards(std::net::IpAddr::from([239, 255, 255, 250]))
                .map(|i| i.to_string())
                .unwrap_or_else(|| "127.0.0.1".to_string());
            format!("http://{}:{}", ip, st.port)
        }
    }
}

pub async fn content_directory(
    st: Arc<AppState>,
    action: Option<String>,
    host: Option<String>,
    body: String,
) -> (StatusCode, String) {
    let base = base_url(&st, host);
    match action.as_deref() {
        Some("Browse") => match browse(&st, &base, &body).await {
            Ok(xml) => (StatusCode::OK, xml),
            Err((code, msg)) => (StatusCode::INTERNAL_SERVER_ERROR, fault(code, msg)),
        },
        Some("GetSearchCapabilities") => (
            StatusCode::OK,
            envelope(
                CD_SERVICE,
                "GetSearchCapabilities",
                "<SearchCaps></SearchCaps>",
            ),
        ),
        Some("GetSortCapabilities") => (
            StatusCode::OK,
            envelope(CD_SERVICE, "GetSortCapabilities", "<SortCaps></SortCaps>"),
        ),
        Some("GetSystemUpdateID") => (
            StatusCode::OK,
            envelope(CD_SERVICE, "GetSystemUpdateID", "<Id>1</Id>"),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            fault(401, "Invalid Action"),
        ),
    }
}

pub fn connection_manager(action: Option<&str>) -> (StatusCode, String) {
    match action {
        Some("GetProtocolInfo") => (
            StatusCode::OK,
            envelope(
                CM_SERVICE,
                "GetProtocolInfo",
                &format!("<Source>{PROTOCOL_INFO}</Source><Sink></Sink>"),
            ),
        ),
        Some("GetCurrentConnectionIDs") => (
            StatusCode::OK,
            envelope(
                CM_SERVICE,
                "GetCurrentConnectionIDs",
                "<ConnectionIDs>0</ConnectionIDs>",
            ),
        ),
        Some("GetCurrentConnectionInfo") => (
            StatusCode::OK,
            envelope(
                CM_SERVICE,
                "GetCurrentConnectionInfo",
                "<RcsID>-1</RcsID><AVTransportID>-1</AVTransportID>\
                 <ProtocolInfo></ProtocolInfo><PeerConnectionManager></PeerConnectionManager>\
                 <PeerConnectionID>-1</PeerConnectionID><Direction>Output</Direction>\
                 <Status>OK</Status>",
            ),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            fault(401, "Invalid Action"),
        ),
    }
}
