#!/usr/bin/env python3
"""Pin each binary's contract and falsify it with synthetic filesystem probes.

No result counters are witnesses here. Snapshots, source digests, real restored
bytes and binary symbols are observed independently of the tool's stdout.
These finite probes do not prove absence of reads or subprocess networking.
"""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import tarfile
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parent.parent


def require(condition, claim):
    if not condition:
        raise AssertionError(claim)


def snapshot(root):
    """lstat links; never walk or read their targets. Hash synthetic bytes."""
    rows = {}
    for path in sorted(root.rglob("*")):
        metadata = path.lstat()
        kind = stat.S_IFMT(metadata.st_mode)
        value = None
        if stat.S_ISREG(metadata.st_mode):
            value = hashlib.sha256(path.read_bytes()).hexdigest()
        elif stat.S_ISLNK(metadata.st_mode):
            value = os.readlink(path)
        rows[str(path.relative_to(root))] = (
            kind, stat.S_IMODE(metadata.st_mode), metadata.st_size,
            metadata.st_mtime_ns, value,
        )
    return rows


def canonical(contract):
    result = dict(contract)
    result["operation_id"] = "<per-invocation>"
    result["tool_version"] = "<manifest-version>"
    return result


def pinned_encoding(value):
    # Python equality considers 0 == False and 30.0 == 30. Preserve JSON types
    # as well as values when pinning a machine interface.
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def negative_controls(tool, binary):
    """Prove the witness rejects both declaration drift and behaviour drift."""
    with tempfile.TemporaryDirectory(prefix="etudes-contract-controls-") as directory:
        root = Path(directory).resolve()
        for mutation, expected in (
            ("schema", "without a schema_version bump"),
            ("schema_type", "without a schema_version bump"),
            ("behaviour", "archive list left mutations behind" if tool == "unpack"
             else "tool mutated outside its selected tree"),
        ):
            wrapper = root / f"mutant-{mutation}"
            # This wrapper keeps the real binary's stdout and exit status. The
            # behaviour mutant advertises the exact valid contract but changes
            # a file outside the permitted effects after a real operation.
            wrapper.write_text(
                "#!/usr/bin/env python3\n"
                "import json, pathlib, subprocess, sys\n"
                f"real = {str(binary.resolve())!r}\n"
                "result = subprocess.run([real, *sys.argv[1:]], capture_output=True)\n"
                "output = result.stdout\n"
                f"mutation = {mutation!r}\n"
                "if sys.argv[1:2] == ['contract'] and mutation == 'schema' and result.returncode == 0:\n"
                "    data = json.loads(output)\n"
                "    del data['overwrite']\n"
                "    output = json.dumps(data).encode()\n"
                "elif sys.argv[1:2] == ['contract'] and mutation == 'schema_type' and result.returncode == 0:\n"
                "    data = json.loads(output)\n"
                "    data['evidence']['access_verified'] = 0\n"
                "    output = json.dumps(data).encode()\n"
                "elif sys.argv[1:2] != ['contract'] and mutation == 'behaviour':\n"
                "    sentinel = pathlib.Path.cwd() / 'outside/sentinel'\n"
                "    if sentinel.is_file(): sentinel.write_bytes(b'outside mutation')\n"
                "    if '--list' in sys.argv:\n"
                "        pathlib.Path(sys.argv[1]).write_bytes(b'archive mutation')\n"
                "sys.stdout.buffer.write(output)\n"
                "sys.stderr.buffer.write(result.stderr)\n"
                "sys.exit(result.returncode)\n"
            )
            wrapper.chmod(0o700)
            work = root / mutation
            work.mkdir()
            probe = Probe(tool, wrapper, work)
            try:
                probe.query()
                if tool == "unpack":
                    probe.unpack()
                else:
                    probe.custody()
            except AssertionError as error:
                require(expected in str(error),
                        f"{mutation} control failed for an unrelated reason: {error}")
            else:
                raise AssertionError(f"witness accepted the {mutation} mutant")


