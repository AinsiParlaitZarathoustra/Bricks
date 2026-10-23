#!/usr/bin/env python3
"""Import the local Logoscrap catalog into the desktop app (offline).

Usage: python3 -I scripts/import_file_icons.py /path/to/Logoscrap

Writes, from the catalog's manifest, SVGs and license files (nothing is
downloaded):
  public/file-icons/svg/<id>.svg      the icons the rules use, unchanged
  public/file-icons/manifest.json     provenance of each icon, the rules
  public/file-icons/LICENSE-*         upstream license files
  public/file-icons/NOTICE.md         attribution summary
  src/lib/features/file-icons/rules.json   the compact rules the app reads

The app then depends on neither the catalog's folder nor the network.
"""
import hashlib
import json
import shutil
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

APP = Path(__file__).resolve().parent.parent
PUBLIC = APP / "public" / "file-icons"
RULES = APP / "src" / "lib" / "features" / "file-icons" / "rules.json"
LICENSES = [
    "LICENSE-DEVICON.txt",
    "LICENSE-MATERIAL-ICON-THEME.txt",
    "LICENSE-VSCODE-ICONS.txt",
    "LICENSE-SIMPLE-ICONS.md",
]

# Extensions the catalog leaves to the generic icon although it has the
# language's logo (an existing rule is never replaced).
LANGUAGE_EXTENSIONS = {
    "rs": "rust",
    "ts": "typescript", "mts": "typescript", "cts": "typescript",
    "js": "javascript", "mjs": "javascript", "cjs": "javascript",
    "py": "python", "pyi": "python",
    "go": "go",
    "java": "java",
    "kt": "kotlin", "kts": "kotlin",
    "swift": "swift",
    "rb": "ruby",
    "php": "php",
    "c": "c", "h": "c",
    "cpp": "cpp", "cc": "cpp", "cxx": "cpp", "hpp": "cpp", "hh": "cpp",
    "cs": "csharp",
    "lua": "lua",
    "dart": "dart",
    "scala": "scala",
    "ex": "elixir", "exs": "elixir",
    "erl": "erlang",
    "hs": "haskell",
    "jl": "julia",
    "r": "r",
    "pl": "perl",
    "zig": "zig",
    "sol": "solidity",
    "tf": "terraform",
    "prisma": "prisma",
    "graphql": "graphql", "gql": "graphql",
    "f90": "fortran",
}

# Code fence labels → the file name resolved for them (the file rules then
# decide the icon, so a label and a file of that language agree).
LANGUAGE_ALIASES = {
    "rust": "x.rs", "rs": "x.rs",
    "typescript": "x.ts", "ts": "x.ts",
    "javascript": "x.js", "js": "x.js", "node": "x.js",
    "python": "x.py", "py": "x.py", "python3": "x.py",
    "tsx": "x.tsx", "jsx": "x.jsx",
    "go": "x.go", "golang": "x.go",
    "java": "x.java",
    "kotlin": "x.kt", "kt": "x.kt",
    "swift": "x.swift",
    "ruby": "x.rb", "rb": "x.rb",
    "php": "x.php",
    "c": "x.c",
    "cpp": "x.cpp", "c++": "x.cpp", "cxx": "x.cpp",
    "csharp": "x.cs", "cs": "x.cs", "c#": "x.cs",
    "lua": "x.lua",
    "dart": "x.dart",
    "scala": "x.scala",
    "elixir": "x.ex",
    "erlang": "x.erl",
    "haskell": "x.hs",
    "julia": "x.jl",
    "r": "x.r",
    "perl": "x.pl",
    "zig": "x.zig",
    "solidity": "x.sol",
    "terraform": "x.tf", "hcl": "x.tf",
    "graphql": "x.graphql",
    "prisma": "x.prisma",
    "shell": "x.sh", "sh": "x.sh", "bash": "x.sh", "zsh": "x.sh", "console": "x.sh",
    "powershell": "x.ps1", "ps1": "x.ps1", "pwsh": "x.ps1",
    "json": "x.json", "jsonc": "x.json",
    "yaml": "x.yaml", "yml": "x.yml",
    "toml": "x.toml",
    "markdown": "x.md", "md": "x.md",
    "html": "x.html",
    "css": "x.css",
    "scss": "x.scss",
    "svelte": "x.svelte",
    "vue": "x.vue",
    "sql": "x.sql",
    "dockerfile": "Dockerfile", "docker": "Dockerfile",
    "makefile": "Makefile", "make": "Makefile",
}


