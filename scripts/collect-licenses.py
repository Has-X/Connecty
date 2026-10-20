"""Collect upstream license texts for the locked Windows agent dependency graph."""
import json
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parent.parent
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--manifest-path", str(root / "agent/Cargo.toml"),
    "--locked", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"
], text=True))
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
packages = {p["id"]: p for p in metadata["packages"]}
seen, pending = set(), [metadata["resolve"]["root"]]
while pending:
    identity = pending.pop()
    if identity in seen:
        continue
    seen.add(identity)
    for dep in nodes[identity]["deps"]:
        if any(k["kind"] != "dev" for k in dep["dep_kinds"]):
            pending.append(dep["pkg"])
texts, missing = [], []
for p in sorted((packages[i] for i in seen if packages[i]["source"]), key=lambda p: (p["name"], p["version"])):
    directory = pathlib.Path(p["manifest_path"]).parent
    files = sorted(f for f in directory.rglob("*") if f.is_file() and
                   f.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")) and
                   f.suffix.lower() not in (".rs", ".html", ".json", ".toml"))
    if p.get("license_file"):
        files = sorted(set(files + [directory / p["license_file"]]))
    if not files and "Apache-2.0" in (p.get("license") or ""):
        texts.append(f"\n{'=' * 72}\n{p['name']} {p['version']}\nSource: {p.get('repository')}\nAuthors: {', '.join(p.get('authors', []))}\n")
        texts.append((root / "assets/licenses/Apache-2.0.txt").read_text(encoding="utf-8"))
        continue
    if not files:
        missing.append(p["name"])
    texts.append(f"\n{'=' * 72}\n{p['name']} {p['version']}\nLicense: {p.get('license')}\nSource: {p.get('repository') or p['source']}\n")
    for f in files:
        texts.append(f"\n--- {f.relative_to(directory)} ---\n{f.read_text(encoding='utf-8', errors='replace')}\n")
if missing:
    raise SystemExit("Missing license text: " + ", ".join(missing))
destination = root / "THIRD-PARTY-LICENSES.txt"
destination.write_text("Upstream Rust dependency license texts, including build dependencies.\n" + "".join(texts), encoding="utf-8")
print(f"Collected {len(seen) - 1} package notices ({destination.stat().st_size} bytes)")
