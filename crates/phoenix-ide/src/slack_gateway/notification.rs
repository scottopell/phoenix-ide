//! Provider-neutral owner notification command. Credentials stay in the local adapter.
use super::Config;
use phoenix_db::SlackReturn;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Serialize)]
struct Request<'a> {
    version: u8,
    notification_id: &'a str,
    owner: &'a str,
    operation: &'static str,
    text: &'a str,
    url: &'a str,
}
#[derive(Debug, Deserialize)]
pub(super) struct Response {
    version: u8,
    notification_id: String,
    owner: String,
    pub receipt: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum Outcome {
    Submitted,
    Failed { reason: String },
}

#[derive(Debug)]
pub(super) enum NotificationError {
    NotDispatched(String),
    Uncertain(String),
}
impl NotificationError {
    pub fn message(&self) -> &str {
        match self {
            Self::NotDispatched(s) | Self::Uncertain(s) => s,
        }
    }
}
impl From<&str> for NotificationError {
    fn from(message: &str) -> Self {
        Self::Uncertain(message.into())
    }
}

pub(super) fn target_url(c: &Config, output: &SlackReturn) -> Result<String, NotificationError> {
    Ok(if output.published {
        let product = output.product_id.as_deref().ok_or_else(|| {
            NotificationError::NotDispatched("Published notice has no conversation identity".into())
        })?;
        format!("{}/c/{product}", c.phoenix_url.trim_end_matches('/'))
    } else {
        format!(
            "{}/settings/slack#slack-return-{}",
            c.phoenix_url.trim_end_matches('/'),
            output.return_id
        )
    })
}

pub(super) async fn notify(
    output: &SlackReturn,
    command: &[String],
    url: &str,
) -> Result<Response, NotificationError> {
    call(
        command,
        Request {
            version: 1,
            notification_id: &output.return_id,
            owner: &output.owner,
            operation: "submit",
            text: &output.text,
            url,
        },
    )
    .await
}

async fn call(command: &[String], request: Request<'_>) -> Result<Response, NotificationError> {
    let (program, args) = command.split_first().ok_or_else(|| {
        NotificationError::NotDispatched("Missing notification executable".into())
    })?;
    let payload = serde_json::to_vec(&request)
        .map_err(|_| NotificationError::NotDispatched("Invalid notification request".into()))?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            NotificationError::NotDispatched("Unable to launch notification command".into())
        })?;
    let mut stdin = child.stdin.take().ok_or("Missing notification input")?;
    let stdout = child.stdout.take().ok_or("Missing notification output")?;
    let result = tokio::time::timeout(COMMAND_TIMEOUT, async {
        stdin
            .write_all(&payload)
            .await
            .map_err(|_| "Notification input interrupted")?;
        drop(stdin);
        let mut bytes = Vec::new();
        stdout
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Notification output interrupted")?;
        if bytes.len() > 65536 {
            return Err("Notification evidence exceeds its size limit");
        }
        if !child
            .wait()
            .await
            .map_err(|_| "Notification command failed")?
            .success()
        {
            return Err("Notification command did not establish success");
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "Notification command timed out; outcome uncertain")??;
    validate_response(&result, request.notification_id, request.owner)
        .map_err(NotificationError::Uncertain)
}

fn validate_response(bytes: &[u8], notification_id: &str, owner: &str) -> Result<Response, String> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|_| "Invalid notification evidence")?;
    if response.version != 1
        || response.notification_id != notification_id
        || response.owner != owner
        || response.receipt.trim().is_empty()
    {
        return Err("Notification evidence identity does not match the request".into());
    }
    if matches!(&response.outcome, Outcome::Failed { reason } if reason.trim().is_empty()) {
        return Err("Notification failure reason is empty".into());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn command_protocol_reads_stdin_and_requires_successful_bounded_evidence() {
        let request = || Request {
            version: 1,
            notification_id: "notice",
            owner: "UOWNER",
            operation: "submit",
            text: "ready",
            url: "https://phoenix.example/c/fixture",
        };
        let response = r#"{"version":1,"notification_id":"notice","owner":"UOWNER","receipt":"receipt","state":"submitted"}"#;
        let command = |script: String| vec!["/bin/sh".into(), "-c".into(), script];
        let valid = command(format!(
            "read -r input; test -n \"$input\"; printf '%s' '{response}'"
        ));
        assert!(call(&valid, request()).await.is_ok());
        let failed = command(format!("cat >/dev/null; printf '%s' '{response}'; exit 1"));
        assert!(call(&failed, request())
            .await
            .unwrap_err()
            .message()
            .contains("success"));
        let oversized = command("cat >/dev/null; head -c 65537 /dev/zero".into());
        assert!(call(&oversized, request())
            .await
            .unwrap_err()
            .message()
            .contains("size limit"));
    }
    #[test]
    fn notification_evidence_requires_exact_owner_request_and_receipt() {
        let value = json!({"version":1,"notification_id":"notice","owner":"UOWNER","receipt":"receipt","state":"submitted"});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(validate_response(&bytes, "notice", "UOWNER").is_ok());
        assert!(validate_response(&bytes, "other", "UOWNER").is_err());
        assert!(validate_response(&bytes, "notice", "UOTHER").is_err());
        let mut wrong_version = value.clone();
        wrong_version["version"] = json!(2);
        assert!(validate_response(
            &serde_json::to_vec(&wrong_version).unwrap(),
            "notice",
            "UOWNER"
        )
        .is_err());
        let mut missing_receipt = value;
        missing_receipt["receipt"] = json!("");
        assert!(validate_response(
            &serde_json::to_vec(&missing_receipt).unwrap(),
            "notice",
            "UOWNER"
        )
        .is_err());
        for reason in ["", " \n\t", "Provider refused delivery"] {
            let value = json!({"version":1,"notification_id":"notice","owner":"UOWNER","receipt":"receipt","state":"failed","reason":reason});
            assert_eq!(
                validate_response(&serde_json::to_vec(&value).unwrap(), "notice", "UOWNER").is_ok(),
                !reason.trim().is_empty()
            );
        }
    }
}
