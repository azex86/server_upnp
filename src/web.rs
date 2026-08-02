//! Serveur HTTP : description du périphérique, points de contrôle SOAP,
//! abonnements aux événements (factices) et diffusion des fichiers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Router;
use tower::util::ServiceExt;
use tower_http::services::ServeFile;

use crate::{desc, soap, AppState};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/desc.xml", get(device_desc))
        .route(
            "/cd/scpd.xml",
            get(|| async { xml(desc::CD_SCPD.to_string()) }),
        )
        .route(
            "/cm/scpd.xml",
            get(|| async { xml(desc::CM_SCPD.to_string()) }),
        )
        .route("/cd/control", post(cd_control))
        .route("/cm/control", post(cm_control))
        .route("/cd/event", any(event))
        .route("/cm/event", any(event))
        .route("/media/{*path}", get(media))
        .with_state(state)
}

fn xml(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")],
        body,
    )
        .into_response()
}

async fn device_desc(State(st): State<Arc<AppState>>) -> Response {
    xml(desc::device_description(&st))
}

/// Nom de l'action extrait de l'en-tête SOAPACTION
/// (forme : `"urn:...:service:ContentDirectory:1#Browse"`).
fn soap_action_name(headers: &HeaderMap) -> Option<String> {
    let v = headers.get("soapaction")?.to_str().ok()?;
    let v = v.trim().trim_matches('"');
    Some(v.rsplit('#').next().unwrap_or(v).to_string())
}

fn host_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn soap_response(status: StatusCode, body: String) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
        .header("Ext", "")
        .body(Body::from(body))
        .unwrap()
}

async fn cd_control(State(st): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let action = soap_action_name(&headers);
    let host = host_header(&headers);
    let (status, xml_body) = soap::content_directory(st, action, host, body).await;
    soap_response(status, xml_body)
}

async fn cm_control(headers: HeaderMap) -> Response {
    let action = soap_action_name(&headers);
    let (status, xml_body) = soap::connection_manager(action.as_deref());
    soap_response(status, xml_body)
}

static SUB_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Abonnement GENA factice : on accuse réception sans jamais émettre
/// d'événement — suffisant pour les TV qui exigent un SUBSCRIBE réussi.
async fn event(State(st): State<Arc<AppState>>, method: Method) -> Response {
    match method.as_str() {
        "SUBSCRIBE" => {
            let n = SUB_COUNTER.fetch_add(1, Ordering::Relaxed);
            Response::builder()
                .status(StatusCode::OK)
                .header("SID", format!("uuid:{}-{n:04x}", st.uuid))
                .header("TIMEOUT", "Second-1800")
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .unwrap()
        }
        "UNSUBSCRIBE" => StatusCode::OK.into_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

/// Diffusion d'un fichier. ServeFile gère Content-Type, HEAD et surtout les
/// requêtes Range (seek des TV et de VLC) ; on ajoute les en-têtes DLNA.
async fn media(
    State(st): State<Arc<AppState>>,
    UrlPath(path): UrlPath<String>,
    req: Request,
) -> Response {
    let Some(full) = st.resolve(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::metadata(&full).await {
        Ok(m) if m.is_file() => {}
        _ => return StatusCode::NOT_FOUND.into_response(),
    }
    match ServeFile::new(&full).oneshot(req).await {
        Ok(res) => {
            let mut res = res.map(Body::new);
            let h = res.headers_mut();
            h.insert(
                "transfermode.dlna.org",
                HeaderValue::from_static("Streaming"),
            );
            h.insert(
                "contentfeatures.dlna.org",
                HeaderValue::from_static(soap::dlna_features()),
            );
            res
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
