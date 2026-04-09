//! Phoenix IDE production monitor — TUI dashboard + headless CLI.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

// ============================================================
// Config
// ============================================================

const PROD_LOG: &str = "/.phoenix-ide/prod.log";
const PROD_DB: &str = "/.phoenix-ide/prod.db";
const BASE_URL: &str = "http://localhost:8031";
const POLL_INTERVAL_MS: u64 = 2000;
const LOG_POLL_MS: u64 = 200;
const MAX_LOG_LINES: usize = 500;
const DEFAULT_LOG_LINES_HEADLESS: usize = 50;

fn home_path(suffix: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(format!("{home}{suffix}"))
}

fn prod_log_path() -> PathBuf {
    home_path(PROD_LOG)
}

fn prod_db_path() -> PathBuf {
    home_path(PROD_DB)
}

fn auth_cookie() -> Option<String> {
    std::env::var("PHOENIX_PASSWORD")
        .ok()
        .map(|p| format!("phoenix-auth={p}"))
}

// ============================================================
// API types — local, wire-format structs
// ============================================================

#[derive(Debug, Clone, Deserialize)]
struct ApiConversation {
    id: String,
    slug: Option<String>,
    #[allow(dead_code)]
    title: Option<String>,
    display_state: String,
    conv_mode_label: Option<String>,
    message_count: Option<i64>,
    model: Option<String>,
    #[allow(dead_code)]
    cwd: String,
    updated_at: Option<String>,
    #[allow(dead_code)]
    state: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    parent_conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsResponse {
    conversations: Vec<ApiConversation>,
}

#[derive(Debug, Deserialize)]
struct ConversationDetailResponse {
    conversation: Value,
    messages: Vec<Value>,
    #[allow(dead_code)]
    display_state: String,
    context_window_size: Option<u64>,
}

// ============================================================
// HTTP client
// ============================================================

fn http_get(path: &str) -> Result<String, String> {
    let url = format!("{BASE_URL}{path}");
    let mut req = ureq::get(&url);
    if let Some(cookie) = auth_cookie() {
        req = req.set("Cookie", &cookie);
    }
    req.call()
        .map_err(|e| format!("HTTP error: {e}"))?
        .into_string()
        .map_err(|e| format!("Read error: {e}"))
}

fn fetch_conversations() -> Result<Vec<ApiConversation>, String> {
    let body = http_get("/api/conversations")?;
    let resp: ConversationsResponse =
        serde_json::from_str(&body).map_err(|e| format!("Parse error: {e}"))?;
    Ok(resp.conversations)
}

fn fetch_conversation_detail(id: &str) -> Result<ConversationDetailResponse, String> {
    let body = http_get(&format!("/api/conversations/{id}"))?;
    serde_json::from_str(&body).map_err(|e| format!("Parse detail error: {e}"))
}

// ============================================================
// Sub-agent DB queries
// ============================================================

#[derive(Debug, Clone)]
struct SubAgentInfo {
    #[allow(dead_code)]
    id: String,
    slug: String,
    /// Raw `type` field from state JSON: "completed", "idle", "error", etc.
    state_type: String,
    /// For completed: state.result. For error: state.message.
    outcome: Option<String>,
    /// First user message — the task prompt given to the agent.
    task: Option<String>,
    created_at: String,
    updated_at: String,
    msg_count: i64,
}

impl SubAgentInfo {
    fn duration_str(&self) -> String {
        if let (Ok(c), Ok(u)) = (
            self.created_at.parse::<chrono::DateTime<chrono::Utc>>(),
            self.updated_at.parse::<chrono::DateTime<chrono::Utc>>(),
        ) {
            let secs = (u - c).num_seconds();
            if secs < 60 {
                return format!("{secs}s");
            }
            if secs < 3600 {
                return format!("{}m{}s", secs / 60, secs % 60);
            }
            return format!("{}h{}m", secs / 3600, (secs % 3600) / 60);
        }
        String::new()
    }

    fn display_state(&self) -> &str {
        match self.state_type.as_str() {
            "completed" => "done",
            "idle" => "idle",
            "error" => "error",
            s if s.contains("awaiting") || s.contains("executing") || s.contains("requesting") => {
                "working"
            }
            _ => &self.state_type,
        }
    }

