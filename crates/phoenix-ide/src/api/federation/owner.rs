use super::*;
use serde_json::{json, Value};

pub(super) const MAX_OPENAPI_BYTES: usize = 65536;
type OwnerResult<T> = Result<Json<T>, StatusCode>;

fn failure(error: impl std::fmt::Display) -> StatusCode {
    tracing::warn!(%error, "owner federation request failed");
    StatusCode::BAD_GATEWAY
}

async fn peer(
    state: &AppState,
    instance: InstanceId,
) -> Result<phoenix_db::FederationPeerConnection, StatusCode> {
    state
        .db
        .federation_peer_connection(instance)
        .await
        .map_err(failure)?
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerQuery {
    instance: InstanceId,
    sql: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSearch {
    instance: InstanceId,
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerRead {
    instance: InstanceId,
    target: ConversationReferenceTarget,
    cursor: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerResolve {
    instance: InstanceId,
    reference: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSend {
    instance: InstanceId,
    target: RemoteSendTarget,
    message: RemoteMessageText,
    message_id: uuid::Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerInstance {
    instance: InstanceId,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerNotes {
    notes: String,
}
#[derive(Serialize)]
pub struct OwnerSearchResponse {
    destination_instance_id: InstanceId,
    hits: Vec<crate::api::global_read::QualifiedSearchHit>,
}
#[derive(Serialize)]
pub struct OwnerOpenapiResponse {
    destination_instance_id: InstanceId,
    document: Value,
}

pub async fn owner_peers(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
) -> OwnerResult<Vec<phoenix_db::FederationPeerSummary>> {
    state
        .db
        .federation_peer_summaries()
        .await
        .map(Json)
        .map_err(failure)
}
pub async fn owner_get_notes(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(instance): Path<InstanceId>,
) -> OwnerResult<PeerNotes> {
    let notes = state
        .db
        .federation_peer_notes(instance)
        .await
        .map_err(failure)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(PeerNotes { notes }))
}
pub async fn owner_put_notes(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(instance): Path<InstanceId>,
    Json(body): Json<PeerNotes>,
) -> OwnerResult<PeerNotes> {
    if body.notes.len() > 32768 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if !state
        .db
        .set_federation_peer_notes(instance, &body.notes)
        .await
        .map_err(failure)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(body))
}
pub async fn owner_query(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerQuery>,
) -> OwnerResult<RemoteQueryDatabaseResponse> {
    query_remote_database(&state.db, body.instance, &body.sql)
        .await
        .map(Json)
        .map_err(failure)
}
pub async fn owner_search(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerSearch>,
) -> OwnerResult<OwnerSearchResponse> {
    let peer = peer(&state, body.instance).await?;
    let hits = search_remote_conversations(&state.db, &peer, &body.query)
        .await
        .map_err(failure)?;
    Ok(Json(OwnerSearchResponse {
        destination_instance_id: body.instance,
        hits,
    }))
}
pub async fn owner_read(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerRead>,
) -> OwnerResult<crate::api::global_read::QualifiedConversationRead> {
    let peer = peer(&state, body.instance).await?;
    read_remote_conversation(&state.db, &peer, body.target, body.cursor)
        .await
        .map(Json)
        .map_err(failure)
}
pub async fn owner_resolve(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerResolve>,
) -> OwnerResult<crate::api::global_read::ResolveGlobalReferenceResponse> {
    let peer = peer(&state, body.instance).await?;
    resolve_remote_reference(&state.db, &peer, body.reference)
        .await
        .map(Json)
        .map_err(failure)
}
pub async fn owner_send(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerSend>,
) -> OwnerResult<RemoteSendMessageResponse> {
    let peer = peer(&state, body.instance).await?;
    let request = RemoteSendMessageRequest {
        destination_instance_id: body.instance,
        target: body.target,
        message_id: body.message_id,
        message: body.message,
        sender: RemoteSenderAttestation::RemoteApi {},
    };
    send_remote_conversation_message(&state.db, &peer, &request)
        .await
        .map(Json)
        .map_err(failure)
}

fn openapi_document(instance: InstanceId) -> Value {
    let mut paths = serde_json::Map::new();
    for (path, operation, schema) in [
        ("/api/federation/query-database", "post", "OwnerQuery"),
        (
            "/api/federation/search-conversations",
            "post",
            "OwnerSearch",
        ),
        ("/api/federation/read-conversation", "post", "OwnerRead"),
        ("/api/federation/resolve-reference", "post", "OwnerResolve"),
        (
            "/api/federation/send-conversation-message",
            "post",
            "OwnerSend",
        ),
        ("/api/federation/openapi", "post", "OwnerInstance"),
    ] {
        paths.insert(path.into(), json!({(operation): {
            "security": [{"OwnerAuthenticated": []}],
            "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{schema}")}}}},
            "responses": {"200": {"description": "Validated destination response"}, "502": {"description": "Remote request rejected or response invalid; sends are not retried"}}
        }}));
    }
    paths.insert("/api/federation/peers".into(), json!({"get": {"security": [{"OwnerAuthenticated": []}], "responses": {"200": {"description": "Bare array of peer_instance_id, peer_display_name, base_url and created_at; no credentials"}}}}));
    paths.insert("/api/federation/peers/{instance}/notes".into(), json!({
        "parameters": [{"name": "instance", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}],
        "get": {"security": [{"OwnerAuthenticated": []}], "responses": {"200": {"description": "Opaque notes string, including empty"}}},
        "put": {"security": [{"OwnerAuthenticated": []}], "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/PeerNotes"}}}}, "responses": {"200": {"description": "Committed opaque notes string"}}}
    }));
    paths.insert("/openapi.json".into(), json!({"get": {"security": [], "responses": {"200": {"description": "Public federation API reference, at most 65536 bytes"}}}}));
    let mut schemas = serde_json::Map::new();
    for (name, fields) in [
        ("OwnerQuery", vec!["instance", "sql"]),
        ("OwnerSearch", vec!["instance", "query"]),
        ("OwnerRead", vec!["instance", "target", "cursor"]),
        ("OwnerResolve", vec!["instance", "reference"]),
        (
            "OwnerSend",
            vec!["instance", "target", "message", "message_id"],
        ),
        ("OwnerInstance", vec!["instance"]),
        ("PeerNotes", vec!["notes"]),
    ] {
        let properties: serde_json::Map<String, Value> = fields.iter().map(|field| {
            let shape = match *field {
                "instance" | "message_id" => json!({"type": "string", "format": "uuid"}),
                "cursor" => json!({"type": "integer", "minimum": 0}),
                "target" => json!({"$ref": if name == "OwnerSend" { "#/components/schemas/RemoteSendTarget" } else { "#/components/schemas/ConversationReferenceTarget" }}),
                "message" | "notes" => json!({"type": "string", "description": "At most 32768 UTF-8 bytes"}),
                _ => json!({"type": "string"}),
            };
            ((*field).into(), shape)
        }).collect();
        schemas.insert(name.into(), json!({"type": "object", "additionalProperties": false, "required": fields, "properties": properties}));
    }
    for (name, allow_message) in [
        ("ConversationReferenceTarget", true),
        ("RemoteSendTarget", false),
    ] {
        let mut variants = Vec::new();
        for (kind, fields) in [
            ("product_conversation", vec!["product_conversation_id"]),
            ("transcript", vec!["transcript_id"]),
            ("message", vec!["transcript_id", "message_id"]),
            ("coordinator", vec![]),
        ] {
            if kind == "message" && !allow_message {
                continue;
            }
            let mut required = vec!["kind"];
            let mut properties =
                serde_json::Map::from_iter([("kind".into(), json!({"const": kind}))]);
            for field in fields {
                required.push(field);
                properties.insert(field.into(), json!({"type": "string", "minLength": 1}));
            }
            variants.push(json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties}));
        }
        schemas.insert(name.into(), json!({"oneOf": variants}));
    }
    json!({"openapi": "3.1.0", "x-phoenix-instance-id": instance,
    "info": {"title": "Phoenix federation API", "version": env!("CARGO_PKG_VERSION"), "description": "Federation-only owner reference. Peer receiver APIs require separately enrolled PeerBearer credentials; owner credentials do not authorize peer receivers."},
    "paths": paths, "components": {"schemas": schemas, "securitySchemes": {
        "OwnerAuthenticated": {"type": "http", "scheme": "bearer", "description": "Existing local owner password or session cookie"},
        "PeerBearer": {"type": "http", "scheme": "bearer", "description": "Separately enrolled directional peer credential; not owner authentication"}
    }}})
}

