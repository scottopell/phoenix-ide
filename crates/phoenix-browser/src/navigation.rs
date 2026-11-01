use async_tungstenite::tungstenite::Message;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::{BrowserError, BrowserSession};

fn navigation_error(error: impl std::fmt::Display) -> BrowserError {
    BrowserError::OperationFailed(error.to_string())
}

fn same_url(current: &str, requested: &str) -> bool {
    matches!((url::Url::parse(current), url::Url::parse(requested)), (Ok(current), Ok(requested)) if current == requested)
}

struct NavigationConnection {
    socket: crate::cdp::Socket,
    session: String,
    next_id: u32,
    root: Option<String>,
    loaded: HashSet<String>,
}

impl NavigationConnection {
    async fn receive(&mut self) -> Result<Value, BrowserError> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| navigation_error("navigation connection ended"))?
                .map_err(navigation_error)?;
            match message {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).map_err(navigation_error)?;
                    if value["sessionId"] == self.session
                        && value["method"] == "Page.lifecycleEvent"
                        && value["params"]["name"] == "load"
                        && value["params"]["frameId"].as_str() == self.root.as_deref()
                    {
                        if let Some(loader) = value["params"]["loaderId"].as_str() {
                            self.loaded.insert(loader.to_owned());
                        }
                    }
                    return Ok(value);
                }
                Message::Close(_) => return Err(navigation_error("navigation connection closed")),
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }

    async fn command(&mut self, method: &str, params: Value) -> Result<Value, BrowserError> {
        let id = self.next_id;
        self.next_id += 1;
        self.socket
            .send(Message::Text(
                json!({"id":id,"sessionId":self.session,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(navigation_error)?;
        loop {
            let response = self.receive().await?;
            if response["id"] != id {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(navigation_error(error));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| navigation_error("missing navigation response"));
        }
    }
}

impl BrowserSession {
    /// Navigate or reload the current URL, retaining same-document fragment semantics.
    ///
    /// # Errors
    /// Returns browser protocol, navigation, or readiness errors. The caller owns the deadline.
    pub async fn navigate(&self, requested: &str) -> Result<(), BrowserError> {
        let (socket, session) = crate::cdp::connect_target(
            self.snapshot_websocket_address(),
            self.page.target_id(),
            8 * 1024 * 1024,
        )
        .await
        .map_err(navigation_error)?;
        let mut connection = NavigationConnection {
            socket,
            session,
            next_id: 2,
            root: None,
            loaded: HashSet::new(),
        };
        connection.command("Page.enable", json!({})).await?;
        let tree = connection.command("Page.getFrameTree", json!({})).await?;
        let frame = &tree["frameTree"]["frame"];
        connection.root = Some(
            frame["id"]
                .as_str()
                .ok_or_else(|| navigation_error("root frame is missing"))?
                .to_owned(),
        );
        let previous_loader = frame["loaderId"]
            .as_str()
            .ok_or_else(|| navigation_error("root loader is missing"))?
            .to_owned();
        let current_url = frame["url"]
            .as_str()
            .ok_or_else(|| navigation_error("root URL is missing"))?;
        let current_url = format!(
            "{current_url}{}",
            frame["urlFragment"].as_str().unwrap_or("")
        );
        let reload = same_url(&current_url, requested);
        connection
            .command("Page.setLifecycleEventsEnabled", json!({"enabled":true}))
            .await?;
        connection.loaded.clear();
        let expected_loader = if reload {
            connection
                .command("Page.reload", json!({"loaderId":previous_loader}))
                .await?;
            None
        } else {
            let result = connection
                .command("Page.navigate", json!({"url":requested}))
                .await?;
            if result["isDownload"] == true {
                return Err(navigation_error("Navigation initiated a download; its completion and saved path are unavailable. Do not replay the navigation automatically."));
            }
            if let Some(error) = result["errorText"].as_str() {
                return Err(navigation_error(error));
            }
            let Some(loader) = result["loaderId"].as_str() else {
                return Ok(());
            };
            Some(loader.to_owned())
        };
        loop {
            if connection.loaded.iter().any(|loaded| {
                expected_loader
                    .as_ref()
                    .map_or_else(|| loaded != &previous_loader, |expected| loaded == expected)
            }) {
                let tree = connection.command("Page.getFrameTree", json!({})).await?;
                let frame = &tree["frameTree"]["frame"];
                if let Some(url) = frame["unreachableUrl"]
                    .as_str()
                    .filter(|url| !url.is_empty())
                {
                    return Err(navigation_error(format!(
                        "Failed to load {url}: Chrome loaded an error document"
                    )));
                }
                return Ok(());
            }
            connection.receive().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::same_url;

    #[test]
    fn current_url_comparison_uses_normalized_urls() {
        for (current, requested, same) in [
            ("http://example.com/", "http://EXAMPLE.com:80", true),
            ("http://example.com/#one", "http://example.com/#two", false),
            ("http://example.com/#one", "http://example.com/", false),
            ("about:blank", "about:blank", true),
            ("data:text/html,hello", "data:text/html,hello", true),
            ("http://example.com/", "http://example.com/other", false),
            ("http://example.com/", "not a URL", false),
        ] {
            assert_eq!(same_url(current, requested), same);
        }
    }
}
