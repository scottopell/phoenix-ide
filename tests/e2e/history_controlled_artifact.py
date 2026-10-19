"""Admission firewall for separately built stock and controlled UI artifacts."""
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import subprocess

SHA = re.compile(r"[0-9a-f]{64}\Z")
MODES = {
    "stock_natural": (0.25, "ui/dist", "ui/src/hooks/conversationViewReporterFactory.ts", "ui/vite.config.ts"),
    "controlled_functional": (1, "ui/dist-controlled-functional", "ui/qualification/conversationViewReporterFactory.controlled.ts", "ui/vite.controlled-functional.config.ts"),
}

@dataclass(frozen=True)
class StockArtifact:
    root: Path
    manifest_sha256: str
    ui_files: tuple[tuple[str, str], ...]

@dataclass(frozen=True)
class ControlledArtifact:
    root: Path
    manifest_sha256: str
    ui_files: tuple[tuple[str, str], ...]


def _file(root, relative):
    path = Path(relative)
    if path.is_absolute() or not path.parts or any(p in {".", ".."} for p in path.parts):
        raise ValueError("artifact paths must be relative and contained")
    result = root / path
    current = result
    while current != root:
        if current.is_symlink():
            raise ValueError("artifact paths must not traverse symlinks")
        current = current.parent
    if not result.is_file() or not result.resolve().is_relative_to(root):
        raise ValueError("artifact file is absent or escapes root")
    return result


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _mapping(root, value):
    if not isinstance(value, dict) or not value or len(value) > 10000:
        raise ValueError("bounded nonempty artifact file map required")
    for name, expected in value.items():
        if not isinstance(name, str) or not isinstance(expected, str) or not SHA.fullmatch(expected):
            raise ValueError("invalid artifact hash entry")
        if digest(_file(root, name)) != expected:
            raise ValueError("artifact file hash mismatch")
    return value


def admit_artifact(app, manifest_path, source_sha, source_tree, mode):
    if mode not in MODES:
        raise ValueError("unknown artifact evidence class")
    app = Path(app).resolve(strict=True)
    manifest_path = Path(manifest_path)
    expected_name = "stock-natural-build.json" if mode == "stock_natural" else "controlled-functional-build.json"
    if manifest_path.absolute() != app / "ui" / expected_name or manifest_path.is_symlink():
        raise ValueError("manifest must be the owned mode-specific build receipt")
    if manifest_path.stat().st_size > 2 * 1024 * 1024:
        raise ValueError("artifact manifest exceeds bound")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate artifact manifest key")
            result[key] = value
        return result
    _file(app, str(manifest_path.relative_to(app)))
    def git(*args):
        return subprocess.check_output(["git", "-C", str(app), *args], stderr=subprocess.DEVNULL).decode().strip()
    if git("rev-parse", "HEAD") != source_sha or git("rev-parse", "HEAD^{tree}") != source_tree or git("status", "--porcelain", "--untracked-files=no"):
        raise ValueError("artifact source must match clean pinned Git HEAD/tree")
    receipt = json.loads(manifest_path.read_text(), object_pairs_hook=unique)
    rate, output, factory, config = MODES[mode]
    required = {"schema_version", "evidence_class", "sample_rate", "source_sha", "source_tree", "factory_relative", "config_relative", "factory_sha256", "config_sha256", "ui_files", "resolved_module_files", "graph_scope", "source_dirty", "guard_relative", "guard_sha256", "admission", "full_acceptance", "overhead_clearance"}
    if not isinstance(receipt, dict) or set(receipt) != required:
        raise ValueError("artifact receipt shape mismatch")
    if (type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1
            or receipt["evidence_class"] != mode or type(receipt["sample_rate"]) not in (int, float)
            or receipt["sample_rate"] != rate or receipt["source_sha"] != source_sha
            or receipt["source_tree"] != source_tree or receipt["factory_relative"] != factory
            or receipt["config_relative"] != config or receipt["graph_scope"] != "ui_source_not_dependency_provenance"):
        raise ValueError("artifact mode or source identity mismatch")
    guard = "ui/qualification/viewReporterBuild.ts"
    if (receipt['source_dirty'] is not False or receipt['guard_relative'] != guard
            or receipt['guard_sha256'] != digest(_file(app, guard))
            or receipt['admission'] != ('stock' if mode == 'stock_natural' else 'cfgtest_loopback_preview_only')
            or receipt['full_acceptance'] is not False or receipt['overhead_clearance'] is not False):
        raise ValueError('artifact guard or evidence claims mismatch')
    for label, relative in (("factory", factory), ("config", config)):
        if receipt[label + "_sha256"] != digest(_file(app, relative)):
            raise ValueError("artifact factory/config mismatch")
    graph = _mapping(app, receipt["resolved_module_files"])
    for name in set(graph) | {factory, config, guard}:
        if not name.startswith('ui/'):
            raise ValueError('foreign source graph path')
        blob = subprocess.check_output(['git', '-C', str(app), 'show', f'{source_sha}:{name}'], stderr=subprocess.DEVNULL)
        if hashlib.sha256(blob).hexdigest() != digest(_file(app, name)):
            raise ValueError('source graph differs from pinned Git blob')
    if factory not in graph:
        raise ValueError("resolved graph must contain the selected factory")
    if mode == "stock_natural" and any(name.startswith("ui/qualification/") for name in graph):
        raise ValueError("stock graph contains controlled qualification source")
    if mode == "controlled_functional" and MODES["stock_natural"][2] in graph:
        raise ValueError("controlled graph contains conflicting stock factory")
    root = app / output
    _file(app, output + '/index.html')
    files = _mapping(app, receipt["ui_files"])
    if any(not name.startswith(output + '/') for name in files):
        raise ValueError('asset outside selected mode output')
    actual = {str(path.relative_to(app)) for path in root.rglob("*") if path.is_file()}
    if actual != set(files) or output + '/index.html' not in files:
        raise ValueError("UI asset inventory mismatch")
    cls = StockArtifact if mode == "stock_natural" else ControlledArtifact
    return cls(root, digest(manifest_path), tuple(sorted((name.removeprefix(output + '/'), value) for name, value in files.items())))


def build_owner_binding(app):
    """Inventory to seal immediately after both builds in the hosted owner step."""
    app = Path(app).resolve(strict=True)
    return {mode: {
        'manifest_sha256': digest(_file(app, 'ui/' + ('stock-natural-build.json' if mode == 'stock_natural' else 'controlled-functional-build.json'))),
        'ui_files': {str(p.relative_to(app)): digest(p) for p in sorted((app / spec[1]).rglob('*')) if p.is_file()},
    } for mode, spec in MODES.items()}


def require_build_owner_binding(app, owner):
    if owner.get('ui_artifacts') != build_owner_binding(app):
        raise ValueError('UI artifacts differ from independent hosted build-owner receipt')


def require_stock(artifact):
    if type(artifact) is not StockArtifact:
        raise ValueError("stock and overhead evidence requires validated stock artifact")
    return artifact


def require_controlled(artifact):
    if type(artifact) is not ControlledArtifact:
        raise ValueError("controlled evidence requires validated controlled artifact")
    return artifact