def safe_svg(data: bytes) -> None:
    """Refuse an SVG with scripts, event handlers or external references."""
    root = ET.fromstring(data)
    if not root.tag.endswith("svg"):
        raise ValueError("not an SVG root")
    for el in root.iter():
        tag = el.tag.rsplit("}", 1)[-1].lower()
        if tag in ("script", "foreignobject", "iframe", "image", "use"):
            if tag == "use":
                href = el.get("href") or el.get("{http://www.w3.org/1999/xlink}href") or ""
                if href.startswith("#"):
                    continue
            raise ValueError(f"forbidden element <{tag}>")
        for name, value in el.attrib.items():
            local = name.rsplit("}", 1)[-1].lower()
            if local.startswith("on"):
                raise ValueError(f"event handler {local}")
            if local == "href" and not value.startswith("#"):
                raise ValueError(f"external reference {value}")
            if "url(" in value and "url(#" not in value.replace(" ", ""):
                raise ValueError(f"external url in {local}")


def resolver_matches(rules, name, expected):
    """The Python twin of the app's resolver, for the import's own checks."""
    lower = name.replace("\\", "/").rsplit("/", 1)[-1].lower()
    if lower in rules["exact"]:
        got = rules["exact"][lower]
    else:
        parts = lower.split(".")
        got = rules["generic"]
        for i in range(1, len(parts)):
            suffix = ".".join(parts[i:]).lstrip(".")
            if suffix in rules["extensions"]:
                got = rules["extensions"][suffix]
                break
    if got != expected:
        raise SystemExit(f"resolver check: {name} → {got}, expected {expected}")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    src = Path(sys.argv[1]).resolve()
    manifest = json.loads((src / "manifest.json").read_text(encoding="utf-8"))
    catalog = manifest["file_catalog"]
    file_icons = {x["id"]: x for x in catalog["file_icons"]}
    logos = {x["id"]: x for x in manifest["assets"] if x["status"] == "available" and x["path"]}

    extensions = {k: v["icon_id"] for k, v in catalog["extension_rules"].items()}
    added = []
    for ext, logo in LANGUAGE_EXTENSIONS.items():
        if ext in extensions:
            continue
        if logo not in logos:
            raise SystemExit(f"{ext}: logo `{logo}` is not available in the catalog")
        extensions[ext] = logo
        added.append({"extension": ext, "icon_id": logo})
    exact = {k: v["icon_id"] for k, v in catalog["exact_filename_rules"].items()}
    folders = {
        k: v["icon_id"] for k, v in catalog["folder_name_rules"].items() if k not in ("*", ".")
    }
    generic = catalog["generic_icon_id"]
    folder_states = {"closed": "folder-closed", "open": "folder-open", "root": "folder-root"}

    used = set(extensions.values()) | set(exact.values()) | set(folders.values())
    used |= {generic} | set(folder_states.values())
    records = {}
    for icon_id in sorted(used):
        rec = file_icons.get(icon_id) or logos.get(icon_id)
        if rec is None:
            raise SystemExit(f"rule points to `{icon_id}`, which the catalog does not provide")
        records[icon_id] = rec

    if PUBLIC.exists():
        shutil.rmtree(PUBLIC)
    (PUBLIC / "svg").mkdir(parents=True)
    icons_out = []
    for icon_id, rec in records.items():
        data = (src / rec["path"]).read_bytes()
        if hashlib.sha256(data).hexdigest() != rec["sha256"]:
            raise SystemExit(f"{icon_id}: content differs from the manifest's sha256")
        safe_svg(data)
        (PUBLIC / "svg" / f"{icon_id}.svg").write_bytes(data)
        icons_out.append({
            "id": icon_id,
            "file": f"svg/{icon_id}.svg",
            "kind": "file-icon" if icon_id in file_icons else "logo",
            "name": rec.get("name") or rec.get("technology"),
            "upstream_path": rec["path"],
            "source": rec["source"],
            "project": rec.get("project") or rec.get("repository"),
            "commit": rec.get("commit"),
            "license": rec["license"],
            "license_url": rec.get("license_url"),
            "attribution": rec.get("attribution"),
            "license_note": rec.get("license_note"),
            "trademark": rec.get("trademark"),
            "sha256": rec["sha256"],
        })
    for name in LICENSES:
        shutil.copyfile(src / name, PUBLIC / name)

    rules = {
        "generic": generic,
        "folder": folder_states,
        "exact": dict(sorted(exact.items())),
        "extensions": dict(sorted(extensions.items())),
        "folders": dict(sorted(folders.items())),
        "aliases": dict(sorted(LANGUAGE_ALIASES.items())),
        "icons": {i["id"]: i["file"] for i in icons_out},
    }
    for alias, target in LANGUAGE_ALIASES.items():
        lower = target.lower()
        ext = lower.rsplit(".", 1)[-1] if "." in lower else None
        if lower not in exact and (ext is None or ext not in extensions):
            raise SystemExit(f"alias `{alias}` → `{target}` resolves to no rule")
    for name, expected in [
        ("Cargo.toml", "file-cargo-toml"),
        ("src/main.rs", "rust"),
        ("index.ts", "typescript"),
        ("App.tsx", extensions["tsx"]),
        ("script.py", "python"),
        ("archive.schema.json", extensions["schema.json"]),
        ("C:\\proj\\lib.rs", "rust"),
        ("unknown.zzz", generic),
    ]:
        resolver_matches(rules, name, expected)

    out_manifest = {
        "schema_version": 1,
        "imported_from": {
            "catalog": "Logoscrap",
            "manifest_generated_at_utc": manifest["generated_at_utc"],
            "file_catalog_generated_at_utc": catalog["generated_at_utc"],
        },
        "repositories": manifest["repositories"],
        "sources": catalog["sources"],
        "trademark_notice": manifest["trademark_notice"],
        "added_associations": {
            "note": "Bricks: extensions mapped to the language logos the catalog already provides.",
            "entries": added,
        },
        "rules": rules,
        "icons": icons_out,
    }
    (PUBLIC / "manifest.json").write_text(
        json.dumps(out_manifest, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    RULES.parent.mkdir(parents=True, exist_ok=True)
    RULES.write_text(json.dumps(rules, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")

    projects = sorted({i["project"] for i in icons_out if i["project"]})
    notice = [
        "# File icons",
        "",
        "Imported offline from the Logoscrap catalog (see `manifest.json` for each icon's",
        "source, upstream revision, license and checksum). The SVGs are unchanged.",
        "",
        manifest["trademark_notice"],
        "",
        "Upstream projects:",
        "",
        *[f"- {p}" for p in projects],
        "",
        "License files: " + ", ".join(f"`{n}`" for n in LICENSES) + ".",
        "",
    ]
    (PUBLIC / "NOTICE.md").write_text("\n".join(notice), encoding="utf-8")
    size = sum(p.stat().st_size for p in (PUBLIC / "svg").iterdir())
    print(f"{len(icons_out)} SVG ({size} bytes), {len(added)} associations added")


if __name__ == "__main__":
    main()
