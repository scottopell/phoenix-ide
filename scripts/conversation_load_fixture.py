#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Build a NEW, sanitized relational fixture, never a raw database backup.

Usage: uv run scripts/conversation_load_fixture.py --source /absolute/prod.db \
    --output-dir .phoenix/conversation-load-fixture [--target PRODUCT_UUID]

Only sqlite_master DDL and explicitly classified relational fields are copied.
The source is mode=ro/query_only, held in ONE read transaction including counts,
schema, and rows (WAL is not ignored). No Phoenix runtime or HTTP is involved.
The output directory must be new and ignored by git if inside a repository.
fixture.db is standalone chmod 0400; manifest is chmod 0600, directory 0700.

This is a timing/scale fixture, not a semantic or rendering-fidelity fixture:
prose is synthetic, preserving decoded character/UTF-8 lengths and CR/LF offsets;
opaque JSON is replaced by one synthetic payload of comparable serialized size,
not by its keys. Display/usage data, images, files, jobs, effects, credentials,
and migration DATA are omitted. Markdown, code, input structure, provider blocks,
image decoding, and active-state UI costs are not reproduced. Unknown message
variants/fields fail closed, not round-tripped. Non-idle/context-exhausted states
become Idle; summaries are synthesized. All transcripts/messages are kept, not
just the target product. The manifest reports the omissions and measured scale.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import sys
import time
import uuid

DEFAULT_TARGET = "7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f"
TABLES = (
    "products", "product_conversations", "work_scopes", "conversations",
    "product_conversation_work_scopes", "messages",
    "completed_continuation_handoffs", "product_conversation_sources",
    "conversation_approved_task_objectives", "work_scope_approved_task_authorities",
)
SAFE_TOOLS = frozenset("""bash patch read_file read_image search keyword_search think
browser_navigate browser_eval browser_click browser_type browser_key_press
browser_take_screenshot browser_resize browser_wait_for_selector browser_profile
browser_recent_console_logs browser_clear_console_logs tmux submit_result submit_error
present_svg spawn_agent spawn_agents wait_agent wait_agents list_agents skill
search_conversations read_conversation query_database send_conversation_message
approve_task request_user_input update_task create_task list_tasks taskmd
""".split())


class FixtureError(ValueError):
    """Errors contain classifications only, never source values."""


