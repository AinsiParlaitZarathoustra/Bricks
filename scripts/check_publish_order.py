#!/usr/bin/env python3
"""Check the crate list of `sync_cargo.sh` against the workspace graph.

Usage: check_publish_order.py CRATE...   (in publication order)

Reads `cargo metadata --no-deps --locked` and refuses, before anything is
published, a list where:

* a name is not a workspace member;
* a crate is private (`publish = false`);
* a crate appears twice;
* a normal, build or optional dependency on another workspace crate is
  missing from the list, or comes after its dependent (crates.io needs every
  dependency of a published manifest to exist first);
* a dev-dependency on a workspace crate carries a version (it would stay in
  the published manifest) while that crate is private or not listed.
  Path-only dev-dependencies are stripped by `cargo publish` and are fine.

Exit 0 when the list is coherent, 1 with one line per problem otherwise.
"""

import json
import subprocess
import sys


def main(argv):
    order = argv[1:]
    if not order:
        print("check_publish_order: no crate given", file=sys.stderr)
        return 2
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    members = {p["name"]: p for p in meta["packages"]}
    position = {}
    problems = []
    for i, name in enumerate(order):
        if name in position:
            problems.append(f"{name}: listed twice")
        position.setdefault(name, i)
    for name in order:
        pkg = members.get(name)
        if pkg is None:
            problems.append(f"{name}: not a workspace crate")
            continue
        if pkg.get("publish") == []:
            problems.append(f"{name}: private (publish = false), cannot be published")
        for dep in pkg["dependencies"]:
            dep_name = dep["name"]
            if dep_name not in members or dep_name == name:
                continue
            kind = dep.get("kind") or "normal"
            if kind == "dev":
                versioned = dep.get("req", "*") != "*"
                private = members[dep_name].get("publish") == []
                if versioned and (private or dep_name not in position):
                    problems.append(
                        f"{name}: dev-dependency {dep_name} has a version but is "
                        f"{'private' if private else 'not published by this list'}"
                    )
                continue
            what = f"{kind}{', optional' if dep.get('optional') else ''}"
            if dep_name not in position:
                problems.append(f"{name}: depends on {dep_name} ({what}), which is not in the list")
            elif position[dep_name] > position[name]:
                problems.append(f"{name}: depends on {dep_name} ({what}), listed after it")
    for p in problems:
        print(f"publish order: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
