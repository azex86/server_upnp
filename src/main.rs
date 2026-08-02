mod desc;
mod soap;
mod ssdp;
mod web;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub const SERVER_ID: &str = "Windows/11 UPnP/1.0 server_upnp/1.0";

pub struct AppState {
    pub port: u16,
    pub root: PathBuf,
    pub friendly_name: String,
    pub uuid: String,
}

impl AppState {
    /// Résout un chemin relatif (composants séparés par '/') sous la racine.
    /// Refuse tout composant qui permettrait de sortir de la racine.
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        let mut p = self.root.clone();
        for comp in rel.split('/') {
            if comp.is_empty() {
                continue;
            }
            if comp == "." || comp == ".." || comp.contains('\\') || comp.contains(':') {
                return None;
            }
            p.push(comp);
        }
        Some(p)
    }
}

/// IP locale utilisée pour joindre `target` (astuce du connect UDP, aucun paquet émis).
pub fn local_ip_towards(target: IpAddr) -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((target, 9)).ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// UUID stable dérivé du couple (racine, port) : le serveur garde la même
/// identité UPnP d'un lancement à l'autre, ce que les TV apprécient.
fn stable_uuid(seed: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h1 = DefaultHasher::new();
    seed.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = DefaultHasher::new();
    (seed, 0xC0FF_EE00u32).hash(&mut h2);
    let b = h2.finish();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16,
        (b >> 48) as u16,
        b & 0xFFFF_FFFF_FFFF
    )
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("Usage : server_upnp <port> <dossier> [nom]");
        eprintln!("  nom : nom du serveur affiché chez les clients (défaut : nom du dossier)");
        eprintln!("Exemple : server_upnp 8200 D:\\Videos \"Mes films\"");
        std::process::exit(2);
    }
    let port: u16 = match args[1].parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Port invalide : {}", args[1]);
            std::process::exit(2);
        }
    };
    let root = match std::fs::canonicalize(&args[2]) {
        Ok(p) => {
            if !p.is_dir() {
                eprintln!("« {} » n'est pas un dossier", args[2]);
                std::process::exit(2);
            }
            p
        }
        Err(e) => {
            eprintln!("Dossier « {} » inaccessible : {e}", args[2]);
            std::process::exit(2);
        }
    };
    let friendly_name = args
        .get(3)
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
        })
        .unwrap_or_else(|| "Fichiers".to_string());
    // Le nom fait partie de l'identité : un serveur renommé apparaît comme un
    // nouveau périphérique, sinon les TV gardent l'ancien nom en cache.
    let uuid = stable_uuid(&format!("{}|{}|{}", root.display(), port, friendly_name));

    let state = Arc::new(AppState {
        port,
        root,
        friendly_name,
        uuid,
    });

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Impossible d'écouter sur le port {port} : {e}");
            std::process::exit(1);
        }
    };

    let ip = local_ip_towards(IpAddr::from([239, 255, 255, 250]))
        .map(|i| i.to_string())
        .unwrap_or_else(|| "<ip-locale>".to_string());
    println!("Serveur UPnP « {} »", state.friendly_name);
    println!("  Racine      : {}", state.root.display());
    println!("  Description : http://{ip}:{port}/desc.xml");
    println!("Annonces SSDP actives — le serveur apparaît dans VLC (Périphériques réseau > Universal Plug'n'Play) et sur la TV.");
    println!("Ctrl+C pour arrêter.");

    tokio::spawn(ssdp::run(state.clone()));

    let app = web::router(state.clone());
    tokio::select! {
        res = axum::serve(listener, app) => {
            if let Err(e) = res {
                eprintln!("Erreur du serveur HTTP : {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nArrêt : envoi des annonces ssdp:byebye…");
            ssdp::send_byebye(&state).await;
        }
    }
}
