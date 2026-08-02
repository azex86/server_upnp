//! Découverte SSDP : réponse aux M-SEARCH et annonces NOTIFY périodiques.
//!
//! Le groupe multicast est rejoint sur TOUTES les interfaces IPv4 (et pas
//! seulement l'interface par défaut, qui peut être une carte virtuelle
//! WSL/Hyper-V) ; les annonces sont émises interface par interface, chacune
//! avec l'URL LOCATION qui lui correspond.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::AppState;

const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const MAX_AGE: u32 = 1800;
/// Période de vérification des interfaces ; les annonces alive partent
/// toutes les `ALIVE_EVERY` itérations (30 s × 20 = 10 min).
const CHECK_PERIOD: Duration = Duration::from_secs(30);
const ALIVE_EVERY: u32 = 20;

/// Les cibles annoncées : (NT/ST, USN).
fn targets(uuid: &str) -> Vec<(String, String)> {
    let u = format!("uuid:{uuid}");
    vec![
        ("upnp:rootdevice".to_string(), format!("{u}::upnp:rootdevice")),
        (u.clone(), u.clone()),
        (
            "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            format!("{u}::urn:schemas-upnp-org:device:MediaServer:1"),
        ),
        (
            "urn:schemas-upnp-org:service:ContentDirectory:1".to_string(),
            format!("{u}::urn:schemas-upnp-org:service:ContentDirectory:1"),
        ),
        (
            "urn:schemas-upnp-org:service:ConnectionManager:1".to_string(),
            format!("{u}::urn:schemas-upnp-org:service:ConnectionManager:1"),
        ),
    ]
}

/// Adresses IPv4 locales utilisables (ni loopback, ni link-local 169.254.x).
fn ipv4_interfaces() -> Vec<Ipv4Addr> {
    let mut ips = Vec::new();
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return ips;
    };
    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        if let IpAddr::V4(ip) = iface.addr.ip() {
            if !ip.is_link_local() && !ips.contains(&ip) {
                ips.push(ip);
            }
        }
    }
    ips
}

fn header_value<'a>(msg: &'a str, name: &str) -> Option<&'a str> {
    for line in msg.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim());
            }
        }
    }
    None
}

/// Socket UDP 1900 en réutilisation d'adresse (le service « SSDP Discovery »
/// de Windows occupe déjà ce port).
fn make_ssdp_socket() -> std::io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    let addr: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, SSDP_PORT));
    sock.bind(&addr.into())?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

/// Rejoint le groupe multicast sur chaque interface pas encore couverte.
/// Renvoie true si au moins une nouvelle interface a été ajoutée.
fn join_new_interfaces(sock: &UdpSocket, joined: &mut HashSet<Ipv4Addr>) -> bool {
    let mut added = false;
    for ip in ipv4_interfaces() {
        if joined.contains(&ip) {
            continue;
        }
        match sock.join_multicast_v4(SSDP_ADDR, ip) {
            Ok(()) => {
                joined.insert(ip);
                added = true;
            }
            // « Déjà membre » : l'OS a conservé l'adhésion, c'est un succès.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                joined.insert(ip);
            }
            Err(e) => {
                eprintln!("SSDP : échec du join multicast sur {ip} : {e}");
            }
        }
    }
    added
}

/// Tâche principale SSDP : écoute des M-SEARCH + annonces périodiques.
pub async fn run(st: Arc<AppState>) {
    let listen = match make_ssdp_socket().and_then(UdpSocket::from_std) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SSDP indisponible (UDP 1900) : {e} — le serveur reste joignable par son URL directe.");
            return;
        }
    };
    let listen = Arc::new(listen);
    let mut joined: HashSet<Ipv4Addr> = HashSet::new();
    join_new_interfaces(&listen, &mut joined);
    if joined.is_empty() {
        eprintln!("SSDP : aucune interface réseau utilisable pour le multicast.");
    }

    tokio::spawn(respond_loop(st.clone(), listen.clone()));

    let Ok(send_sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return;
    };
    let _ = send_sock.set_multicast_ttl_v4(2);
    let _ = send_sock.set_multicast_loop_v4(true);

    // Rafale initiale d'annonces (les paquets UDP peuvent se perdre).
    for _ in 0..3 {
        send_alive_all(&send_sock, &st).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let mut tick: u32 = 0;
    loop {
        tokio::time::sleep(CHECK_PERIOD).await;
        tick += 1;
        // Purge des interfaces disparues : Windows perd l'adhésion multicast
        // quand une interface tombe (veille, câble débranché). Si la même IP
        // revient ensuite, il faut re-joindre le groupe, sinon plus aucun
        // M-SEARCH n'est reçu sur cette interface.
        let current: HashSet<Ipv4Addr> = ipv4_interfaces().into_iter().collect();
        joined.retain(|ip| {
            if current.contains(ip) {
                true
            } else {
                let _ = listen.leave_multicast_v4(SSDP_ADDR, *ip);
                false
            }
        });
        // Une interface vient d'apparaître (câble branché, Wi-Fi connecté…) :
        // on la couvre et on s'annonce immédiatement dessus.
        let new_iface = join_new_interfaces(&listen, &mut joined);
        if new_iface || tick % ALIVE_EVERY == 0 {
            send_alive_all(&send_sock, &st).await;
        }
    }
}