pub async fn federation_openapi(State(state): State<AppState>) -> Result<Response, StatusCode> {
    let document = openapi_document(state.db.instance_id().await.map_err(failure)?);
    let bytes = serde_json::to_vec(&document).map_err(failure)?;
    if bytes.len() > MAX_OPENAPI_BYTES {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response())
}

fn validate_openapi(document: &Value, instance: InstanceId) -> Result<(), FederationClientError> {
    let valid = document.get("openapi").and_then(Value::as_str) == Some("3.1.0")
        && document
            .get("x-phoenix-instance-id")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<InstanceId>().ok())
            == Some(instance)
        && ["title", "version", "description"]
            .iter()
            .all(|field| document["info"][*field].is_string())
        && document["paths"]
            .as_object()
            .is_some_and(|paths| !paths.is_empty())
        && document["components"]["schemas"].is_object()
        && document["components"]["securitySchemes"].is_object();
    if valid {
        Ok(())
    } else {
        Err(FederationClientError::RemoteRejected(
            "invalid federation API reference".into(),
        ))
    }
}

pub async fn owner_openapi(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(body): Json<OwnerInstance>,
) -> OwnerResult<OwnerOpenapiResponse> {
    let peer = peer(&state, body.instance).await?;
    let endpoint = peer
        .base_url
        .navigation_url("/openapi.json")
        .map_err(failure)?;
    let response = remote_query_client(&peer.tls_trust)
        .map_err(failure)?
        .get(endpoint.as_url().clone())
        .send()
        .await
        .map_err(failure)?;
    if !response.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk.map_err(failure)?, MAX_OPENAPI_BYTES)
            .map_err(failure)?;
    }
    let document: Value = serde_json::from_slice(&bytes).map_err(failure)?;
    validate_openapi(&document, body.instance).map_err(failure)?;
    Ok(Json(OwnerOpenapiResponse {
        destination_instance_id: body.instance,
        document,
    }))
}

#[cfg(test)]
mod tests;
