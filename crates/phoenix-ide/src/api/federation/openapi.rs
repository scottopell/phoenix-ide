use std::collections::BTreeMap;

use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    Json,
};
use futures::StreamExt as _;
use phoenix_core::domain::instance_identity::InstanceId;
use phoenix_db::FederationPeerConnection;
use schemars::{
    gen::{SchemaGenerator, SchemaSettings},
    JsonSchema,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    append_bounded_response_chunk, owner, peer_notes, remote_query_client, FederationClientError,
    RemoteAuthoredSendMessageRequest, RemoteQueryDatabaseRequest, RemoteQueryDatabaseResponse,
    RemoteReadConversationRequest, RemoteReadConversationResponse, RemoteReadErrorResponse,
    RemoteResolveErrorResponse, RemoteResolveReferenceRequest, RemoteResolveReferenceResponse,
    RemoteSearchErrorResponse, RemoteSearchRequest, RemoteSearchResponse, RemoteSendMessageRequest,
    RemoteSendMessageResponse,
};
use crate::api::{
    auth::{OwnerAuthenticated, PeerAuthenticated},
    global_read, AppState,
};

pub(crate) const DOCUMENT_BYTES: usize = 512 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchOpenApiRequest {
    /// Required saved peer UUID or unambiguous saved name; never a URL or local fallback.
    pub instance: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReferenceInfo {
    pub title: String,
    pub version: String,
    pub description: String,
    #[serde(flatten)]
    pub additional: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReferenceComponents {
    pub schemas: BTreeMap<String, Value>,
    #[serde(rename = "securitySchemes")]
    pub security_schemes: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub additional: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct FederationApiReference {
    pub openapi: String,
    pub info: ReferenceInfo,
    #[serde(rename = "x-phoenix-instance-id")]
    pub instance_id: InstanceId,
    pub paths: BTreeMap<String, Value>,
    pub components: ReferenceComponents,
    #[serde(flatten)]
    pub additional: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DestinationApiReference {
    pub destination_instance_id: InstanceId,
    pub document: FederationApiReference,
}

pub async fn serve(
    owner: Result<OwnerAuthenticated, StatusCode>,
    peer: Result<PeerAuthenticated, StatusCode>,
    State(state): State<AppState>,
) -> Result<Json<FederationApiReference>, StatusCode> {
    if owner.is_err() && peer.is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = state
        .db
        .instance_id()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(generate(id)))
}

pub async fn fetch(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<FetchOpenApiRequest>, JsonRejection>,
) -> Result<Json<DestinationApiReference>, owner::OwnerFederationError> {
    let request = owner::body(request)?;
    let peer = owner::peer(&state, request.instance).await?;
    fetch_peer(&peer)
        .await
        .map(Json)
        .map_err(owner::remote_error)
}

pub(crate) async fn fetch_peer(
    peer: &FederationPeerConnection,
) -> Result<DestinationApiReference, FederationClientError> {
    let endpoint = peer.base_url.navigation_url("/openapi.json")?;
    let response = remote_query_client(&peer.tls_trust)?
        .get(endpoint.as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .send()
        .await?;
    if response.status() != StatusCode::OK {
        return Err(FederationClientError::RemoteRejected(
            "API reference unavailable".into(),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > DOCUMENT_BYTES as u64)
    {
        return Err(FederationClientError::ResponseTooLarge(DOCUMENT_BYTES));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, DOCUMENT_BYTES)?;
    }
    let reference = decode(&bytes, peer.peer_instance_id)?;
    let rendered = serde_json::to_string(&reference)?;
    if rendered.contains(peer.bearer_credential.expose()) {
        return Err(FederationClientError::RemoteRejected(
            "API reference contains credentials".into(),
        ));
    }
    Ok(reference)
}

fn decode(
    bytes: &[u8],
    destination: InstanceId,
) -> Result<DestinationApiReference, FederationClientError> {
    if bytes.len() > DOCUMENT_BYTES {
        return Err(FederationClientError::ResponseTooLarge(DOCUMENT_BYTES));
    }
    let document: FederationApiReference = serde_json::from_slice(bytes)?;
    if document.instance_id != destination {
        return Err(FederationClientError::DestinationMismatch);
    }
    if document.openapi != "3.1.0" || document.paths.is_empty() {
        return Err(FederationClientError::RemoteRejected(
            "invalid API reference".into(),
        ));
    }
    Ok(DestinationApiReference {
        destination_instance_id: destination,
        document,
    })
}

#[derive(Clone, Copy)]
enum Access {
    Owner,
    Peer,
    Reference,
}
impl Access {
    fn security(self) -> Value {
        match self {
            Self::Owner => json!([{"OwnerBearer":[]}, {"OwnerSession":[]}]),
            Self::Peer => json!([{"PeerBearer":[]}]),
            Self::Reference => json!([{"OwnerBearer":[]}, {"OwnerSession":[]}, {"PeerBearer":[]}]),
        }
    }
}

struct ReferenceBuilder {
    generator: SchemaGenerator,
    paths: BTreeMap<String, Value>,
}
impl ReferenceBuilder {
    fn schema<T: JsonSchema>(&mut self) -> Value {
        serde_json::to_value(self.generator.subschema_for::<T>()).expect("schema is serializable")
    }
    fn operation<ResponseBody: JsonSchema, ErrorBody: JsonSchema>(
        &mut self,
        path: &str,
        method: &str,
        access: Access,
        description: &str,
        request: Option<Value>,
    ) {
        let response = self.schema::<ResponseBody>();
        let error = self.schema::<ErrorBody>();
        let authentication_error = self.schema::<MessageErrorBody>();
        let plain_error = self.schema::<String>();
        let mut operation = json!({
            "summary": description,
            "security": access.security(),
            "responses": {
                "200": {"description":"Success", "content":{"application/json":{"schema":response}}},
                "default": {"description":"Rejected, unavailable, invalid input, or authentication failure. Error details never grant authority.", "content":{"application/json":{"schema":{"anyOf":[error, authentication_error]}}, "text/plain":{"schema":plain_error}}}
            }
        });
        if let Some(schema) = request {
            operation["requestBody"] =
                json!({"required":true,"content":{"application/json":{"schema":schema}}});
        }
        self.paths.entry(path.into()).or_insert_with(|| json!({}))[method] = operation;
    }
    fn post<Request: JsonSchema, ResponseBody: JsonSchema, ErrorBody: JsonSchema>(
        &mut self,
        path: &str,
        access: Access,
        description: &str,
    ) {
        let request = self.schema::<Request>();
        self.operation::<ResponseBody, ErrorBody>(path, "post", access, description, Some(request));
    }
}

#[derive(Serialize, JsonSchema)]
pub(super) struct MessageErrorBody {
    pub error: String,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct SubscriptionSyncReceipt {
    pub synced_once: bool,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn generate(instance_id: InstanceId) -> FederationApiReference {
    let settings =
        SchemaSettings::draft07().with(|s| s.definitions_path = "#/components/schemas/".into());
    let mut b = ReferenceBuilder {
        generator: settings.into_generator(),
        paths: BTreeMap::new(),
    };
    use super::subscriptions::{
        NotificationQuery, OwnerSubscriptionRequest, RemoteUnsubscribeReceipt, RemoteWatchReceipt,
        SubscriptionRequest, SubscriptionResponse,
    };
    use owner::{
        OwnerErrorBody, OwnerQueryRequest, OwnerReadRequest, OwnerResolveRequest,
        OwnerSearchRequest, OwnerSearchResponse, OwnerSendRequest,
    };
    use peer_notes::PeerNotesDocument;
    use phoenix_db::federation_subscriptions::{RemoteNotification, RemoteSubscription};
    use Access::{Owner, Peer, Reference};
    b.operation::<FederationApiReference, MessageErrorBody>("/openapi.json", "get", Reference, "Federation API reference only; owner auth or active directional peer bearer. Not the complete Phoenix API.", None);
    b.post::<FetchOpenApiRequest, DestinationApiReference, OwnerErrorBody>("/api/federation/openapi", Owner, "Fetch the explicit saved destination's reference over verified HTTPS; no redirects or local fallback; 512 KiB maximum.");
    b.post::<OwnerQueryRequest, RemoteQueryDatabaseResponse, OwnerErrorBody>(
        "/api/federation/query-database",
        Owner,
        "Bounded read-only SQL on an explicit saved peer.",
    );
    b.post::<OwnerSearchRequest, OwnerSearchResponse, OwnerErrorBody>(
        "/api/federation/search-conversations",
        Owner,
        "Search an explicit saved peer and return qualified evidence and navigation.",
    );
    b.post::<OwnerReadRequest, global_read::QualifiedConversationRead, OwnerErrorBody>("/api/federation/read-conversation", Owner, "Read a typed destination target; cursor defaults to zero and next_cursor continues the bounded read.");
    b.post::<OwnerResolveRequest, global_read::ResolveGlobalReferenceResponse, OwnerErrorBody>(
        "/api/federation/resolve-reference",
        Owner,
        "Resolve a destination reference with instance-qualified navigation.",
    );
    b.post::<OwnerSendRequest, RemoteSendMessageResponse, OwnerErrorBody>("/api/federation/send-conversation-message", Owner, "Send with idempotent message_id; owner attribution is RemoteApi, not caller-supplied Coordinator authority.");
    b.post::<RemoteQueryDatabaseRequest, RemoteQueryDatabaseResponse, MessageErrorBody>(
        "/api/federation/peer/query-database",
        Peer,
        "Enrolled peer bounded read-only SQL; destination identity must match.",
    );
    b.post::<RemoteSearchRequest, RemoteSearchResponse, RemoteSearchErrorResponse>(
        "/api/federation/peer/search-conversations",
        Peer,
        "Enrolled peer search; caller and destination identities must match.",
    );
    b.post::<RemoteReadConversationRequest, RemoteReadConversationResponse, RemoteReadErrorResponse>("/api/federation/peer/read-conversation", Peer, "Enrolled peer bounded read with explicit cursor and typed target.");
    b.post::<RemoteResolveReferenceRequest, RemoteResolveReferenceResponse, RemoteResolveErrorResponse>("/api/federation/peer/resolve-reference", Peer, "Enrolled peer reference resolution; caller and destination identities must match.");
    b.post::<RemoteSendMessageRequest, RemoteSendMessageResponse, MessageErrorBody>("/api/federation/peer/send-conversation-message", Peer, "Direct peer API send; always RemoteApi from the enrolled bearer. Sender and caller-instance claims are rejected; API callers cannot message upward to the Coordinator.");
    let authored_send = "/api/federation/peer/send-authored-conversation-message";
    b.post::<RemoteAuthoredSendMessageRequest, RemoteSendMessageResponse, MessageErrorBody>(
        authored_send,
        Peer,
        "Server-to-server authored conversation send.",
    );
    let authored_operation = &mut b
        .paths
        .get_mut(authored_send)
        .expect("registered authored send")["post"];
    authored_operation["tags"] = json!(["Server-to-server authored operations"]);
    authored_operation["description"] = json!("Peer bearer authentication binds only the enrolled source instance. This operation trusts that source server to derive ordinary conversation or Coordinator membership from its persisted source conversation and source tool-call context. It is not a direct user/API authorship surface or independently signed user authorship proof. RemoteApi and caller-instance claims are rejected. Only a source-attested Coordinator may message the destination Coordinator. The same directional bearer authorizes both peer send operations; possession of it permits source-server attestations, not proof of an uncompromised source.");
    b.operation::<Vec<phoenix_db::FederationPeerSummary>, OwnerErrorBody>(
        "/api/federation/peers",
        "get",
        Owner,
        "Credential-free saved connection summaries.",
        None,
    );
    let notes = "/api/federation/peers/{instance}/notes";
    b.operation::<PeerNotesDocument, OwnerErrorBody>(
        notes,
        "get",
        Owner,
        "Read opaque connection notes; notes grant no authority.",
        None,
    );
    let request = b.schema::<PeerNotesDocument>();
    b.operation::<PeerNotesDocument, OwnerErrorBody>(
        notes,
        "put",
        Owner,
        "Replace entire opaque notes text (at most 32768 UTF-8 bytes).",
        Some(request),
    );
    let instance_parameter = b.schema::<String>();
    b.paths.get_mut(notes).expect("notes path")["parameters"] = json!([{"name":"instance","in":"path","required":true,"schema":instance_parameter,"description":"Saved instance UUID or unambiguous saved name"}]);
    for (suffix, summary) in [
        ("create", "Future-only bounded terminal-fact subscription."),
        (
            "replay",
            "Replay after a nonnegative sequence cursor; gaps are explicit.",
        ),
        (
            "authorize",
            "Authorize exact fact or gap; this is not delivery acknowledgment.",
        ),
        ("unsubscribe", "Stop an exact subscription binding."),
    ] {
        b.post::<SubscriptionRequest, SubscriptionResponse, MessageErrorBody>(
            &format!("/api/federation/peer/subscriptions/{suffix}"),
            Peer,
            summary,
        );
    }
    b.post::<OwnerSubscriptionRequest, RemoteWatchReceipt, MessageErrorBody>(
        "/api/federation/subscriptions/create",
        Owner,
        "Create saved-peer future-only subscription with explicit instance and subscription_id.",
    );
    b.post::<OwnerSubscriptionRequest, SubscriptionSyncReceipt, MessageErrorBody>(
        "/api/federation/subscriptions/sync-once",
        Owner,
        "One bounded saved-peer replay/authorization/delivery attempt.",
    );
    b.post::<OwnerSubscriptionRequest, RemoteUnsubscribeReceipt, MessageErrorBody>(
        "/api/federation/subscriptions/unsubscribe",
        Owner,
        "Stop local delivery and report remote unsubscribe confirmation separately.",
    );
    b.operation::<Vec<RemoteSubscription>, MessageErrorBody>(
        "/api/federation/subscriptions",
        "get",
        Owner,
        "List saved remote subscriptions (up to 1024).",
        None,
    );
    let notifications = "/api/federation/subscriptions/notifications";
    b.operation::<Vec<RemoteNotification>, MessageErrorBody>(
        notifications,
        "get",
        Owner,
        "Bounded received subscription facts and explicit dispositions.",
        None,
    );
    let object = NotificationQuery::json_schema(&mut b.generator)
        .into_object()
        .object
        .expect("query object");
    let required = object.required;
    let parameters = object
        .properties
        .into_iter()
        .map(|(name, schema)| {
            let required = required.contains(&name);
            json!({"in":"query","name":name,"required":required,"schema":schema})
        })
        .collect();
    b.paths.get_mut(notifications).expect("notifications path")["get"]["parameters"] =
        Value::Array(parameters);
    FederationApiReference {
        openapi: "3.1.0".into(),
        info: ReferenceInfo { additional: BTreeMap::new(), title: "Federation API reference".into(), version: env!("CARGO_PKG_VERSION").into(), description: "Supported federation owner facades, peer operations, and subscriptions. Owner routes use the configured owner bearer password or session; peer operations require an active directional enrollment bearer and reject owner credentials. Documentation alone accepts either channel. With no configured password the existing local owner policy applies. Enrollment secrets and connection credentials are never returned by this reference. Owner operation errors use static codes; peer operation errors use their typed response or authentication error. No generic RPC, URL-based destination, or implicit-local operation is provided.".into() },
        instance_id, paths: b.paths, additional: BTreeMap::new(),
        components: ReferenceComponents {
            additional: BTreeMap::new(),
            schemas: b.generator.definitions().iter().map(|(name,schema)| (name.clone(),serde_json::to_value(schema).expect("schema is serializable"))).collect(),
            security_schemes: BTreeMap::from([
                ("OwnerBearer".into(), json!({"type":"http","scheme":"bearer","description":"Configured owner password, never a peer enrollment token"})),
                ("OwnerSession".into(), json!({"type":"apiKey","in":"cookie","name":"phoenix-auth","description":"Owner session minted by login"})),
                ("PeerBearer".into(), json!({"type":"http","scheme":"bearer","description":"Active directional phx_peer_ enrollment credential; not an owner password"})),
            ]),
        },
    }
}

#[cfg(test)]
mod tests;