class Probe:
    def __init__(self, tool, binary, directory):
        self.tool = tool
        self.binary = str(binary.resolve())
        self.directory = directory
        self.state = directory / "state"
        self.state.mkdir()
        self.environment = dict(os.environ)
        self.environment.update(
            ETUDE_STATE_DIR=str(self.state),
            ETUDE_JOURNAL_KEY=secrets.token_hex(32),
            SWEEP_GRACE_SECS="0",
            TMPDIR=str(directory),
        )

    def run(self, *arguments, codes=(0,), environment=None):
        result = subprocess.run(
            [self.binary, *map(str, arguments)], cwd=self.directory,
            env=environment or self.environment, capture_output=True, timeout=30,
        )
        # Do not put the secret, or any environment values, in a failure report.
        require(result.returncode in codes,
                f"{self.tool}: {arguments[0] if arguments else 'default'} "
                f"exited {result.returncode}, expected {codes}")
        require(self.environment["ETUDE_JOURNAL_KEY"].encode() not in
                result.stdout + result.stderr, "a supplied key was disclosed")
        return result

    def query(self):
        for owner in ("sweep", "stash"):
            old = self.state / f"{owner}-old.journal"
            old.write_bytes(b"synthetic expired journal; contract must not prune")
            os.utime(old, (1, 1))
        before = snapshot(self.directory)
        environment = dict(self.environment, ETUDE_JOURNAL_KEY="malformed-do-not-echo")
        output = self.run("contract", "--json", environment=environment)
        require(b"malformed-do-not-echo" not in output.stdout + output.stderr,
                "contract disclosed a supplied environment value")
        contract = json.loads(output.stdout)
        require(contract["tool"] == self.tool, "wrong tool in contract")
        require(contract["kind"] == "capability_contract", "wrong contract kind")
        require(contract["status"] == "done", "contract status must be done")
        require(isinstance(contract["operation_id"], str) and contract["operation_id"],
                "operation_id must be a nonempty string")
        require(contract["contract_query"] ==
                {"operation": "contract", "reads": [], "writes": []},
                "contract must declare no application filesystem access")
        require(snapshot(self.directory) == before,
                "contract mutated files, migrated state or pruned expired journals")
        plain = json.loads(self.run("contract").stdout)
        require(canonical(plain) == canonical(contract), "plain contract disagrees")
        require(plain["operation_id"] != contract["operation_id"],
                "operation_id was reused")
        for arguments in (("--yes",), ("--json", "extra"), ("--json", "--json")):
            self.run("contract", *arguments, codes=(2,), environment=environment)
        require(snapshot(self.directory) == before,
                "invalid contract request had filesystem side effects")
        version = self.run("--version").stdout.decode().strip().split()[-1]
        require(contract["tool_version"] == version, "contract tool_version is stale")
        require(type(contract["schema_version"]) is int, "schema version must be an integer")
        pin = ROOT / "docs/contracts" / f"v{contract['schema_version']}" / f"{self.tool}.json"
        require(pin.is_file(), "schema version has no retained contract pin")
        require(pinned_encoding(canonical(contract)) == pinned_encoding(json.loads(pin.read_text())),
                "contract schema or semantics changed without a schema_version bump; "
                "retain the old pin and add a new version")
        self.contract = contract
        # The query immutability probe has finished; remove its fake journals.
        for path in self.state.glob("*.journal"):
            path.unlink()

    def operation(self, name):
        return next(row for row in self.contract["mutation_scope"]["operations"]
                    if row["operation"] == name)

    def network(self):
        require(self.contract["network"]["policy"] == "none", "network policy drift")
        result = subprocess.run(["nm", "-u", self.binary], capture_output=True, timeout=30)
        require(result.returncode == 0, "nm could not inspect the binary")
        symbols = [line.split()[-1].split("@")[0].removeprefix("_")
                   for line in result.stdout.decode().splitlines() if line.strip()]
        require(len(symbols) > 20, "nm saw too few symbols; witness is broken")
        forbidden = {"socket", "connect", "bind", "getaddrinfo", "gethostbyname",
                     "SSL_connect", "SSL_new"}
        require(not forbidden.intersection(symbols), "binary links direct networking symbols")
        require(self.contract["network"]["subprocess_network_verified"] is False,
                "a symbol scan does not verify subprocess network behaviour")
        require(self.contract["evidence"]["access_verified"] is False,
                "a declaration cannot certify absence of access")

    def custody(self):
        tree = self.directory / "selected"
        tree.mkdir()
        outside = self.directory / "outside"
        outside.mkdir()
        (outside / "sentinel").write_bytes(b"outside scope")
        names = ["Screenshot 2025-01-01 at 1.00.00 PM.png",
                 "Screenshot 2025-01-01 at 1.01.00 PM.png",
                 "Screenshot 2025-01-01 at 1.02.00 PM.png"]
        for name in names:
            (tree / name).write_bytes(b"synthetic private text 123-45-6789")
        sensitive = tree / "Tax return 2024.pdf"
        sensitive.write_bytes(b"synthetic sensitive-name fixture")
        hidden = tree / ".credential"
        hidden.write_bytes(b"synthetic hidden fixture")
        (tree / "link").symlink_to(outside / "sentinel")
        before = snapshot(tree)
        outside_before = snapshot(outside)
        require(self.contract["observation_scope"]["default_user_content"] == "metadata_only",
                "custody metadata scope changed")
        require(self.contract["overwrite"]["user_destinations"] == "refused",
                "custody overwrite policy changed")
        require(self.contract["deletion"]["user_payloads"] == "never",
                "custody payload deletion policy changed")
        require(self.contract["reversibility"]["mode"] == "conditional_journal_restore",
                "custody recovery declaration changed")
        if self.tool == "sweep":
            require(self.operation("scan")["writes"] == [], "scan declared mutation")
            self.run(tree, "--since", "0", "--json")
            require(snapshot(tree) == before, "scan changed selected files")
            # Noninteractive consent is declined; the metadata plan still runs.
            self.run(tree, "--since", "0", "--inspect-content")
            require(snapshot(tree) == before, "refused inspection mutated files")
            require("selected_tree_entries" in self.operation("apply")["writes"],
                    "apply moved entries outside its declared mutation scope")
            self.run("apply", tree, "--yes", "--since", "0")
            for name in names:
                require((tree / "Screenshots" / name).read_bytes() ==
                        b"synthetic private text 123-45-6789", "sweep move lost bytes")
            require(sensitive.read_bytes() == b"synthetic sensitive-name fixture",
                    "sweep moved or changed a sensitive file")
            require((tree / "link").is_symlink(), "sweep moved a symlink")
            # Existing file at the restore path must survive the attempted undo.
            collision = tree / names[0]
            collision.write_bytes(b"occupied restore path")
            self.run("undo", tree, codes=(0, 2))
            require(collision.read_bytes() == b"occupied restore path", "undo overwrote a file")
            require((tree / "Screenshots" / names[0]).exists(), "collision deleted held payload")
            collision.unlink()
            self.run("undo", tree)
        else:
            require("selected_tree_entries" in self.operation("stash")["writes"],
                    "stash moved entries outside its declared mutation scope")
            self.run(tree, "--for", "1d", "--json")
            holding = list(tree.glob(".stash-*"))
            require(len(holding) == 1, "stash did not create exactly one holding directory")
            require((holding[0] / sensitive.name).read_bytes() ==
                    b"synthetic sensitive-name fixture", "stash did not hold a sensitive file")
            require((holding[0] / "link").is_symlink(), "stash followed a symlink")
            held = snapshot(tree)
            state_before = snapshot(self.state)
            self.run("status", tree, "--json")
            require(snapshot(tree) == held and snapshot(self.state) == state_before,
                    "status changed the tree or journals")
            self.run("pop", tree, "--if-due", codes=(2,))
            require(snapshot(tree) == held, "early pop moved files")
            collision = tree / names[0]
            collision.write_bytes(b"occupied restore path")
            self.run("pop", tree, codes=(0, 2))
            require(collision.read_bytes() == b"occupied restore path", "pop overwrote a file")
            require((holding[0] / names[0]).exists(), "collision deleted held payload")
            collision.unlink()
            self.run("pop", tree)
        require(snapshot(tree) == before, "journal recovery did not restore exact synthetic files")
        require(snapshot(outside) == outside_before, "tool mutated outside its selected tree")
        require(hidden.read_bytes() == b"synthetic hidden fixture", "tool touched a hidden item")
        journals = list(self.state.glob(f"{self.tool}-*.journal"))
        require(journals, "persistent state missing from declared journal namespace")
        require(self.contract["persistent_state"]["journal_namespace"] == self.tool,
                "journal namespace declaration disagrees")
        for journal in journals:
            require(names[0].encode() not in journal.read_bytes(), "journal contains plaintext paths")
        no_journal = self.directory / "no-journal"
        no_journal.mkdir()
        for name in names:
            (no_journal / name).write_bytes(b"opaque payload")
        state_before = snapshot(self.state)
        if self.tool == "sweep":
            self.run("apply", no_journal, "--yes", "--since", "0", "--no-journal")
        else:
            self.run(no_journal, "--no-journal")
        require(snapshot(self.state) == state_before, "--no-journal still wrote persistent state")
        require("--no-journal" in self.contract["reversibility"]["disabled_by"],
                "contract omits unjournaled recovery loss")

    def unpack(self):
        require(self.contract["deletion"]["source_entries"] == "never",
                "unpack source deletion declaration drift")
        require(self.contract["persistent_state"]["journal_store"] == "none",
                "unpack unexpectedly declares journals")
        require("private_archive_copy" in self.operation("list")["writes"],
                "list omits its private archive-copy mutation")
        payload = b"synthetic archive payload\n"
        formats = self.contract["formats"]["archive_extraction"]
        # Independent fixture encoding: not generated from the production table.
        encodings = {".zip": "zip", ".jar": "zip", ".tar": "tar",
                     ".tar.gz": "w:gz", ".tgz": "w:gz",
                     ".tar.bz2": "w:bz2", ".tbz": "w:bz2",
                     ".tar.xz": "w:xz", ".txz": "w:xz", ".gz": "gzip"}
        require(set(formats["supported"]) == set(encodings), "exact archive formats drifted")
        for number, (suffix, encoding) in enumerate(encodings.items()):
            archive = self.directory / ("payload" + suffix)
            destination = self.directory / f"out-{number}"
            if encoding == "zip":
                with zipfile.ZipFile(archive, "w") as writer:
                    entry = zipfile.ZipInfo("payload")
                    entry.create_system = 3
                    entry.external_attr = (stat.S_IFREG | 0o600) << 16
                    writer.writestr(entry, payload)
            elif encoding == "gzip":
                archive.write_bytes(gzip.compress(payload))
            else:
                with tarfile.open(archive, "w" if encoding == "tar" else encoding) as writer:
                    entry = tarfile.TarInfo("payload")
                    entry.size = len(payload)
                    entry.mode = 0o600
                    writer.addfile(entry, io.BytesIO(payload))
            digest = hashlib.sha256(archive.read_bytes()).digest()
            before = snapshot(self.directory)
            self.run(archive, "--list", "--json")
            require(snapshot(self.directory) == before, "archive list left mutations behind")
            self.run(archive, "--into", destination, "--json")
            after = snapshot(self.directory)
            changed = {key for key in before.keys() | after.keys()
                       if before.get(key) != after.get(key)}
            require(all(key == destination.name or key.startswith(destination.name + "/")
                        for key in changed), "extraction changed paths outside its destination")
            require((destination / "payload").read_bytes() == payload,
                    f"declared {suffix} support failed real extraction")
            require(hashlib.sha256(archive.read_bytes()).digest() == digest,
                    "extraction changed the source archive")
            published = snapshot(destination)
            self.run(archive, "--into", destination, codes=(2,))
            require(snapshot(destination) == published, "unpack overwrote an existing destination")
            require(not list(self.directory.glob(".unpack-*")), "staging cleanup left artifacts")
        require(snapshot(self.state) == {}, "unpack wrote undeclared persistent state")
        require(".dmg" in formats["unsupported"] and formats["unlisted"] == "unsupported",
                "unsupported formats were omitted")
        for suffix in [*formats["unsupported"], ".unknown"]:
            archive = self.directory / ("unsupported" + suffix)
            archive.write_bytes(b"not a supported format")
            before = snapshot(self.directory)
            output = self.run(archive, codes=(2,) if suffix == ".dmg" else (3,))
            text = output.stderr.decode()
            require("refused" in text if suffix == ".dmg" else "unsupported archive format" in text,
                    f"{suffix} was not named unsupported")
            require(snapshot(self.directory) == before, "unsupported format had filesystem effects")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tool", required=True, choices=("sweep", "stash", "unpack"))
    parser.add_argument("--bin", required=True, type=Path)
    parser.add_argument("--negative-controls", action="store_true")
    arguments = parser.parse_args()
    if arguments.negative_controls:
        negative_controls(arguments.tool, arguments.bin)
        print(f"ok {arguments.tool}: witness rejects unversioned schema and scope mutations")
        return
    with tempfile.TemporaryDirectory(prefix="etudes-contract-") as directory:
        probe = Probe(arguments.tool, arguments.bin, Path(directory).resolve())
        probe.query()
        probe.network()
        if arguments.tool == "unpack":
            probe.unpack()
        else:
            probe.custody()
    print(f"ok {arguments.tool}: contract pin, scope, formats, overwrite, custody and state probes")
    print("unproven: absence of file reads, subprocess networking, and unexercised paths; "
          "no zero counter is access evidence")


if __name__ == "__main__":
    main()