    fn state_color(&self) -> Color {
        match self.display_state() {
            "done" => Color::Green,
            "working" => Color::Yellow,
            "error" => Color::Red,
            _ => Color::DarkGray,
        }
    }
}

fn fetch_sub_agents(db_path: &PathBuf, parent_id: &str) -> Vec<SubAgentInfo> {
    let Ok(conn) = Connection::open(db_path) else {
        return Vec::new();
    };

    let query = "
        SELECT
            c.id,
            COALESCE(c.slug, c.id) as slug,
            c.state,
            c.created_at,
            c.updated_at,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as msg_count,
            (SELECT m.content FROM messages m
             WHERE m.conversation_id = c.id AND m.message_type = 'user'
             ORDER BY m.sequence_id LIMIT 1) as first_msg
        FROM conversations c
        WHERE c.parent_conversation_id = ?1
        ORDER BY c.created_at
    ";

    let Ok(mut stmt) = conn.prepare(query) else {
        return Vec::new();
    };

    stmt.query_map([parent_id], |row| {
        let id: String = row.get(0)?;
        let slug: String = row.get(1)?;
        let state_json: Option<String> = row.get(2)?;
        let created_at: String = row.get(3)?;
        let updated_at: String = row.get(4)?;
        let msg_count: i64 = row.get(5)?;
        let first_msg_json: Option<String> = row.get(6)?;

        let (state_type, outcome) = parse_state_fields(state_json.as_deref());
        let task = first_msg_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_string));

        Ok(SubAgentInfo {
            id,
            slug,
            state_type,
            outcome,
            task,
            created_at,
            updated_at,
            msg_count,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Extract (`state_type`, `outcome_text`) from a state JSON string.
fn parse_state_fields(state_json: Option<&str>) -> (String, Option<String>) {
    let Some(json) = state_json else {
        return ("?".to_string(), None);
    };
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return ("?".to_string(), None);
    };
    let state_type = v
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let outcome = match state_type.as_str() {
        "completed" => v.get("result").and_then(Value::as_str).map(str::to_string),
        "error" => v.get("message").and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    (state_type, outcome)
}

// ============================================================
// Log tailing
// ============================================================

#[derive(Debug, Clone)]
struct LogLine {
    level: String,
    message: String,
    target: Option<String>,
    timestamp: Option<String>,
    raw: String,
}

fn extra_fields(v: &Value) -> String {
    v.get("fields")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter(|(k, _)| k.as_str() != "message")
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("{k}={val}")
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn parse_log_line(raw: &str) -> LogLine {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '\0') {
        return LogLine {
            level: String::new(),
            message: String::new(),
            target: None,
            timestamp: None,
            raw: String::new(),
        };
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => {
            let level = v
                .get("level")
                .and_then(Value::as_str)
                .unwrap_or("INFO")
                .to_string();
            let message = v
                .get("fields")
                .and_then(|f| f.get("message"))
                .and_then(Value::as_str)
                .or_else(|| {
                    v.get("fields")
                        .and_then(|f| f.get("status"))
                        .map(|_| "request")
                })
                .unwrap_or("")
                .to_string();
            let target = v.get("target").and_then(Value::as_str).map(str::to_string);
            let timestamp = v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string);

            let extra = extra_fields(&v);
            let full_msg = if extra.is_empty() {
                message
            } else {
                format!("{message} {extra}")
            };

            LogLine {
                level,
                message: full_msg,
                target,
                timestamp,
                raw: trimmed.to_string(),
            }
        }
        Err(_) => LogLine {
            level: "RAW".to_string(),
            message: trimmed.to_string(),
            target: None,
            timestamp: None,
            raw: trimmed.to_string(),
        },
    }
}

fn start_log_tailer(path: PathBuf) -> mpsc::Receiver<LogLine> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(mut file) = File::open(&path) else {
            return;
        };
        let _ = file.seek(SeekFrom::End(0));
        let mut reader = BufReader::new(file);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    std::thread::sleep(Duration::from_millis(LOG_POLL_MS));
                }
                Ok(_) => {
                    let parsed = parse_log_line(&line);
                    if !parsed.raw.is_empty() {
                        let _ = tx.send(parsed);
                    }
                }
            }
        }
    });
    rx
}

