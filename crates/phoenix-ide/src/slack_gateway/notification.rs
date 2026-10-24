//! Provider-neutral owner notification command. Credentials stay in the local adapter.
use super::Config;
use phoenix_db::{SlackNotificationObservation, SlackReturn};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Serialize)]
struct Request<'a> {
    version: u8,
    notification_id: &'a str,
    owner: &'a str,
    #[serde(flatten)]
    operation: Operation<'a>,
}
#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Operation<'a> {
    Submit { text: &'a str, url: &'a str },
    Inspect { receipt: &'a str },
}
#[derive(Debug, Deserialize)]
struct Response {
    version: u8,
    notification_id: String,
    owner: String,
    receipt: String,
    #[serde(flatten)]
    outcome: Outcome,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Outcome {
    Submitted,
    Delivered { proof: String },
    Failed { reason: String },
}

#[derive(Debug)]
pub(super) enum Submission {
    Submitted { receipt: String },
    Failed { receipt: String, reason: String },
}

pub(super) async fn notify(c: &Config, output: &SlackReturn) -> Result<Submission, String> {
    let url = if output.published {
        let product = output
            .product_id
            .as_deref()
            .ok_or("Published notice has no conversation identity")?;
        format!("{}/c/{product}", c.phoenix_url.trim_end_matches('/'))
    } else {
        format!(
            "{}/settings/slack#slack-return-{}",
            c.phoenix_url.trim_end_matches('/'),
            output.return_id
        )
    };
    let response = call(
        c.notification_command
            .as_deref()
            .ok_or("Private notification command is not configured")?,
        Request {
            version: 1,
            notification_id: &output.return_id,
            owner: &output.owner,
            operation: Operation::Submit {
                text: &output.text,
                url: &url,
            },
        },
    )
    .await?;
    submission(response)
}

fn submission(response: Response) -> Result<Submission, String> {
    match response.outcome {
        Outcome::Submitted => Ok(Submission::Submitted { receipt: response.receipt }),
        Outcome::Failed { reason } if !reason.trim().is_empty() => {
            Ok(Submission::Failed { receipt: response.receipt, reason })
        }
        Outcome::Delivered { .. } | Outcome::Failed { .. } =>
            Err("Notification submission did not establish submission or explicit failure; outcome uncertain".into()),
    }
}

pub(super) async fn observe(output: &SlackReturn) -> Result<SlackNotificationObservation, String> {
    let receipt = output
        .notification_receipt
        .as_deref()
        .ok_or("Missing notification receipt")?;
    let command = output
        .notification_command
        .as_deref()
        .ok_or("Missing frozen notification command")?;
    let response = call(
        command,
        Request {
            version: 1,
            notification_id: &output.return_id,
            owner: &output.owner,
            operation: Operation::Inspect { receipt },
        },
    )
    .await?;
    if response.receipt != receipt {
        return Err("Notification evidence does not match the submitted receipt".into());
    }
    Ok(match response.outcome {
        Outcome::Submitted => SlackNotificationObservation::Pending,
        Outcome::Delivered { proof } if !proof.trim().is_empty() => {
            SlackNotificationObservation::Delivered { proof }
        }
        Outcome::Delivered { .. } => return Err("Notification delivery proof is empty".into()),
        Outcome::Failed { reason } => SlackNotificationObservation::Failed { reason },
    })
}

async fn call(command: &[String], request: Request<'_>) -> Result<Response, String> {
    let (program, args) = command
        .split_first()
        .ok_or("Missing notification executable")?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Unable to launch notification command")?;
    let mut stdin = child.stdin.take().ok_or("Missing notification input")?;
    let stdout = child.stdout.take().ok_or("Missing notification output")?;
    let payload = serde_json::to_vec(&request).map_err(|_| "Invalid notification request")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(45), async {
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
            operation: Operation::Inspect { receipt: "receipt" },
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
            .contains("success"));
        let oversized = command("cat >/dev/null; head -c 65537 /dev/zero".into());
        assert!(call(&oversized, request())
            .await
            .unwrap_err()
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
    }
}