/// Répond aux requêtes M-SEARCH des clients (VLC, TV…).
async fn respond_loop(st: Arc<AppState>, sock: Arc<UdpSocket>) {
    let mut buf = [0u8; 2048];
    loop {
        let Ok((n, src)) = sock.recv_from(&mut buf).await else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&buf[..n]) else {
            continue;
        };
        if !text.starts_with("M-SEARCH") {
            continue;
        }
        let Some(st_query) = header_value(text, "ST") else {
            continue;
        };
        // L'IP annoncée dans LOCATION doit être joignable depuis le client :
        // on prend celle de l'interface qui mène vers lui.
        let ip = crate::local_ip_towards(src.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let location = format!("http://{}:{}/desc.xml", ip, st.port);
        let date = httpdate::fmt_http_date(std::time::SystemTime::now());
        for (nt, usn) in targets(&st.uuid) {
            if st_query != "ssdp:all" && st_query != nt {
                continue;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 CACHE-CONTROL: max-age={MAX_AGE}\r\n\
                 DATE: {date}\r\n\
                 EXT:\r\n\
                 LOCATION: {location}\r\n\
                 SERVER: {}\r\n\
                 ST: {nt}\r\n\
                 USN: {usn}\r\n\
                 CONTENT-LENGTH: 0\r\n\r\n",
                crate::SERVER_ID
            );
            let _ = sock.send_to(resp.as_bytes(), src).await;
        }
    }
}

/// Émet les annonces ssdp:alive sur chaque interface, avec l'URL LOCATION
/// propre à chacune.
async fn send_alive_all(sock: &UdpSocket, st: &AppState) {
    let dst = SocketAddr::from((SSDP_ADDR, SSDP_PORT));
    for ip in ipv4_interfaces() {
        if socket2::SockRef::from(sock).set_multicast_if_v4(&ip).is_err() {
            continue;
        }
        let location = format!("http://{}:{}/desc.xml", ip, st.port);
        for (nt, usn) in targets(&st.uuid) {
            let msg = format!(
                "NOTIFY * HTTP/1.1\r\n\
                 HOST: 239.255.255.250:1900\r\n\
                 CACHE-CONTROL: max-age={MAX_AGE}\r\n\
                 LOCATION: {location}\r\n\
                 NT: {nt}\r\n\
                 NTS: ssdp:alive\r\n\
                 SERVER: {}\r\n\
                 USN: {usn}\r\n\r\n",
                crate::SERVER_ID
            );
            let _ = sock.send_to(msg.as_bytes(), dst).await;
        }
    }
}

/// Annonce la disparition du serveur (à l'arrêt), sur chaque interface.
pub async fn send_byebye(st: &AppState) {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return;
    };
    let _ = sock.set_multicast_ttl_v4(2);
    let dst = SocketAddr::from((SSDP_ADDR, SSDP_PORT));
    for _ in 0..2 {
        for ip in ipv4_interfaces() {
            if socket2::SockRef::from(&sock).set_multicast_if_v4(&ip).is_err() {
                continue;
            }
            for (nt, usn) in targets(&st.uuid) {
                let msg = format!(
                    "NOTIFY * HTTP/1.1\r\n\
                     HOST: 239.255.255.250:1900\r\n\
                     NT: {nt}\r\n\
                     NTS: ssdp:byebye\r\n\
                     USN: {usn}\r\n\r\n"
                );
                let _ = sock.send_to(msg.as_bytes(), dst).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