// ============================================================
// Application state
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum RightPanel {
    Logs,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DetailTab {
    State,
    Messages,
    SubAgents,
}

impl DetailTab {
    fn index(self) -> usize {
        match self {
            Self::State => 0,
            Self::Messages => 1,
            Self::SubAgents => 2,
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Messages,
            2 => Self::SubAgents,
            _ => Self::State,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SubDetailTab {
    State,
    Messages,
}

impl SubDetailTab {
    fn index(self) -> usize {
        match self {
            Self::State => 0,
            Self::Messages => 1,
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Messages,
            _ => Self::State,
        }
    }
}

/// Which panel currently receives keyboard input.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    /// Left panel: ↑↓ navigates the conversation list.
    List,
    /// Middle panel: ↑↓ scrolls, Tab/1/2/3 switch tabs, Esc returns to List.
    /// When on `SubAgents` tab, ↑↓ move cursor, Enter opens sub-agent detail.
    Detail,
    /// Right-most panel (sub-agent detail): ↑↓ scrolls, Tab/1/2 switch, Esc returns to Detail.
    SubAgentDetail,
}

struct App {
    conversations: Vec<ApiConversation>,
    list_state: ListState,
    right_panel: RightPanel,
    detail_tab: DetailTab,
    detail: Option<ConversationDetailResponse>,
    log_lines: Vec<LogLine>,
    log_rx: mpsc::Receiver<LogLine>,
    last_refresh: Instant,
    error: Option<String>,
    detail_scroll: u16,
    msg_scroll: u16,
    focus: Focus,
    sub_agents: Vec<SubAgentInfo>,
    sub_agent_cursor: usize,
    sub_agents_scroll: u16,
    sub_agent_detail: Option<ConversationDetailResponse>,
    sub_detail_tab: SubDetailTab,
    sub_detail_scroll: u16,
    sub_detail_msg_scroll: u16,
    db_path: PathBuf,
}

impl App {
    fn new(log_rx: mpsc::Receiver<LogLine>) -> Self {
        let mut s = Self {
            conversations: Vec::new(),
            list_state: ListState::default(),
            right_panel: RightPanel::Logs,
            detail_tab: DetailTab::State,
            detail: None,
            log_lines: Vec::new(),
            log_rx,
            last_refresh: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            error: None,
            detail_scroll: 0,
            msg_scroll: 0,
            focus: Focus::List,
            sub_agents: Vec::new(),
            sub_agent_cursor: 0,
            sub_agents_scroll: 0,
            sub_agent_detail: None,
            sub_detail_tab: SubDetailTab::State,
            sub_detail_scroll: 0,
            sub_detail_msg_scroll: 0,
            db_path: prod_db_path(),
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        match fetch_conversations() {
            Ok(convs) => {
                self.conversations = convs;
                self.error = None;
                if self.list_state.selected().is_none() && !self.conversations.is_empty() {
                    self.list_state.select(Some(0));
                }
                if let Some(sel) = self.list_state.selected() {
                    if sel >= self.conversations.len() && !self.conversations.is_empty() {
                        self.list_state.select(Some(self.conversations.len() - 1));
                    }
                }
            }
            Err(e) => {
                self.error = Some(e);
            }
        }
        self.last_refresh = Instant::now();

        if self.right_panel == RightPanel::Detail {
            if let Some(sel) = self.list_state.selected() {
                if let Some(conv) = self.conversations.get(sel) {
                    let id = conv.id.clone();
                    self.load_detail(&id);
                }
            }
        }
    }

    fn load_detail(&mut self, id: &str) {
        match fetch_conversation_detail(id) {
            Ok(d) => {
                self.detail = Some(d);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
            }
        }
        self.sub_agents = fetch_sub_agents(&self.db_path, id);
    }

    fn drain_logs(&mut self) {
        while let Ok(line) = self.log_rx.try_recv() {
            self.log_lines.push(line);
            if self.log_lines.len() > MAX_LOG_LINES {
                self.log_lines.remove(0);
            }
        }
    }

    fn selected_conv(&self) -> Option<&ApiConversation> {
        self.list_state
            .selected()
            .and_then(|i| self.conversations.get(i))
    }

    fn move_up(&mut self) {
        let len = self.conversations.len();
        if len == 0 {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state
            .select(Some(if i == 0 { len - 1 } else { i - 1 }));
        self.invalidate_detail();
    }

    fn move_down(&mut self) {
        let len = self.conversations.len();
        if len == 0 {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some((i + 1) % len));
        self.invalidate_detail();
    }

    /// If the detail panel is visible, clear and reload it for the current selection.
    fn invalidate_detail(&mut self) {
        if self.right_panel == RightPanel::Detail {
            self.detail = None;
            self.detail_scroll = 0;
            self.msg_scroll = 0;
            self.sub_agent_cursor = 0;
            self.sub_agents_scroll = 0;
            self.sub_agent_detail = None;
            self.sub_detail_scroll = 0;
            self.sub_detail_msg_scroll = 0;
            if self.focus == Focus::SubAgentDetail {
                self.focus = Focus::Detail;
            }
            if let Some(id) = self.selected_conv().map(|c| c.id.clone()) {
                self.load_detail(&id);
            }
        }
    }

    fn load_sub_agent_detail(&mut self, id: &str) {
        match fetch_conversation_detail(id) {
            Ok(d) => {
                self.sub_agent_detail = Some(d);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
            }
        }
    }
}

// ============================================================
// Rendering helpers
// ============================================================

fn state_color(display_state: &str) -> Color {
    match display_state {
        "working" => Color::Green,
        "error" => Color::Red,
        "terminal" => Color::DarkGray,
        _ => Color::Cyan,
    }
}

fn state_dot(display_state: &str) -> &'static str {
    match display_state {
        "working" => "●",
        "error" => "✗",
        "terminal" => "◌",
        _ => "○",
    }
}

fn format_relative_time(ts: &str) -> String {
    let now = chrono::Utc::now();
    if let Ok(t) = ts.parse::<chrono::DateTime<chrono::Utc>>() {
        let diff = now.signed_duration_since(t);
        let secs = diff.num_seconds();
        if secs < 60 {
            return format!("{secs}s");
        }
        let mins = diff.num_minutes();
        if mins < 60 {
            return format!("{mins}m");
        }
        let hours = diff.num_hours();
        if hours < 24 {
            return format!("{hours}h");
        }
        return format!("{}d", diff.num_days());
    }
    String::new()
}

fn slug_truncate(slug: &str, max: usize) -> String {
    slug.chars().take(max).collect()
}

fn content_preview(content: &Value, limit: usize) -> String {
    match content {
        Value::Object(obj) => {
            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                text.chars().take(limit).collect()
            } else if let Some(c) = obj.get("content").and_then(Value::as_str) {
                c.chars().take(limit).collect()
            } else {
                format!("{obj:?}").chars().take(limit).collect()
            }
        }
        Value::Array(arr) => arr
            .first()
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(limit)
            .collect(),
        other => other.to_string().chars().take(limit).collect(),
    }
}

fn json_pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

// ============================================================
// Panel renderers
// ============================================================

fn border_color(focused: bool) -> Color {
    if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    }
}

fn render_conv_list(f: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .conversations
        .iter()
        .map(|c| {
            let dot = state_dot(&c.display_state);
            let color = state_color(&c.display_state);
            let slug = c.slug.as_deref().unwrap_or(&c.id);
            let slug_short = slug_truncate(slug, 20);
            let mode = c.conv_mode_label.as_deref().unwrap_or("?");
            let age = c
                .updated_at
                .as_deref()
                .map(format_relative_time)
                .unwrap_or_default();
            let msgs = c.message_count.unwrap_or(0);

            let line = Line::from(vec![
                Span::styled(dot, Style::default().fg(color)),
                Span::raw(" "),
                Span::styled(
                    format!("{slug_short:<20}"),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" "),
                Span::styled(format!("{mode:<7}"), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(format!("{msgs:>4}"), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(format!("{age:>4}"), Style::default().fg(Color::DarkGray)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let total = app.conversations.len();
    let working = app
        .conversations
        .iter()
        .filter(|c| c.display_state == "working")
        .count();
    let idle = app
        .conversations
        .iter()
        .filter(|c| c.display_state == "idle")
        .count();

    let focused = app.focus == Focus::List;
    let title = format!(
        "CONVERSATIONS  total:{total} idle:{idle} active:{working}{}",
        if focused {
            "  [↑↓ nav · Enter inspect]"
        } else {
            ""
        }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border_color(focused)));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_log_panel(f: &mut Frame, area: Rect, app: &App) {
    let visible_h = area.height.saturating_sub(2) as usize;
    let total = app.log_lines.len();
    let start = total.saturating_sub(visible_h);

    let items: Vec<ListItem> = app.log_lines[start..]
        .iter()
        .map(|l| {
            let (level_color, level_str) = match l.level.as_str() {
                "ERROR" => (Color::Red, "ERR"),
                "WARN" => (Color::Yellow, "WRN"),
                "DEBUG" => (Color::DarkGray, "DBG"),
                "TRACE" => (Color::DarkGray, "TRC"),
                _ => (Color::Cyan, "INF"),
            };

            let ts = l
                .timestamp
                .as_deref()
                .and_then(|t| t.get(11..19))
                .unwrap_or("");

            let target_short = l
                .target
                .as_deref()
                .and_then(|t| t.split("::").last())
                .unwrap_or("");

            let line = Line::from(vec![
                Span::styled(ts, Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(level_str, Style::default().fg(level_color)),
                Span::raw(" "),
                Span::styled(
                    format!("{target_short:<12}"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(" "),
                Span::styled(&l.message, Style::default().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title("LOGS  [Enter=inspect conversation]")
        .border_style(Style::default().fg(Color::DarkGray));

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_state_tab(lines: &mut Vec<Line>, detail: &ConversationDetailResponse) {
    let conv = &detail.conversation;

    let fields = [
        "id",
        "slug",
        "display_state",
        "conv_mode_label",
        "model",
        "message_count",
        "cwd",
        "created_at",
        "updated_at",
    ];
    for field in fields {
        if let Some(v) = conv.get(field) {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{field:<20}"), Style::default().fg(Color::DarkGray)),
                Span::styled(val, Style::default().fg(Color::White)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "state:",
        Style::default().fg(Color::DarkGray),
    )));
    if let Some(state) = conv.get("state") {
        for l in json_pretty(state).lines() {
            lines.push(Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::Cyan),
            )));
        }
    }

    if let Some(ctx) = detail.context_window_size {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("context_window   ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{ctx} tokens"), Style::default().fg(Color::White)),
        ]));
    }
}

fn render_messages_tab(lines: &mut Vec<Line>, detail: &ConversationDetailResponse) {
    for msg in &detail.messages {
        let seq = msg.get("sequence_id").and_then(Value::as_i64).unwrap_or(0);
        let mtype = msg
            .get("message_type")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let (type_color, type_label) = match mtype {
            "user" => (Color::Yellow, "USER"),
            "agent" => (Color::Green, "AGNT"),
            "tool" => (Color::Blue, "TOOL"),
            "system" => (Color::Magenta, "SYS "),
            "error" => (Color::Red, "ERR "),
            "continuation" => (Color::DarkGray, "CONT"),
            "skill" => (Color::Cyan, "SKLL"),
            _ => (Color::White, "    "),
        };

        let preview = msg
            .get("content")
            .map(|c| content_preview(c, 60))
            .unwrap_or_default();

        let usage = msg
            .get("usage_data")
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_i64)
            .map(|t| format!(" {t}tok"))
            .unwrap_or_default();

        lines.push(Line::from(vec![
            Span::styled(format!("{seq:>4} "), Style::default().fg(Color::DarkGray)),
            Span::styled(
                type_label,
                Style::default().fg(type_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(preview, Style::default().fg(Color::White)),
            Span::styled(usage, Style::default().fg(Color::DarkGray)),
        ]));
    }
}

#[allow(clippy::too_many_lines)]
fn render_subagents_tab(
    lines: &mut Vec<Line>,
    sub_agents: &[SubAgentInfo],
    cursor: usize,
    focused: bool,
) {
    if sub_agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "No sub-agents found in DB for this conversation.",
            Style::default().fg(Color::DarkGray),
        )));
        return;
    }

    let done = sub_agents
        .iter()
        .filter(|a| a.state_type == "completed")
        .count();
    let errors = sub_agents
        .iter()
        .filter(|a| a.state_type == "error")
        .count();
    let working = sub_agents
        .iter()
        .filter(|a| {
            matches!(
                a.state_type.as_str(),
                "tool_executing" | "llm_requesting" | "awaiting_llm"
            )
        })
        .count();

    lines.push(Line::from(vec![
        Span::styled(
            format!("{} sub-agents", sub_agents.len()),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  done:{done}  errors:{errors}  working:{working}"),
            Style::default().fg(Color::DarkGray),
        ),
        if focused {
            Span::styled(
                "  [↑↓ nav · Enter open]",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(""));

    for (idx, sa) in sub_agents.iter().enumerate() {
        let selected = focused && idx == cursor;
        let color = sa.state_color();
        let dot = match sa.display_state() {
            "done" => "●",
            "error" => "✗",
            "working" => "◉",
            _ => "○",
        };
        let duration = sa.duration_str();
        let turns = sa.msg_count / 2;

        let selector = if selected { "> " } else { "  " };
        let slug_style = if selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };

        lines.push(Line::from(vec![
            Span::styled(selector, Style::default().fg(Color::Yellow)),
            Span::styled(dot, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(format!("{:<28}", sa.slug), slug_style),
            Span::styled(
                format!("{:<8}", sa.display_state()),
                Style::default().fg(color),
            ),
            Span::styled(
                format!("{duration:>6}  {turns} turns  {} msgs", sa.msg_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        if let Some(task) = &sa.task {
            let preview: String = task.chars().take(120).collect();
            let truncated = if task.len() > 120 { "…" } else { "" };
            lines.push(Line::from(vec![
                Span::styled("    task   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{preview}{truncated}"),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }

        if let Some(outcome) = &sa.outcome {
            let preview: String = outcome.chars().take(120).collect();
            let truncated = if outcome.len() > 120 { "…" } else { "" };
            let (label, ocol) = if sa.state_type == "error" {
                ("    error  ", Color::Red)
            } else {
                ("    result ", Color::Green)
            };
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{preview}{truncated}"), Style::default().fg(ocol)),
            ]));
        }

        lines.push(Line::from(""));
    }
}

fn render_detail_panel(f: &mut Frame, area: Rect, app: &mut App) {
    let titles: Vec<Line> = ["State", "Messages", "Sub-agents"]
        .iter()
        .map(|t| Line::from(*t))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let focused = app.focus == Focus::Detail;
    let tab_hint = if focused && app.detail_tab == DetailTab::SubAgents {
        "DETAIL  [↑↓ cursor · Enter open · Tab/1/2/3 tabs · Esc back]"
    } else if focused {
        "DETAIL  [↑↓ scroll · Tab/1/2/3 tabs · Esc back]"
    } else {
        "DETAIL  [Enter to focus]"
    };
    let bc = border_color(focused);

    let tabs = Tabs::new(titles)
        .select(app.detail_tab.index())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(tab_hint)
                .border_style(Style::default().fg(bc)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    let sub_agent_cursor = app.sub_agent_cursor;
    let sub_agents = app.sub_agents.clone();

    // Auto-scroll sub-agents list to keep cursor visible. Done here because
    // we know the content area height only at render time.
    if app.detail_tab == DetailTab::SubAgents && !sub_agents.is_empty() {
        let content_h = chunks[1].height.saturating_sub(2); // minus borders
        let mut cursor_line = 2u16; // preamble: summary line + blank line
        for (i, sa) in sub_agents.iter().enumerate() {
            if i == sub_agent_cursor {
                break;
            }
            cursor_line += 2; // header + blank
            if sa.task.is_some() {
                cursor_line += 1;
            }
            if sa.outcome.is_some() {
                cursor_line += 1;
            }
        }
        let entry_h = {
            let sa = &sub_agents[sub_agent_cursor];
            2u16 + u16::from(sa.task.is_some()) + u16::from(sa.outcome.is_some())
        };
        if cursor_line < app.sub_agents_scroll {
            app.sub_agents_scroll = cursor_line;
        } else if cursor_line + entry_h > app.sub_agents_scroll + content_h {
            app.sub_agents_scroll = (cursor_line + entry_h).saturating_sub(content_h);
        }
    }

    let scroll = match app.detail_tab {
        DetailTab::State => app.detail_scroll,
        DetailTab::Messages => app.msg_scroll,
        DetailTab::SubAgents => app.sub_agents_scroll,
    };

    let Some(detail) = &app.detail else {
        let p = Paragraph::new("Loading...").block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(bc)),
        );
        f.render_widget(p, chunks[1]);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    match app.detail_tab {
        DetailTab::State => render_state_tab(&mut lines, detail),
        DetailTab::Messages => render_messages_tab(&mut lines, detail),
        DetailTab::SubAgents => {
            render_subagents_tab(&mut lines, &sub_agents, sub_agent_cursor, focused);
        }
    }

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(bc)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, chunks[1]);
}

fn render_sub_agent_detail_panel(f: &mut Frame, area: Rect, app: &mut App) {
    let titles: Vec<Line> = ["State", "Messages"]
        .iter()
        .map(|t| Line::from(*t))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let focused = app.focus == Focus::SubAgentDetail;
    let tab_hint = if focused {
        "SUB-AGENT  [↑↓ scroll · Tab/1/2 tabs · Esc back]"
    } else {
        "SUB-AGENT"
    };
    let bc = border_color(focused);

    let tabs = Tabs::new(titles)
        .select(app.sub_detail_tab.index())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(tab_hint)
                .border_style(Style::default().fg(bc)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    let Some(detail) = &app.sub_agent_detail else {
        let p = Paragraph::new("Loading...").block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(bc)),
        );
        f.render_widget(p, chunks[1]);
        return;
    };

    let scroll = match app.sub_detail_tab {
        SubDetailTab::State => app.sub_detail_scroll,
        SubDetailTab::Messages => app.sub_detail_msg_scroll,
    };

    let mut lines: Vec<Line> = Vec::new();
    match app.sub_detail_tab {
        SubDetailTab::State => render_state_tab(&mut lines, detail),
        SubDetailTab::Messages => render_messages_tab(&mut lines, detail),
    }

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(bc)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, chunks[1]);
}

fn render_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let age = app.last_refresh.elapsed().as_secs();
    let err_part = app
        .error
        .as_deref()
        .map(|e| format!("  ERR: {e}"))
        .unwrap_or_default();
    let hint = match app.focus {
        Focus::List => "↑↓ navigate  Enter inspect  l logs  r refresh  q quit",
        Focus::Detail => "↑↓ scroll/cursor  Tab/1/2/3 tabs  Enter open sub-agent  Esc back  q quit",
        Focus::SubAgentDetail => "↑↓ scroll  Tab/1/2 tabs  Esc back to parent  q quit",
    };
    let text = format!(" {hint}  [{age}s ago]{err_part}");
    let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(para, area);
}

fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(size);

    let main_area = main_chunks[0];
    let status_area = main_chunks[1];

    let show_sub_detail = app.right_panel == RightPanel::Detail && app.sub_agent_detail.is_some();

    if show_sub_detail {
        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(35),
                Constraint::Percentage(35),
            ])
            .split(main_area);

        render_conv_list(f, horiz[0], app);
        render_detail_panel(f, horiz[1], app);
        render_sub_agent_detail_panel(f, horiz[2], app);
    } else {
        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(main_area);

        render_conv_list(f, horiz[0], app);

        match app.right_panel {
            RightPanel::Logs => render_log_panel(f, horiz[1], app),
            RightPanel::Detail => render_detail_panel(f, horiz[1], app),
        }
    }

    render_status_bar(f, status_area, app);
}

// ============================================================
// TUI event handling
// ============================================================

fn scroll_up(app: &mut App) {
    match app.detail_tab {
        DetailTab::State => app.detail_scroll = app.detail_scroll.saturating_sub(1),
        DetailTab::Messages => app.msg_scroll = app.msg_scroll.saturating_sub(1),
        DetailTab::SubAgents => {
            let len = app.sub_agents.len();
            if len == 0 {
                return;
            }
            app.sub_agent_cursor = if app.sub_agent_cursor == 0 {
                len - 1
            } else {
                app.sub_agent_cursor - 1
            };
        }
    }
}

fn scroll_down(app: &mut App) {
    match app.detail_tab {
        DetailTab::State => app.detail_scroll = app.detail_scroll.saturating_add(1),
        DetailTab::Messages => app.msg_scroll = app.msg_scroll.saturating_add(1),
        DetailTab::SubAgents => {
            let len = app.sub_agents.len();
            if len == 0 {
                return;
            }
            app.sub_agent_cursor = (app.sub_agent_cursor + 1) % len;
        }
    }
}

fn sub_scroll_up(app: &mut App) {
    match app.sub_detail_tab {
        SubDetailTab::State => app.sub_detail_scroll = app.sub_detail_scroll.saturating_sub(1),
        SubDetailTab::Messages => {
            app.sub_detail_msg_scroll = app.sub_detail_msg_scroll.saturating_sub(1);
        }
    }
}

fn sub_scroll_down(app: &mut App) {
    match app.sub_detail_tab {
        SubDetailTab::State => app.sub_detail_scroll = app.sub_detail_scroll.saturating_add(1),
        SubDetailTab::Messages => {
            app.sub_detail_msg_scroll = app.sub_detail_msg_scroll.saturating_add(1);
        }
    }
}

#[allow(clippy::too_many_lines)]
fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    // Global keys — always active.
    match (code, modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Char('r'), _) => {
            app.refresh();
            return false;
        }
        (KeyCode::Char('l'), _) => {
            app.right_panel = RightPanel::Logs;
            app.detail = None;
            app.focus = Focus::List;
            return false;
        }
        _ => {}
    }

    // Focus-specific keys.
    match app.focus {
        Focus::List => match code {
            KeyCode::Up => app.move_up(),
            KeyCode::Down => app.move_down(),
            KeyCode::Enter => {
                if let Some(id) = app.selected_conv().map(|c| c.id.clone()) {
                    app.right_panel = RightPanel::Detail;
                    app.detail_tab = DetailTab::State;
                    app.detail_scroll = 0;
                    app.msg_scroll = 0;
                    app.load_detail(&id);
                    app.focus = Focus::Detail;
                }
            }
            _ => {}
        },

        Focus::Detail => match code {
            KeyCode::Esc => app.focus = Focus::List,
            KeyCode::Up => scroll_up(app),
            KeyCode::Down => scroll_down(app),
            KeyCode::PageUp => {
                for _ in 0..10 {
                    scroll_up(app);
                }
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    scroll_down(app);
                }
            }
            KeyCode::Enter => {
                if app.detail_tab == DetailTab::SubAgents && !app.sub_agents.is_empty() {
                    let id = app.sub_agents[app.sub_agent_cursor].id.clone();
                    app.sub_detail_tab = SubDetailTab::State;
                    app.sub_detail_scroll = 0;
                    app.sub_detail_msg_scroll = 0;
                    app.load_sub_agent_detail(&id);
                    app.focus = Focus::SubAgentDetail;
                }
            }
            KeyCode::Tab => {
                let next = (app.detail_tab.index() + 1) % 3;
                app.detail_tab = DetailTab::from_index(next);
                app.detail_scroll = 0;
                app.msg_scroll = 0;
            }
            KeyCode::Char('1') => {
                app.detail_tab = DetailTab::State;
                app.detail_scroll = 0;
            }
            KeyCode::Char('2') => {
                app.detail_tab = DetailTab::Messages;
                app.msg_scroll = 0;
            }
            KeyCode::Char('3') => {
                app.detail_tab = DetailTab::SubAgents;
            }
            _ => {}
        },

        Focus::SubAgentDetail => match code {
            KeyCode::Esc => {
                app.focus = Focus::Detail;
                app.sub_agent_detail = None;
                app.sub_detail_scroll = 0;
                app.sub_detail_msg_scroll = 0;
            }
            KeyCode::Up => sub_scroll_up(app),
            KeyCode::Down => sub_scroll_down(app),
            KeyCode::PageUp => {
                for _ in 0..10 {
                    sub_scroll_up(app);
                }
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    sub_scroll_down(app);
                }
            }
            KeyCode::Tab => {
                let next = (app.sub_detail_tab.index() + 1) % 2;
                app.sub_detail_tab = SubDetailTab::from_index(next);
                app.sub_detail_scroll = 0;
                app.sub_detail_msg_scroll = 0;
            }
            KeyCode::Char('1') => {
                app.sub_detail_tab = SubDetailTab::State;
                app.sub_detail_scroll = 0;
            }
            KeyCode::Char('2') => {
                app.sub_detail_tab = SubDetailTab::Messages;
                app.sub_detail_msg_scroll = 0;
            }
            _ => {}
        },
    }
    false
}

// ============================================================
// TUI main loop
// ============================================================

fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let log_rx = start_log_tailer(prod_log_path());
    let mut app = App::new(log_rx);

    loop {
        app.drain_logs();
        terminal.draw(|f| draw(f, &mut app))?;

        if app.last_refresh.elapsed() >= Duration::from_millis(POLL_INTERVAL_MS) {
            app.refresh();
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = event::read()?
            {
                if handle_key(&mut app, code, modifiers) {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

// ============================================================
// Headless mode
// ============================================================

fn headless_conversations() {
    match fetch_conversations() {
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        Ok(convs) => {
            let total = convs.len();
            let working: Vec<_> = convs
                .iter()
                .filter(|c| c.display_state == "working")
                .collect();
            let idle = convs.iter().filter(|c| c.display_state == "idle").count();
            let terminal = convs
                .iter()
                .filter(|c| c.display_state == "terminal")
                .count();
            let error = convs.iter().filter(|c| c.display_state == "error").count();

            println!(
                "Phoenix IDE — {total} conversations  idle:{idle} working:{} terminal:{terminal} error:{error}",
                working.len()
            );
            println!();

            if !working.is_empty() {
                println!("ACTIVE:");
                for c in &working {
                    let slug = c.slug.as_deref().unwrap_or(&c.id);
                    let mode = c.conv_mode_label.as_deref().unwrap_or("?");
                    let msgs = c.message_count.unwrap_or(0);
                    let model = c.model.as_deref().unwrap_or("?");
                    let age = c
                        .updated_at
                        .as_deref()
                        .map(format_relative_time)
                        .unwrap_or_default();
                    println!("  {slug}");
                    println!("    mode={mode} msgs={msgs} model={model} updated={age}");
                }
                println!();
            }

            println!("ALL:");
            for c in &convs {
                let dot = state_dot(&c.display_state);
                let slug = c.slug.as_deref().unwrap_or(&c.id);
                let mode = c.conv_mode_label.as_deref().unwrap_or("?");
                let msgs = c.message_count.unwrap_or(0);
                let age = c
                    .updated_at
                    .as_deref()
                    .map(format_relative_time)
                    .unwrap_or_default();
                println!("  {dot} {slug:<40} {mode:<7} msgs={msgs:<4} {age}");
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn print_conv_detail(d: &ConversationDetailResponse) {
    let conv = &d.conversation;

    let print_field = |label: &str, key: &str| {
        if let Some(v) = conv.get(key) {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            println!("{label:<20} {val}");
        }
    };

    println!("=== CONVERSATION ===");
    print_field("id:", "id");
    print_field("slug:", "slug");
    print_field("title:", "title");
    print_field("state:", "display_state");
    print_field("mode:", "conv_mode_label");
    print_field("model:", "model");
    print_field("messages:", "message_count");
    print_field("cwd:", "cwd");
    print_field("created:", "created_at");
    print_field("updated:", "updated_at");

    if let Some(ctx) = d.context_window_size {
        println!("{:<20} {} tokens", "context_window:", ctx);
    }

    println!();
    println!("=== STATE ===");
    if let Some(state) = conv.get("state") {
        println!("{}", json_pretty(state));
    }

    println!();
    println!("=== MESSAGES ({}) ===", d.messages.len());

    let mut by_type: HashMap<&str, usize> = HashMap::new();
    for msg in &d.messages {
        let t = msg
            .get("message_type")
            .and_then(Value::as_str)
            .unwrap_or("?");
        *by_type.entry(t).or_insert(0) += 1;
    }
    let mut type_counts: Vec<_> = by_type.iter().collect();
    type_counts.sort_by_key(|(k, _)| *k);
    for (t, n) in &type_counts {
        print!("{t}={n}  ");
    }
    println!();
    println!();

    for msg in &d.messages {
        let seq = msg.get("sequence_id").and_then(Value::as_i64).unwrap_or(0);
        let mtype = msg
            .get("message_type")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let preview = msg
            .get("content")
            .map(|c| content_preview(c, 80))
            .unwrap_or_default();
        let tok = msg
            .get("usage_data")
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_i64)
            .map(|t| format!(" [{t}tok]"))
            .unwrap_or_default();
        println!("[{seq:>4}] {mtype:<12} {preview}{tok}");
    }

    // Sub-agents from DB
    let conv_id = conv.get("id").and_then(Value::as_str).unwrap_or_default();
    let sub_agents = fetch_sub_agents(&prod_db_path(), conv_id);
    if !sub_agents.is_empty() {
        println!();
        println!("=== SUB-AGENTS ({}) ===", sub_agents.len());
        let done = sub_agents
            .iter()
            .filter(|a| a.state_type == "completed")
            .count();
        let errs = sub_agents
            .iter()
            .filter(|a| a.state_type == "error")
            .count();
        println!("done:{done}  errors:{errs}  total:{}", sub_agents.len());
        println!();
        for sa in &sub_agents {
            let dur = sa.duration_str();
            let turns = sa.msg_count / 2;
            println!(
                "[{}] {}  {}  {dur}  {turns} turns",
                sa.display_state(),
                sa.slug,
                sa.state_type,
            );
            if let Some(task) = &sa.task {
                let preview: String = task.chars().take(140).collect();
                let ellipsis = if task.len() > 140 { "…" } else { "" };
                println!("  task:   {preview}{ellipsis}");
            }
            if let Some(outcome) = &sa.outcome {
                let preview: String = outcome.chars().take(140).collect();
                let ellipsis = if outcome.len() > 140 { "…" } else { "" };
                println!("  result: {preview}{ellipsis}");
            }
            println!();
        }
    }
}

fn headless_conversation(id_or_slug: &str) {
    let is_uuid = id_or_slug.len() == 36 && id_or_slug.chars().filter(|c| *c == '-').count() == 4;

    let detail = if is_uuid {
        fetch_conversation_detail(id_or_slug)
    } else {
        match fetch_conversations() {
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            Ok(convs) => {
                let found = convs.iter().find(|c| c.slug.as_deref() == Some(id_or_slug));
                match found {
                    None => {
                        eprintln!("No conversation found with slug '{id_or_slug}'");
                        std::process::exit(1);
                    }
                    Some(c) => fetch_conversation_detail(&c.id),
                }
            }
        }
    };

    match detail {
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        Ok(d) => print_conv_detail(&d),
    }
}

fn headless_logs(lines: usize) {
    let path = prod_log_path();
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Cannot open {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty() && !l.trim().chars().all(|c| c == '\0'))
        .collect();

    let start = all_lines.len().saturating_sub(lines);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for raw in &all_lines[start..] {
        let l = parse_log_line(raw);
        if l.raw.is_empty() {
            continue;
        }
        let ts = l
            .timestamp
            .as_deref()
            .and_then(|t| t.get(11..19))
            .unwrap_or("");
        let target = l.target.as_deref().unwrap_or("");
        let _ = writeln!(out, "{ts} {:<5} {:<30} {}", l.level, target, l.message);
    }
}

// ============================================================
// Entry point
// ============================================================

fn print_usage() {
    eprintln!("Usage: phoenix-monitor [headless <subcommand>]");
    eprintln!();
    eprintln!("Subcommands:");
    eprintln!("  (no args)                     Launch TUI dashboard");
    eprintln!("  headless conversations         List all conversations");
    eprintln!("  headless conversation <id>     Show conversation detail");
    eprintln!("  headless logs [--lines N]      Tail log file");
    eprintln!();
    eprintln!("Auth: set PHOENIX_PASSWORD env var or pass --password <pw>");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut log_lines_override: Option<usize> = None;
    let mut consumed = vec![false; args.len()];
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--password" && i + 1 < args.len() {
            // Safe: env var set within process scope only
            std::env::set_var("PHOENIX_PASSWORD", &args[i + 1]);
            consumed[i] = true;
            consumed[i + 1] = true;
            i += 2;
        } else if args[i] == "--lines" && i + 1 < args.len() {
            log_lines_override = args[i + 1].parse().ok();
            consumed[i] = true;
            consumed[i + 1] = true;
            i += 2;
        } else {
            i += 1;
        }
    }

    let positional: Vec<&str> = args[1..]
        .iter()
        .enumerate()
        .filter(|(idx, _)| !consumed[idx + 1])
        .map(|(_, a)| a.as_str())
        .collect();

    match positional.as_slice() {
        [] => {
            if let Err(e) = run_tui() {
                eprintln!("TUI error: {e}");
                std::process::exit(1);
            }
        }
        ["headless", "conversations"] => headless_conversations(),
        ["headless", "conversation", id] => headless_conversation(id),
        ["headless", "logs"] => {
            headless_logs(log_lines_override.unwrap_or(DEFAULT_LOG_LINES_HEADLESS));
        }
        _ => {
            print_usage();
            std::process::exit(1);
        }
    }
}
