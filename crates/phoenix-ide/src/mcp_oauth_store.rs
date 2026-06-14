//! Database-backed `OAuthStore` for the MCP client (REQ-MCP-010, REQ-MCP-012).
//!
//! The MCP manager lives in `phoenix-tools`, which does not depend on
//! `phoenix-db`; this adapter bridges the manager's `OAuthStore` trait onto
//! the `mcp_oauth_registrations` / `mcp_oauth_tokens` tables.

use crate::db::{Database, McpOAuthRegistrationRow, McpOAuthTokenRow};
use crate::tools::mcp::oauth::{OAuthRegistrationRecord, OAuthStore, OAuthTokenRecord};
use async_trait::async_trait;

pub struct DbOAuthStore {
    db: Database,
}

impl DbOAuthStore {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OAuthStore for DbOAuthStore {
    async fn registration(
        &self,
        auth_server: &str,
    ) -> Result<Option<OAuthRegistrationRecord>, String> {
        let row = self
            .db
            .get_mcp_oauth_registration(auth_server)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|row| OAuthRegistrationRecord {
            auth_server: row.auth_server,
            client_id: row.client_id,
            client_secret: row.client_secret,
            token_endpoint_auth_method: row.token_endpoint_auth_method,
            redirect_uri: row.redirect_uri,
        }))
    }

    async fn upsert_registration(&self, record: &OAuthRegistrationRecord) -> Result<(), String> {
        self.db
            .upsert_mcp_oauth_registration(&McpOAuthRegistrationRow {
                auth_server: record.auth_server.clone(),
                client_id: record.client_id.clone(),
                client_secret: record.client_secret.clone(),
                token_endpoint_auth_method: record.token_endpoint_auth_method.clone(),
                redirect_uri: record.redirect_uri.clone(),
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn token(&self, server_name: &str) -> Result<Option<OAuthTokenRecord>, String> {
        let row = self
            .db
            .get_mcp_oauth_token(server_name)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|row| OAuthTokenRecord {
            server_name: row.server_name,
            resource: row.resource_uri,
            scopes: row.scopes.split_whitespace().map(str::to_string).collect(),
            access_token: row.access_token,
            refresh_token: row.refresh_token,
            expires_at: row.expires_at,
        }))
    }

    async fn upsert_token(&self, record: &OAuthTokenRecord) -> Result<(), String> {
        self.db
            .upsert_mcp_oauth_token(&McpOAuthTokenRow {
                server_name: record.server_name.clone(),
                resource_uri: record.resource.clone(),
                scopes: record.scopes.join(" "),
                access_token: record.access_token.clone(),
                refresh_token: record.refresh_token.clone(),
                expires_at: record.expires_at,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete_token(&self, server_name: &str) -> Result<(), String> {
        self.db
            .delete_mcp_oauth_token(server_name)
            .await
            .map_err(|e| e.to_string())
    }
}
