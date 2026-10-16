#!/usr/bin/env python3
"""Select workspace package tests from Git changes and rust-analyzer's crate graph."""

import argparse
import asyncio
from collections import defaultdict, deque
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile


class AnalysisError(Exception):
    pass


def command(root, *args):
    result = subprocess.run(args, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise AnalysisError(f"{shlex.join(args)}: {os.fsdecode(result.stderr).strip()}")
    return os.fsdecode(result.stdout)


def revision(root, ref):
    return command(root, "git", "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").strip()


def changes(root, args):
    head = revision(root, args.head or "HEAD")
    if head != revision(root, "HEAD"):
        raise AnalysisError("Check out the requested head before analyzing its crate graph.")
    if args.worktree:
        base = head
        paths = command(root, "git", "diff", "--name-only", "--no-renames", "-z", base, "--")
        paths += command(root, "git", "ls-files", "--others", "--exclude-standard", "-z")
    else:
        if command(root, "git", "status", "--porcelain", "--untracked-files=all"):
            raise AnalysisError("Working tree has modified or untracked files; commit/stash them or use --worktree.")
        if args.commit:
            head = revision(root, args.commit)
            if head != revision(root, "HEAD"):
                raise AnalysisError("Check out the requested commit before analyzing its crate graph.")
            parents = command(root, "git", "rev-list", "--parents", "-n", "1", head).split()[1:]
            base = parents[0] if parents else None
        else:
            base = command(root, "git", "merge-base", revision(root, args.base), head).strip()
        if base:
            paths = command(root, "git", "diff", "--name-only", "--no-renames", "-z", base, head, "--")
        else:
            paths = command(root, "git", "ls-tree", "-r", "--name-only", "-z", head)
    return base, head, sorted(set(paths.rstrip("\0").split("\0")) - {""})


def workspace_packages(root):
    metadata = json.loads(command(root, "cargo", "metadata", "--no-deps", "--locked", "--all-features", "--format-version", "1"))
    members = set(metadata["workspace_members"])
    packages = {p["name"]: p for p in metadata["packages"] if p["id"] in members}
    if not packages:
        raise AnalysisError("Cargo returned no workspace packages.")
    return packages


class Analyzer:
    def __init__(self, process):
        self.process = process
        self.sequence = 0
        self.status = None

    async def send(self, message):
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        await self.process.stdin.drain()

    async def receive(self):
        headers = await self.process.stdout.readuntil(b"\r\n\r\n")
        fields = dict(line.split(b":", 1) for line in headers.strip().split(b"\r\n"))
        length = int(next(value for key, value in fields.items() if key.lower() == b"content-length"))
        message = json.loads(await self.process.stdout.readexactly(length))
        if message.get("method") == "experimental/serverStatus":
            self.status = message["params"]
        if "method" in message and "id" in message:
            if message["method"] == "window/workDoneProgress/create":
                await self.send({"id": message["id"], "result": None})
            else:
                await self.send({"id": message["id"], "error": {"code": -32601, "message": "Unsupported client request"}})
        return message

    async def request(self, method, params):
        self.sequence += 1
        identifier = self.sequence
        await self.send({"id": identifier, "method": method, "params": params})
        while True:
            message = await self.receive()
            if "method" not in message and message.get("id") == identifier:
                if "error" in message:
                    raise AnalysisError(f"{method}: {message['error']}")
                return message["result"]


async def analyzer_graph(root, executable, timeout, target):
    with tempfile.TemporaryFile() as log:
        process = await asyncio.create_subprocess_exec(
            executable, cwd=root, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=log,
        )
        client = Analyzer(process)
        try:
            async with asyncio.timeout(timeout):
                cargo = {"features": "all", "buildScripts": {"enable": False}, "sysroot": None}
                if target:
                    cargo["target"] = target
                await client.request("initialize", {
                    "processId": os.getpid(), "rootUri": root.as_uri(),
                    "capabilities": {"experimental": {"serverStatusNotification": True}},
                    "initializationOptions": {
                        "cargo": cargo, "procMacro": {"enable": False},
                        "checkOnSave": False, "cachePriming": {"enable": False},
                    },
                })
                await client.send({"method": "initialized", "params": {}})
                while not client.status or not client.status.get("quiescent"):
                    await client.receive()
                if client.status["health"] != "ok":
                    raise AnalysisError(f"rust-analyzer workspace is incomplete: {client.status.get('message')}")
                graph = await client.request("rust-analyzer/viewCrateGraph", {"full": False})
                await client.request("shutdown", None)
                await client.send({"method": "exit", "params": None})
                await process.wait()
                return graph
        except (TimeoutError, EOFError, asyncio.IncompleteReadError, BrokenPipeError) as error:
            log.seek(0)
            detail = log.read().decode(errors="replace")[-2000:].strip()
            raise AnalysisError(f"rust-analyzer did not complete ({type(error).__name__}): {detail}") from error
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()


def package_graph(dot, packages):
    """Map RA targets to packages; retain declared edges hidden by target cfgs."""
    targets = defaultdict(set)
    manifests = {str(Path(p["manifest_path"]).parent.resolve()): name for name, p in packages.items()}
    declared = {name: set() for name in packages}
    for name, package in packages.items():
        for target in package["targets"]:
            targets[target["name"].replace("-", "_")].add(name)
        for dependency in package["dependencies"]:
            if dependency.get("path"):
                owner = manifests.get(str(Path(dependency["path"]).resolve()))
                if owner and owner != name:
                    declared[name].add(owner)

    nodes, edges = {}, []
    lines = dot.strip().splitlines()
    if not lines or lines[0].strip() != "digraph rust_analyzer_crate_graph {" or lines[-1].strip() != "}":
        raise AnalysisError("Unsupported rust-analyzer crate graph format.")
    for line in lines[1:-1]:
        node = re.fullmatch(r'\s*(\w+)\[label="([\w-]+)"\]\[shape="box"\];\s*', line)
        edge = re.fullmatch(r'\s*(\w+) -> (\w+)\[label=""\];\s*', line)
        if node:
            identifier, label = node.groups()
            if label not in targets or identifier in nodes:
                raise AnalysisError(f"Unmapped rust-analyzer crate: {label}")
            nodes[identifier] = targets[label]
        elif edge:
            edges.append(edge.groups())
        else:
            raise AnalysisError(f"Unsupported rust-analyzer graph entry: {line}")
    represented = set().union(*nodes.values()) if nodes else set()
    if represented != set(packages):
        raise AnalysisError(f"Missing workspace packages in rust-analyzer: {sorted(set(packages) - represented)}")
    dependencies = {name: set() for name in packages}
    for source, dest in edges:
        if source not in nodes or dest not in nodes:
            raise AnalysisError("rust-analyzer graph references an unknown target.")
        for owner in nodes[source]:
            # DOT labels do not identify a package. Declarations disambiguate
            # duplicate integration-test and build-script target names.
            dependencies[owner].update(nodes[dest] & declared[owner])
    supplements = []
    for owner in sorted(packages):
        for dependency in sorted(declared[owner] - dependencies[owner]):
            supplements.append(f"{owner} -> {dependency}")
        # RA loads one target configuration and may omit inactive dependencies.
        dependencies[owner].update(declared[owner])
    return dependencies, supplements


def classify(root, paths, packages):
    roots = sorted(
        ((Path(p["manifest_path"]).parent.relative_to(root), name) for name, p in packages.items()),
        key=lambda item: len(item[0].parts), reverse=True,
    )
    direct, ignored, full = defaultdict(list), [], []
    for filename in paths:
        path = Path(filename)
        if path.name in {"Cargo.toml", "Cargo.lock"}:
            full.append(f"Dependency configuration changed: {filename}")
            continue
        owner = next((name for directory, name in roots if path.is_relative_to(directory)), None)
        if owner:
            direct[owner].append(filename)
        elif filename.startswith(("docs/", "openspec/")) or filename in {"README.md", "CONTRIBUTING.md", "AGENTS.md"}:
            ignored.append(filename)
        else:
            # Shared fixtures, build inputs and unknown paths may be consumed
            # outside the declared crate graph, including by generated runners.
            full.append(f"Shared or unclassified input changed: {filename}")
    return dict(direct), ignored, full


def select(direct, dependencies):
    reverse = defaultdict(set)
    for owner, deps in dependencies.items():
        for dependency in deps:
            reverse[dependency].add(owner)
    reasons = {name: ["Changed files: " + ", ".join(files)] for name, files in direct.items()}
    pending = deque(sorted(direct))
    while pending:
        dependency = pending.popleft()
        for owner in sorted(reverse[dependency]):
            if owner not in reasons:
                reasons[owner] = [f"Depends on affected package: {dependency}"]
                pending.append(owner)
    return reasons


def make_plan(root, args):
    base, head, paths = changes(root, args)
    packages = workspace_packages(root)
    direct, ignored, fallback = classify(root, paths, packages)
    dependencies, supplements = {}, []
    if direct and not fallback:
        try:
            dot = asyncio.run(analyzer_graph(root, args.rust_analyzer, args.timeout, args.target))
            dependencies, supplements = package_graph(dot, packages)
        except (AnalysisError, OSError, ValueError, KeyError, TypeError) as error:
            fallback.append(f"Dependency analysis failed; selecting all workspace packages: {error}")
    reasons = {name: list(fallback) for name in packages} if fallback else select(direct, dependencies)
    selected = sorted(reasons)
    argv = []
    if selected:
        argv = ["cargo", "nextest", "run", "--locked", "--all-targets", "--all-features"]
        if args.target:
            argv += ["--target", args.target]
        for name in selected:
            argv += ["-p", name]
    return {
        "base": base, "head": head, "mode": "all" if fallback else "affected" if selected else "none",
        "changed_files": paths, "direct_packages": sorted(direct), "packages": selected,
        "ignored_files": ignored, "reasons": reasons, "fallback_reasons": fallback,
        "dependencies": {name: sorted(deps) for name, deps in sorted(dependencies.items())},
        "supplemented_edges": supplements, "command": argv,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--commit", help="Compare a checked-out commit with its first parent")
    source.add_argument("--base", help="Compare the merge base of this local ref and --head with --head")
    source.add_argument("--worktree", action="store_true", help="Include staged, unstaged and untracked changes against HEAD")
    parser.add_argument("--head", help="PR head ref, which must be checked out (default: HEAD)")
    parser.add_argument("--target", help="Target triple for rust-analyzer and nextest")
    parser.add_argument("--rust-analyzer", default="rust-analyzer", help="rust-analyzer executable")
    parser.add_argument("--timeout", type=float, default=120, help="Total analyzer timeout in seconds (default: 120)")
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--json", action="store_true", help="Print a machine-readable plan")
    output.add_argument("--run", action="store_true", help="Execute the selected nextest command")
    args = parser.parse_args()
    if args.head and not args.base:
        parser.error("--head requires --base")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    try:
        root = Path(command(Path.cwd(), "git", "rev-parse", "--show-toplevel").strip()).resolve()
        plan = make_plan(root, args)
        if args.json:
            print(json.dumps(plan, indent=2))
        else:
            print(f"Selection: {plan['mode']}")
            for name in plan["packages"]:
                print(f"  {name}: {'; '.join(plan['reasons'][name])}")
            if plan["command"]:
                print(shlex.join(plan["command"]), flush=True)
            else:
                print("No affected Rust package tests.")
        if args.run and plan["command"]:
            return subprocess.run(plan["command"], cwd=root).returncode
        return 0
    except (AnalysisError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