def dumps(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


def fields(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= value.keys() or set(value) - set(required) - set(optional):
        raise FixtureError("unrecognized content fields or shape")


def synthetic(text):
    if not isinstance(text, str):
        raise FixtureError("expected text")
    def replace(match):
        s = match.group()
        pattern = "fixture synthetic benchmark text "
        if s.isascii():
            return (pattern * ((len(s) + len(pattern) - 1) // len(pattern)))[:len(s)]
        return "".join({1: "x", 2: "é", 3: "界", 4: "🌑"}[len(c.encode("utf-8"))] for c in s)
    return re.sub(r"[^\r\n]+", replace, text)


def opaque(value):
    size = len(dumps(value).encode("utf-8"))
    overhead = len(dumps({"fixture_payload": ""}).encode())
    return {"fixture_payload": "x" * max(0, size - overhead)}


def identity(value):
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise FixtureError("invalid relational identity")
    try:
        parsed = uuid.UUID(value)
        if str(parsed) == value.lower():
            return value
    except ValueError:
        pass
    return "fixture-" + hashlib.sha256(value.encode()).hexdigest()


def tool_name(value):
    return value if isinstance(value, str) and value in SAFE_TOOLS else "fixture_tool"


def boolean(value):
    if not isinstance(value, bool):
        raise FixtureError("expected boolean")
    return value


def sanitize_block(block, omissions):
    if not isinstance(block, dict):
        raise FixtureError("invalid agent block")
    kind = block.get("type")
    if kind == "text":
        fields(block, ("type", "text"))
        return {"type": kind, "text": synthetic(block["text"])}
    if kind == "thinking":
        fields(block, ("type", "thinking"), ("signature",))
        omissions["thinking_signatures"] += int(bool(block.get("signature")))
        return {"type": kind, "thinking": synthetic(block["thinking"]), "signature": ""}
    if kind in ("tool_use", "server_tool_use", "mcp_tool_use"):
        required = ("type", "id", "name", "input") + (("server_name",) if kind == "mcp_tool_use" else ())
        fields(block, required)
        result = {"type": kind, "id": identity(block["id"]), "name": tool_name(block["name"]), "input": opaque(block["input"])}
        if kind == "mcp_tool_use":
            result["server_name"] = "fixture-server"
        return result
    if kind == "tool_result":
        fields(block, ("type", "tool_use_id", "content"), ("is_error", "images"))
        omissions["embedded_images"] += len(block.get("images", []))
        return {"type": kind, "tool_use_id": identity(block["tool_use_id"]), "content": synthetic(block["content"]), "is_error": boolean(block.get("is_error", False)), "images": []}
    if kind in ("web_search_tool_result", "web_fetch_tool_result", "code_execution_tool_result", "bash_code_execution_tool_result", "text_editor_code_execution_tool_result", "mcp_tool_result"):
        fields(block, ("type", "tool_use_id", "content"), ("is_error",) if kind == "mcp_tool_result" else ())
        result = {"type": kind, "tool_use_id": identity(block["tool_use_id"]), "content": opaque(block["content"])}
        if kind == "mcp_tool_result":
            result["is_error"] = boolean(block.get("is_error", False))
        return result
    # Typed search results and image sources need a dedicated safe projection.
    raise FixtureError("unsupported agent block type")


def sanitize_content(kind, raw, omissions=None):
    if omissions is None:
        omissions = Counter()
    value = json.loads(raw)
    if kind == "agent":
        if not isinstance(value, list):
            raise FixtureError("agent content must be a block list")
        return dumps([sanitize_block(b, omissions) for b in value])
    if kind in ("system", "error", "continuation"):
        key = {"system": "text", "error": "message", "continuation": "summary"}[kind]
        fields(value, (key,))
        return dumps({key: synthetic(value[key])})
    if kind == "user":
        fields(value, ("text",), ("llm_text", "is_meta", "images", "files"))
        result = {"text": synthetic(value["text"])}
        if "llm_text" in value:
            result["llm_text"] = None if value["llm_text"] is None else synthetic(value["llm_text"])
        if "is_meta" in value:
            result["is_meta"] = boolean(value["is_meta"])
        for key in ("images", "files"):
            omissions["embedded_" + key] += len(value.get(key, []))
        return dumps(result)
    if kind == "tool":
        fields(value, ("tool_use_id", "content", "is_error"), ("images",))
        omissions["embedded_images"] += len(value.get("images", []))
        return dumps({"tool_use_id": identity(value["tool_use_id"]), "content": synthetic(value["content"]), "is_error": boolean(value["is_error"]), "images": []})
    if kind == "skill":
        fields(value, ("name", "body", "trigger"), ("files",))
        omissions["embedded_files"] += len(value.get("files", []))
        return dumps({"name": "fixture-skill", "body": synthetic(value["body"]), "trigger": synthetic(value["trigger"])})
    raise FixtureError("unsupported message type")


# Columns have explicit, non-overlapping consumers. New columns fail closed.
ID_COLUMNS = frozenset("""id message_id conversation_id product_conversation_id
parent_conversation_id seed_parent_id continued_in_conv_id spawned_from_conversation_id
work_scope_id predecessor_conversation_id successor_conversation_id continuation_message_id
accepted_successor_message_id target_product_conversation_id source_product_conversation_id
source_conversation_id origin_product_conversation_id origin_transcript_id
origin_source_message_id origin_source_tool_use_id worktree_id relation_key task_id
objective_conversation_id
cm_task_id cm_next_taskmd_id_hint""".split())
PROSE_COLUMNS = frozenset("""title seed_label chain_name cm_task_title retired_reason
approved_title approved_artifact_body approved_task_title approved_plan task_title""".split())
PATH_COLUMNS = frozenset("cwd sub_agent_cwd_override worktree_path approved_task_file".split())
LABEL_COLUMNS = frozenset("desired_base_branch branch_name base_branch worktree_fingerprint".split())
NUMERIC_COLUMNS = frozenset("""sequence_id user_initiated archived clear_watermark
transcript_generation coordinator_head auto_continue_on_context_exhaustion created_at_us""".split())
DATE_COLUMNS = frozenset("created_at updated_at state_updated_at retired_at".split())
ENUMS = {
    "kind": {"ordinary", "coordinator"}, "ordinary_lifecycle": {"open", "history"},
    "authority_kind": {"direct", "restricted_explore", "work"}, "lifecycle": {"active", "retired"},
    "environment_kind": {"allocated_worktree", "unowned_cwd", "none"},
    "runtime_role": {"user", "sub_agent", "coordinator"},
    "cm_kind": {"direct", "branch", "explore", "work", "detached_product_creation", "attached_work_child"},
    "effort": {"none", "minimal", "low", "medium", "high", "xhigh", "max"},
    "service_tier": {"standard", "fast"}, "llm_language": {"phoenix-native", "caveman"},
    "opening_authority": {"user_authorized_instruction", "generated_predecessor_context"},
    "relation_kind": {"approved_task"},
    "message_type": {"user", "agent", "tool", "system", "error", "continuation", "skill"},
}
SPECIAL_COLUMNS = frozenset("state state_kind slug model project_id origin_kind origin_subscription_event_id steering_queue content display_data usage_data approved_priority".split())
KNOWN_COLUMNS = ID_COLUMNS | PROSE_COLUMNS | PATH_COLUMNS | LABEL_COLUMNS | NUMERIC_COLUMNS | DATE_COLUMNS | ENUMS.keys() | SPECIAL_COLUMNS


def sanitize_row(table, row, omissions):
    unknown = set(row.keys()) - KNOWN_COLUMNS
    if unknown:
        raise FixtureError(f"unclassified columns in {table}: {', '.join(sorted(unknown))}")
    result = {}
    state = {"type": "idle"}
    if "state" in row.keys():
        original = json.loads(row["state"])
        if not isinstance(original, dict):
            raise FixtureError("invalid state shape")
        if original.get("type") == "context_exhausted":
            fields(original, ("type", "summary"))
            state = {"type": "context_exhausted", "summary": synthetic(original["summary"])}
        elif original.get("type") != "idle":
            omissions["states_normalized_to_idle"] += 1
        else:
            fields(original, ("type",))
    token = hashlib.sha256(str(row["id"] if "id" in row.keys() else row["conversation_id"] if "conversation_id" in row.keys() else table).encode()).hexdigest()[:24]
    for col in row.keys():
        v = row[col]
        if col == "state":
            result[col] = dumps(state)
        elif col == "state_kind":
            result[col] = state["type"]
        elif col == "content":
            result[col] = sanitize_content(row["message_type"], v, omissions)
        elif col in ("project_id", "display_data", "usage_data") or col.startswith("origin_") and col != "origin_kind":
            omissions[col] += int(v is not None)
            result[col] = None
        elif col == "origin_kind":
            result[col] = "unknown_historical"
        elif col == "steering_queue":
            omissions["steering_queue_entries"] += len(json.loads(v))
            result[col] = "[]"
        elif v is None:
            result[col] = None
        elif col == "approved_priority":
            priority = json.loads(v)
            if priority not in {"p0", "p1", "p2", "p3", "p4"}:
                raise FixtureError("unsupported approved priority")
            result[col] = dumps(priority)
        elif col in ID_COLUMNS:
            result[col] = identity(v)
        elif col in PROSE_COLUMNS:
            result[col] = synthetic(v) if col in ("approved_artifact_body", "approved_plan") else "fixture-" + token
        elif col in PATH_COLUMNS:
            result[col] = "/tmp/phoenix-load-fixture/fixture-" + token
        elif col in LABEL_COLUMNS or col == "slug":
            result[col] = "fixture-" + token
        elif col == "model":
            result[col] = "fixture-model"
        elif col in ENUMS:
            if v not in ENUMS[col]:
                raise FixtureError(f"unsupported enum in {table}.{col}")
            result[col] = v
        elif col in NUMERIC_COLUMNS:
            if not isinstance(v, (int, float)):
                raise FixtureError("invalid numeric metadata")
            result[col] = v
        elif col in DATE_COLUMNS:
            if not isinstance(v, str) or not re.fullmatch(r"[0-9TtZz :.+-]+", v):
                raise FixtureError("invalid timestamp metadata")
            result[col] = v
        else:
            raise FixtureError(f"unclassified value in {table}.{col}")
    return result


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def open_source(path):
    path = Path(path).expanduser().resolve(strict=True)
    if not path.is_file():
        raise FixtureError("source must be a regular SQLite file")
    db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=30)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA query_only=ON")
    db.execute("BEGIN")
    return db


def new_output_directory(path):
    path = Path(path).expanduser().absolute()
    if path.exists() or path.is_symlink():
        raise FixtureError("output directory must be NEW")
    root = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True)
    if root.returncode == 0:
        repo = Path(root.stdout.strip()).resolve()
        resolved = path.resolve()
        if resolved.is_relative_to(repo):
            ignored = subprocess.run(["git", "check-ignore", "--quiet", "--no-index", str(resolved)], cwd=repo)
            if ignored.returncode != 0:
                raise FixtureError("output directory inside repository must be gitignored")
    path.mkdir(mode=0o700, parents=True)
    path.chmod(0o700)
    return path


def create_schema(source, output):
    definitions = source.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY rowid").fetchall()
    skipped = {r["name"] for r in definitions if r["type"] == "table" and re.search(r"CREATE\s+VIRTUAL\s+TABLE", r["sql"], re.I)}
    skipped |= {r["name"] for r in source.execute("PRAGMA table_list") if r["type"] == "shadow"}
    tables = {r["name"] for r in definitions if r["type"] == "table" and r["name"] not in skipped and not r["name"].startswith("sqlite_")}
    for r in definitions:
        if r["type"] == "table" and r["name"] in tables:
            output.execute(r["sql"])
    # Indexes are deferred until after bulk inserts; triggers are NEVER copied.
    return definitions, tables, skipped


def current_member(db, target):
    """Resolve the recorded continuation tail for the measurement manifest."""
    root = db.execute("SELECT id FROM conversations WHERE product_conversation_id=? AND parent_conversation_id IS NULL AND NOT EXISTS (SELECT 1 FROM conversations p WHERE p.continued_in_conv_id=conversations.id) LIMIT 1", (target,)).fetchone()
    if root is None:
        raise FixtureError("target root missing")
    row = db.execute("SELECT * FROM conversations WHERE id=?", (root[0],)).fetchone()
    seen = set()
    while row["continued_in_conv_id"] is not None:
        if row["id"] in seen:
            raise FixtureError("continuation cycle")
        seen.add(row["id"])
        row = db.execute("SELECT * FROM conversations WHERE id=?", (row["continued_in_conv_id"],)).fetchone()
    member = row["id"]
    return member


def file_hash(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def build_fixture(source_path, output_dir, target=DEFAULT_TARGET):
    start = time.monotonic()
    target = identity(target)
    source = open_source(source_path)
    directory = None
    output = None
    try:
        directory = new_output_directory(output_dir)
        db_path = directory / "fixture.db"
        fd = os.open(db_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
        output = sqlite3.connect(db_path)
        output.execute("PRAGMA journal_mode=DELETE")
        output.execute("PRAGMA foreign_keys=OFF")
        definitions, tables, skipped = create_schema(source, output)
        if not {"conversations", "messages", "product_conversations"} <= tables:
            raise FixtureError("required relational read tables missing")
        omissions = Counter()
        counts = {}
        message_types = Counter()
        tool_mix = Counter()
        source_bytes = synthetic_bytes = 0
        output.execute("BEGIN")
        for table in TABLES:
            if table not in tables:
                continue
            expected_count = source.execute(f"SELECT count(*) FROM {quote(table)}").fetchone()[0]
            counts[table] = 0
            cursor = source.execute(f"SELECT * FROM {quote(table)}")
            columns = [d[0] for d in cursor.description]
            sql = f"INSERT INTO {quote(table)} ({','.join(map(quote, columns))}) VALUES ({','.join('?' for _ in columns)})"
            batch = []
            for row in cursor:
                sanitized = sanitize_row(table, row, omissions)
                if table == "messages":
                    message_types[sanitized["message_type"]] += 1
                    source_bytes += len(row["content"].encode())
                    synthetic_bytes += len(sanitized["content"].encode())
                    if sanitized["message_type"] == "agent":
                        for block in json.loads(sanitized["content"]):
                            if block["type"] in ("tool_use", "server_tool_use", "mcp_tool_use"):
                                tool_mix[block["name"]] += 1
                batch.append(tuple(sanitized[c] for c in columns))
                counts[table] += 1
                if len(batch) >= 256:
                    output.executemany(sql, batch)
                    batch.clear()
            output.executemany(sql, batch)
            if counts[table] != expected_count:
                raise FixtureError("source snapshot row count mismatch")
        for table in sorted(tables - set(TABLES)):
            omissions["table_rows:" + table] = source.execute(f"SELECT count(*) FROM {quote(table)}").fetchone()[0]
        for r in definitions:
            if (r["type"] == "index" and r["tbl_name"] in tables) or r["type"] == "view":
                output.execute(r["sql"])
        violations = output.execute("PRAGMA foreign_key_check").fetchmany(1)
        if violations:
            raise FixtureError("sanitized fixture failed foreign_key_check")
        for table, count in counts.items():
            if output.execute(f"SELECT count(*) FROM {quote(table)}").fetchone()[0] != count:
                raise FixtureError("inserted row count mismatch")
        output.commit()
        output.execute("PRAGMA foreign_keys=ON")
        members = [r[0] for r in output.execute("SELECT id FROM conversations WHERE product_conversation_id=? ORDER BY id", (target,))]
        if not members:
            raise FixtureError("target product has no transcripts")
        target_scale = output.execute("SELECT count(*),coalesce(sum(length(cast(m.content AS BLOB))),0) FROM messages m JOIN conversations c ON c.id=m.conversation_id WHERE c.product_conversation_id=?", (target,)).fetchone()
        output.row_factory = sqlite3.Row
        latest_member = current_member(output, target)
        output.close()
        output = None
        source.rollback()
        source.close()
        db_path.chmod(0o400)
        manifest_path = directory / "manifest.json"
        manifest = {
            "format": "sanitized-conversation-load-v1", "timing_only": True,
            "target": {"product_id": target, "title": "fixture-" + hashlib.sha256(target.encode()).hexdigest()[:24], "latest_member": latest_member, "members": members, "transcript_count": len(members), "message_count": target_scale[0], "content_bytes": target_scale[1]},
            "counts": counts, "message_types": dict(message_types), "tool_mix": dict(tool_mix),
            "source_content_bytes": source_bytes, "synthetic_content_bytes": synthetic_bytes,
            "db_sha256": file_hash(db_path), "db_bytes": db_path.stat().st_size,
            "build_seconds": round(time.monotonic() - start, 3),
            "omissions": dict(sorted(omissions.items())), "skipped_fts_tables": sorted(skipped),
            "limitations": [
                "Timing/scale only: synthetic prose retains lengths and newlines, not Markdown/code semantics.",
                "Opaque JSON keys/structure and tool display/usage payloads are not retained; UTF-8 serialized input size is approximated.",
                "Images/files, credentials, migrations data, jobs, effects, and all non-allowlisted rows are omitted.",
                "Non-idle/context-exhausted states become idle; runtime is never started.",
                "The normal binary serves target-scoped persisted reads; live services and command capabilities are unavailable.",
            ],
        }
        manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
        manifest_path.chmod(0o600)
        return manifest
    except BaseException:
        if output is not None:
            output.close()
        if directory is not None:
            shutil.rmtree(directory)
        raise
    finally:
        source.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", required=True, type=Path, help="explicit source database (read-only)")
    parser.add_argument("--output-dir", required=True, type=Path, help="NEW private directory, gitignored inside a repository")
    parser.add_argument("--target", default=DEFAULT_TARGET)
    args = parser.parse_args(argv)
    old_umask = os.umask(0o077)
    try:
        manifest = build_fixture(args.source, args.output_dir, args.target)
        print(dumps({"counts": manifest["counts"], "target_transcripts": manifest["target"]["transcript_count"], "target_messages": manifest["target"]["message_count"], "db_sha256": manifest["db_sha256"], "build_seconds": manifest["build_seconds"]}))
        return 0
    except FixtureError as error:
        print(f"Fixture build failed closed: {error}. No completed output.", file=sys.stderr)
        return 1
    except (sqlite3.Error, OSError, ValueError, TypeError, KeyError):
        # Do not print exception values: JSON/SQLite errors can contain source data.
        print("Fixture build failed closed; no completed output. Inspect schema/content classifications with safe metadata only.", file=sys.stderr)
        return 1
    finally:
        os.umask(old_umask)


if __name__ == "__main__":
    sys.exit(main())
