// Draft standalone integration-test file for crates/phoenix-tools/tests/.
// Exercises the phoenix-browser CLI protocol; no Playwright/direct CDP.
#![cfg(unix)]

use base64::Engine as _;
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

struct BrowserCli {
    dir: tempfile::TempDir,
    artifacts: Mutex<Vec<PathBuf>>,
}
impl BrowserCli {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("pb-touch-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            dir,
            artifacts: Mutex::new(Vec::new()),
        }
    }
    fn invoke(&self, args: &[&str], stdin: Option<&str>, success: bool) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-browser"));
        command
            .env("PHOENIX_BROWSER_RUNTIME_DIR", self.dir.path())
            .env("PHOENIX_TMP_DIR", self.dir.path().join("scratch"))
            .env_remove("PHOENIX_BROWSER_SESSION")
            .env_remove("PHOENIX_BROWSER_IDLE_TIMEOUT")
            .args(["--json", "--session", "touch-acceptance"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command.spawn().unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            success,
            "{args:?}: {stdout}\n{stderr}"
        );
        let response: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {stdout}\n{stderr}"));
        assert_eq!(response["session"], "named:touch-acceptance");
        assert_eq!(
            response["status"],
            if success { "success" } else { "error" }
        );
        if let Some(paths) = response["result"]["artifacts"].as_array() {
            self.artifacts
                .lock()
                .unwrap()
                .extend(paths.iter().filter_map(Value::as_str).map(PathBuf::from));
        }
        response
    }
    fn ok(&self, args: &[&str]) -> Value {
        self.invoke(args, None, true)
    }
    fn eval(&self, js: &str) -> Value {
        self.invoke(&["eval", "--stdin"], Some(js), true)["result"]["data"]["value"].clone()
    }
}
impl Drop for BrowserCli {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_phoenix-browser"))
            .env("PHOENIX_BROWSER_RUNTIME_DIR", self.dir.path())
            .env("PHOENIX_TMP_DIR", self.dir.path().join("scratch"))
            .args(["--json", "--session", "touch-acceptance", "close"])
            .output();
        for path in self.artifacts.get_mut().unwrap().iter() {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn fixture_url() -> String {
    let html = r#"<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><style>html,body{margin:0}#target{width:300px;height:240px;background:#def}</style><div id="target"></div><script>
window.touchTrace=[];window.geometry=[];window.active=false;window.mutation=null;const target=document.querySelector('#target');
for(const type of ['touchstart','touchmove','touchend','touchcancel'])target.addEventListener(type,e=>{const t=e.changedTouches[0];window.touchTrace.push({type,isTrusted:e.isTrusted,identifier:t?.identifier??null,touches:e.touches.length,targetTouches:e.targetTouches.length,x:t?.clientX??null,y:t?.clientY??null,active:window.active});if(type==='touchstart')window.active=true;if(type==='touchend'||type==='touchcancel')window.active=false});
</script>"#;
    format!(
        "data:text/html;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(html)
    )
}
fn device() -> Value {
    json!({"width":390,"height":844,"device_scale_factor":1,"mobile":true,"touch":true})
}
fn plan(steps: Value) -> String {
    json!({"action":"run_scenario","runs":1,"warmup":0,"throttle_rate":1,"gc_per_run":false,"reset":"none","device":device(),"steps":steps}).to_string()
}
fn assert_schema(cli: &BrowserCli) {
    let schema = cli.ok(&["schema", "browser_profile"]);
    let operation = schema["result"]["operations"]
        .as_array()
        .expect("schema operations array")
        .iter()
        .find(|operation| operation["name"] == "browser_profile")
        .expect("browser_profile schema operation");
    let input = &operation["input_schema"];
    assert_eq!(
        input["properties"]["device"]["properties"]["touch"]["type"],
        "boolean"
    );
    assert!(
        input["properties"]["device"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "touch"),
        "{input}"
    );
    let desc = input["properties"]["steps"]["description"]
        .as_str()
        .unwrap();
    for step in ["touch_start", "touch_move", "touch_end", "pause"] {
        assert!(desc.contains(step), "schema omits {step}: {desc}");
    }
}

#[test]
#[ignore = "requires explicitly installed Chrome/Chromium; invoke this acceptance test directly"]
fn cli_profile_touch_composition_trust_geometry_and_failure_release() {
    assert!(
        std::env::var_os("PHOENIX_CHROME_EXECUTABLE").is_some(),
        "set PHOENIX_CHROME_EXECUTABLE to an installed Chrome/Chromium binary"
    );
    let cli = BrowserCli::new();
    assert_schema(&cli);
    cli.ok(&["open", &fixture_url()]);
    let successful=cli.ok(&["profile","run_scenario","--input",&plan(json!([
        {"kind":"wait_selector","selector":"#target"},
        {"kind":"touch_start","x":100,"y":100},
        {"kind":"touch_move","x":120,"y":115},
        {"kind":"eval","expression":"(()=>{const target=document.querySelector('#target');const before=target.getBoundingClientRect().height;window.geometry=[];window.mutation={before,heldBefore:window.active,at:performance.now()};target.style.height='260px';const after=target.getBoundingClientRect().height;window.mutation.after=after;window.mutation.heldAfter=window.active;let count=0;const sample=()=>{if(!window.active)return;const r=target.getBoundingClientRect();window.geometry.push({time:performance.now(),height:r.height,x:r.x,y:r.y,width:r.width,top:r.top,right:r.right,bottom:r.bottom,left:r.left,held:window.active});if(++count<8&&window.active)requestAnimationFrame(sample)};requestAnimationFrame(sample);return window.mutation})()"},
        {"kind":"pause","milliseconds":120},{"kind":"touch_end"}
    ]))]);
    assert_eq!(successful["result"]["data"]["outcome"], "completed");
    let evidence =
        cli.eval("({trace:window.touchTrace,geometry:window.geometry,mutation:window.mutation})");
    assert_eq!(evidence["mutation"]["heldBefore"], true);
    assert_eq!(evidence["mutation"]["heldAfter"], true);
    assert_eq!(evidence["mutation"]["before"], 240.0);
    assert_eq!(evidence["mutation"]["after"], 260.0);
    let trace = evidence["trace"].as_array().unwrap();
    let event = |kind: &str| {
        trace
            .iter()
            .find(|e| e["type"] == kind)
            .unwrap_or_else(|| panic!("missing {kind}: {trace:?}"))
    };
    let start = event("touchstart");
    let moved = event("touchmove");
    let end = event("touchend");
    assert!(
        start["isTrusted"].as_bool().unwrap()
            && moved["isTrusted"].as_bool().unwrap()
            && end["isTrusted"].as_bool().unwrap()
    );
    assert_eq!(start["identifier"], moved["identifier"]);
    assert_eq!(start["identifier"], end["identifier"]);
    assert_eq!(start["touches"], 1);
    assert_eq!(moved["touches"], 1);
    assert_eq!(end["touches"], 0);
    let geometry = evidence["geometry"].as_array().unwrap();
    assert!(
        !geometry.is_empty(),
        "rAF sampler did not run during held-contact window"
    );
    for sample in geometry {
        assert_eq!(
            sample["held"], true,
            "sample outside held contact: {sample}"
        );
        for key in [
            "time", "height", "x", "y", "width", "top", "right", "bottom", "left",
        ] {
            assert!(
                sample[key].as_f64().is_some_and(f64::is_finite),
                "{key}: {sample}"
            );
        }
    }

    let failed=cli.invoke(&["profile","run_scenario","--input",&plan(json!([
        {"kind":"wait_selector","selector":"#target"},{"kind":"touch_start","x":100,"y":100},
        {"kind":"eval","expression":"throw Error('acceptance failure while held')"},{"kind":"touch_end"}
    ]))],None,false);
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("acceptance failure while held"),
        "{failed}"
    );
    let tail = cli.eval("window.touchTrace.slice(-4)");
    assert!(
        tail.as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "touchcancel"),
        "failure did not release contact with touchcancel: {tail}"
    );
    cli.ok(&[
        "profile",
        "run_scenario",
        "--input",
        &plan(json!([
            {"kind":"wait_selector","selector":"#target"},
            {"kind":"touch_start","x":100,"y":100},
            {"kind":"touch_end"}
        ])),
    ]);
}

// Failure plans deliberately retain touch_end after the failing eval so validation
// accepts their contact balance; assertions target the runtime error and cancel trace.
// The recorder starts inside the scenario after readiness and captures real rects while held.
// No CLI status/outcome strings are inferred beyond the documented protocol fields.
// The ignored test asserts its configured browser path exists rather than green-skipping.
// A host must enforce its chosen outer timeout when invoking this opt-in test.
