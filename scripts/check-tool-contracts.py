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
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
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
    if isinstance(result.get("details"), dict) and result["details"].get("kind") == "capability_contract":
        result["details"] = canonical(result["details"])
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
            ("result_schema", "without a schema_version bump"),
            ("receipt_type", "without a schema_version bump"),
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
                "result = subprocess.run([real, *sys.argv[1:]], stdin=subprocess.DEVNULL, capture_output=True)\n"
                "output = result.stdout\n"
                f"mutation = {mutation!r}\n"
                "if sys.argv[1:2] == ['contract'] and mutation == 'schema' and result.returncode == 0:\n"
                "    data = json.loads(output)\n"
                "    del data['details']['overwrite']\n"
                "    output = json.dumps(data).encode()\n"
                "elif sys.argv[1:2] == ['contract'] and mutation == 'schema_type' and result.returncode == 0:\n"
                "    data = json.loads(output)\n"
                "    data['details']['evidence']['access_verified'] = 0\n"
                "    output = json.dumps(data).encode()\n"
                "elif '--json' in sys.argv and mutation == 'result_schema':\n"
                "    data = json.loads(output)\n"
                "    del data['recovery']\n"
                "    output = json.dumps(data).encode()\n"
                "elif '--json' in sys.argv and mutation == 'receipt_type':\n"
                "    data = json.loads(output)\n"
                "    data['observations']['scope'] = []\n"
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
            env=environment or self.environment, stdin=subprocess.DEVNULL, capture_output=True, timeout=30,
        )
        # Do not put the secret, or any environment values, in a failure report.
        require(result.returncode in codes,
                f"{self.tool}: {arguments[0] if arguments else 'default'} "
                f"exited {result.returncode}, expected {codes}")
        require(self.environment["ETUDE_JOURNAL_KEY"].encode() not in
                result.stdout + result.stderr, "a supplied key was disclosed")
        if "--json" in arguments:
            self.envelope(json.loads(result.stdout))
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
        envelope = json.loads(output.stdout)
        self.envelope(envelope)
        contract = envelope["details"]
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
        plain_envelope = json.loads(self.run("contract").stdout)
        plain = plain_envelope["details"]
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
        require(pinned_encoding(canonical(envelope)) == pinned_encoding(json.loads(pin.read_text())),
                "contract schema or semantics changed without a schema_version bump; "
                "retain the old pin and add a new version")
        current_fixture = ROOT / "docs/contracts/v1" / f"{self.tool}.json"
        require(current_fixture.read_bytes() == pin.read_bytes(), "published contract fixture differs from versioned pin")
        historical_hashes = {'sweep': '9260c537fe634784dbf1230f69326ae3fd052cb4d804d973dadbe170e0d4f72d', 'stash': 'f394ed9f0b45a357c580e6394a6c6a0b5768ded1f3909edbff8efa43b8d8e0d8', 'unpack': '714024469ed4adad5b4a255ef2e1bbe629cce92f20bd8a081d167ed10213d583'}
        historical = ROOT / "docs/contracts/history/v1" / f"{self.tool}.json"
        require(hashlib.sha256(historical.read_bytes()).hexdigest() == historical_hashes[self.tool], "historical v1 contract pin changed")
        self.contract = contract
        self.result_probes()
        # The query immutability probe has finished; remove its fake journals.
        for path in self.state.glob("*.journal"):
            path.unlink()

    def envelope(self, value):
        pin = ROOT / "docs/contracts" / f"v{value['schema_version']}" / "envelope.json"
        require(pin.is_file(), "result schema has no version pin")
        shape = json.loads(pin.read_text())
        require(sorted(value) == sorted(shape["fields"]),
                "result fields changed without a schema_version bump")
        require(value["status"] in shape["statuses"], "invalid result status")
        require(type(value["schema_version"]) is int, "invalid result schema version")
        require(isinstance(value["tool_version"], str), "invalid tool version")
        require(isinstance(value["operation_id"], str) and value["operation_id"], "invalid operation id")
        for field in ("scope", "observations", "effects", "recovery", "disclosure", "details"):
            require(type(value[field]) is dict, f"invalid {field} object")
        for field, members in shape["objects"].items():
            require(sorted(value[field]) == sorted(members), f"{field} changed without a schema_version bump")
            for member, kind in members.items():
                require(type(value[field][member]).__name__ == kind, f"{field}.{member} type changed without a schema_version bump")
        for domain in (value["scope"]["domains"], value["scope"]["startup"]):
            require(sorted(domain) == sorted(shape["domain_fields"]), "scope domains changed without a schema_version bump")
            for member, kind in shape["domain_fields"].items():
                require(type(domain[member]).__name__ == kind, "scope domain type changed without a schema_version bump")
            require(all(type(item) is str for field in ("reads", "writes") for item in domain[field]), "invalid scope domain item")
        for counter in value["effects"]["counters"]:
            require(sorted(counter) == ["count", "name"] and type(counter["name"]) is str and type(counter["count"]) is int and counter["count"] >= 0, "effects counter changed without a schema_version bump")
        require(type(value["verification"]) is list, "invalid verification array")
        for claim in value["verification"]:
            require(sorted(claim) == ["claim", "status"] and type(claim["claim"]) is str, "verification fields changed without a schema_version bump")
            require(claim["status"] in ("pass", "fail", "unproven"), "invalid verification verdict")
        receipt = value["observations"]
        require(receipt["unproven"], "receipt must disclose coverage limits")
        for row in receipt["categories"]:
            require(sorted(row) == ["attempted", "category", "failed", "observed", "verified"], "receipt fields drift")
            require(type(row["category"]) is str and all(type(row[key]) is int and row[key] >= 0 for key in ("attempted", "observed", "verified", "failed")), "invalid receipt category or count type")
            require(row["attempted"] == row["observed"] + row["failed"], "receipt outcomes do not partition attempts")
            require(0 <= row["verified"] <= row["observed"], "verification exceeds observed reads")
        require("not evidence" in value["disclosure"]["zero_counters"], "zero counter overclaims")
        self.reason(value, shape.get("reason"))

    def reason(self, value, rule):
        """Every result says whether to retry. A tool that leaves a refusal
        unclassified prints `"reason": null`; that must never pass."""
        reason = value["reason"]
        require(reason is not None,
                f"a {value['status']} result has no reason (reason is null); "
                "a caller cannot tell whether to retry")
        if rule is None:
            return
        require(type(reason) is dict, "invalid reason object")
        require(set(rule["required"]) <= set(reason) <= set(rule["required"] + rule["optional"]),
                "reason fields changed without a schema_version bump")
        require(type(reason["code"]) is str and reason["code"], "invalid reason code")
        require(reason["retry"] in rule["retry_values"], "invalid reason retry class")
        if reason["retry"] in rule["subject_required_for"]:
            require(type(reason.get("subject")) is dict and
                    type(reason["subject"].get("kind")) is str and reason["subject"]["kind"],
                    f"reason {reason['code']} has retry class {reason['retry']} but names no subject kind")
        if value["status"] in ("refused", "error"):
            require(reason["retry"] != "not_needed",
                    f"a {value['status']} result claims no retry is needed")
        # Status and reason must tell one story. A caller that reads only the
        # reason must never see success beside a result that did not finish.
        status, code = value["status"], reason["code"]
        if status == "done":
            require(code == "done", f"a done result carries reason {code}")
        elif status == "nothing_to_do":
            require(code in ("nothing_to_do", *LOOKUP_FOUND_NOTHING),
                    f"a nothing_to_do result carries reason {code}")
        else:
            require(code not in ("done", "nothing_to_do"),
                    f"a result with status {status} carries the success reason {code}")
        waiting_plan = (self.tool == "sweep" and status == "nothing_to_do" and
                        code == "nothing_to_do" and reason["retry"] == "later" and
                        reason.get("subject", {}).get("kind") == "input")
        require(waiting_plan or
                (reason["retry"] == "not_needed") == (code in ("done", "nothing_to_do")),
                f"reason {code} has retry class {reason['retry']}")

    def result_probes(self):
        for command in ("help", "--version"):
            result = self.run(command, "--json")
            require(json.loads(result.stdout)["status"] == "done", "informational result claims failure")
        empty = self.directory / "empty-tree"
        empty.mkdir()
        if self.tool != "unpack":
            result = self.run(empty, "--json", codes=(1,))
            value = json.loads(result.stdout)
            require(value["status"] == "nothing_to_do", "empty operation is not reported")
            require(value["reason"] == {"code": "nothing_to_do", "retry": "not_needed"},
                    "empty operation must not ask the caller to wait")
        if self.tool == "sweep":
            recent = self.directory / "recent-tree"
            recent.mkdir()
            (recent / "photo.jpg").write_bytes(b"recent fixture")
            result = self.run(recent, "--since", "3600", "--json", codes=(1,))
            value = json.loads(result.stdout)
            require(value["reason"]["retry"] == "later",
                    "a folder held only by the grace window must say retry later")
            require(value["reason"]["subject"]["kind"] == "input",
                    "retry later must identify the input")
            (recent / "tax_return.pdf").write_bytes(b"held for another reason")
            result = self.run(recent, "--since", "3600", "--json", codes=(1,))
            require(json.loads(result.stdout)["reason"]["retry"] == "not_needed",
                    "mixed holds must not promise waiting alone will release the folder")
            if sys.platform == "darwin":
                self.finder_aliases()
                self.locked_items()
        missing = self.directory / "does-not-exist"
        output = self.run(missing, "--json", codes=(2, 3))
        envelope = json.loads(output.stdout)
        self.envelope(envelope)
        require(envelope["status"] in ("refused", "error"), "error result claims success")
        invalid = self.run("--unknown", "--json", codes=(2,))
        self.envelope(json.loads(invalid.stdout))

    def finder_aliases(self):
        tree = self.directory / "alias-tree"
        tree.mkdir()
        alias = tree / "photo-alias.jpg"
        alias.write_bytes(b"opaque alias data must not be read or resolved")
        flags = bytearray(32)
        flags[8:10] = (0x8000).to_bytes(2, "big")
        subprocess.run(["xattr", "-wx", "com.apple.FinderInfo", flags.hex(), str(alias)],
                       stdin=subprocess.DEVNULL, check=True, capture_output=True)
        ordinary = tree / "ordinary.jpg"
        ordinary.write_bytes(b"ordinary fixture")
        flags[8:10] = (0x4000).to_bytes(2, "big")
        subprocess.run(["xattr", "-wx", "com.apple.FinderInfo", flags.hex(), str(ordinary)],
                       stdin=subprocess.DEVNULL, check=True, capture_output=True)
        (tree / "link.jpg").symlink_to(alias)
        before = snapshot(tree)
        for options in ((), ("--include-tagged",), ("--scheme", "tags", "--map", "jpg=Pictures")):
            result = self.run(tree, *options, "--json", codes=(0, 1))
            value = json.loads(result.stdout)
            require(value["details"]["skipped"].get("aliases_skipped") == 1,
                    "Finder aliases must have a separate skip count")
            require(value["details"]["skipped"]["symlinks"] == 1,
                    "Finder aliases must not inflate the symlink count")
            require(value["details"]["scanned"] == 1,
                    "only the unflagged ordinary file is a candidate")
            for row in value["observations"]["categories"]:
                if row["category"] == "fingerprint_bytes":
                    require(row["observed"] == 0,
                            "alias scan read fingerprint bytes")
        human = self.run(tree, codes=(1,)).stdout.decode()
        require("1 Finder alias skipped by flag" in human,
                "human summary must disclose the alias count")
        require(snapshot(tree) == before, "alias scan changed the fixture")

    def locked_items(self):
        tree = self.directory / "locked-tree"
        tree.mkdir()
        locked = tree / "locked.jpg"
        locked.write_bytes(b"locked fixture")
        for i in range(3):
            (tree / f"ordinary-{i}.jpg").write_bytes(b"ordinary fixture")
        subprocess.run(["chflags", "uchg", str(locked)], stdin=subprocess.DEVNULL, check=True)
        try:
            before = snapshot(tree)
            for options in ((), ("--scheme", "tags", "--map", "jpg=Pictures")):
                result = self.run(tree, *options, "--json", codes=(0, 1))
                value = json.loads(result.stdout)
                require(value["details"]["skipped"].get("locked_skipped") == 1,
                        "locked items must have a separate skip count")
                require(value["details"]["scanned"] == 3,
                        "only unlocked files are candidates")
            human = self.run(tree, codes=(0, 1)).stdout.decode()
            require("1 locked item left alone by flag" in human,
                    "human summary must disclose the locked count")
            require(snapshot(tree) == before, "locked scan changed the fixture")
        finally:
            subprocess.run(["chflags", "nouchg", str(locked)], stdin=subprocess.DEVNULL, check=True)

    def read_category(self, result, name):
        categories = json.loads(result.stdout)["observations"]["categories"]
        require(any(row["category"] == name and row["observed"] > 0 for row in categories),
                f"receipt omitted successful {name} reads in the exercised fixture")
        receipt = pinned_encoding(json.loads(result.stdout)["observations"])
        require("synthetic private text" not in receipt and "synthetic archive payload" not in receipt,
                "receipt disclosed fixture content")

    def operation(self, name):
        return next(row for row in self.contract["mutation_scope"]["operations"]
                    if row["operation"] == name)

    def network(self):
        require(self.contract["network"]["policy"] == "none", "network policy drift")
        result = subprocess.run(["nm", "-u", self.binary], stdin=subprocess.DEVNULL, capture_output=True, timeout=30)
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
            self.run(tree, "--since", "0", "--inspect-content", "--json")
            require(snapshot(tree) == before, "refused inspection mutated files")
            require("selected_tree_entries" in self.operation("apply")["writes"],
                    "apply moved entries outside its declared mutation scope")
            result = self.run("apply", tree, "--yes", "--since", "0", "--json")
            self.read_category(result, "fingerprint_bytes")
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
            result = self.run(tree, "--for", "1d", "--json")
            self.read_category(result, "fingerprint_bytes")
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

    def tags(self):
        """`sweep --scheme tags`: the declared tag scopes, and that the tool only
        adds tags, from an exported plan, moving nothing and restoring on undo.
        Tag values are written and read with the system's xattr and plutil."""
        attribute = "com.apple.metadata:_kMDItemUserTags"
        require("finder_tag_names" in self.operation("scan")["reads"],
                "the tag scheme's tag-name reads are not declared on scan")
        require("selected_tree_finder_tags" in self.operation("apply_exported_plan")["writes"],
                "apply of a tag plan omits its tag writes")
        require("selected_tree_finder_tags" in self.operation("undo")["writes"],
                "undo of a tag plan omits its tag writes")
        require("selected_tree_finder_tags" not in self.operation("scan")["writes"],
                "a scan declares tag writes")
        tree = self.directory / "tag-tree"
        tree.mkdir()
        names = ["one.pdf", "two.pdf", "three.pdf"]
        for name in names:
            (tree / name).write_bytes(b"synthetic payload")
        before_value = subprocess.run(
            ["plutil", "-convert", "binary1", "-o", "-", "-"], input=b'["Red\\n6","Old"]',
            capture_output=True, check=True).stdout
        subprocess.run(["xattr", "-wx", attribute, before_value.hex(), str(tree / "one.pdf")], check=True)

        def tag_bytes():
            rows = {}
            for name in names:
                shown = subprocess.run(["xattr", "-px", attribute, str(tree / name)],
                                       capture_output=True)
                rows[name] = bytes.fromhex(shown.stdout.decode().replace("\n", "").replace(" ", "")) \
                    if shown.returncode == 0 else None
            return rows

        def tag_names(name):
            raw = tag_bytes()[name]
            shown = subprocess.run(["plutil", "-convert", "json", "-o", "-", "-"], input=raw,
                                   capture_output=True, check=True).stdout
            return json.loads(shown)

        files_before, tags_before = snapshot(tree), tag_bytes()
        plan = self.directory / "tag-plan.bin"
        scan = self.run(tree, "--scheme", "tags", "--map", "pdf=Work", "--since", "0",
                        "--export-plan", plan, "--json")
        require(snapshot(tree) == files_before and tag_bytes() == tags_before,
                "planning a tag change wrote to the tree")
        detail = json.loads(scan.stdout)["details"]
        require(detail["new_tags"] == ["Work"] and detail["tags"][0]["files"] == 3,
                "the tag plan does not list its tag, count and new flag")
        live = self.run("apply", tree, "--scheme", "tags", "--map", "pdf=Work", "--yes",
                        "--since", "0", codes=(2,))
        require(tag_bytes() == tags_before, "the live apply path wrote tags")
        digest = detail["binding"]["plan_digest"]
        self.run("apply", "--plan", plan, "--plan-digest", digest, "--yes", "--json")
        require(snapshot(tree) == files_before, "a tag apply moved or changed a file")
        require(tag_names("one.pdf") == ["Red\n6", "Old", "Work"], "a tag apply did not only add")
        require(tag_names("two.pdf") == ["Work"], "a tag apply did not tag an untagged file")
        self.run("undo", tree)
        require(tag_bytes() == tags_before, "undo did not restore the exact previous tags")
        require(snapshot(tree) == files_before, "undo changed a file")

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
            result = self.run(archive, "--list", "--json")
            self.read_category(result, "archive_bytes")
            self.read_category(result, "os_random_bytes")
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


# A lookup that found nothing exits 1 with status nothing_to_do, and names
# what was not found. These are the only refusal codes that status may carry.
LOOKUP_FOUND_NOTHING = ("unknown_operation", "missing_input")


class Unreached(Exception):
    """A probe could not set up the condition it needs. Reported as unproven."""


def screenshots(tree, count=3):
    """Three same-kind names: the smallest tree sweep will group and move."""
    tree.mkdir()
    for number in range(1, count + 1):
        (tree / f"Screenshot 2025-01-0{number} at 1.00.00 PM.png").write_bytes(b"synthetic payload")
    return tree


def project(tree):
    screenshots(tree)
    (tree / "Cargo.toml").write_bytes(b"[package]\n")
    return tree


def zip_archive(path, members):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as writer:
        for name, payload in members:
            entry = zipfile.ZipInfo(name)
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | 0o600) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            writer.writestr(entry, payload)
    return path


def needs_denied_permissions():
    if os.geteuid() == 0:
        raise Unreached("mode bits do not deny the superuser")


def garbage_journal(probe):
    """Bytes the throwaway key cannot open, under a well-formed journal name."""
    (probe.state / f"{probe.tool}-unreadable.journal").write_bytes(b"synthetic bytes, not a sealed journal")
    return probe.run("status", "--id", "unreadable", "--json", codes=(3,))


def restore_into_read_only_folder(probe):
    """A journalled move, then a restore the folder's mode refuses. The restore
    stops part-way; afterwards the same command finishes it."""
    needs_denied_permissions()
    tree = screenshots(probe.directory / "tree")
    before = snapshot(tree)
    move = ("apply", tree, "--yes", "--since", "0") if probe.tool == "sweep" else (tree,)
    back = "undo" if probe.tool == "sweep" else "pop"
    probe.run(*move, "--json")
    tree.chmod(0o500)
    try:
        result = probe.run(back, tree, "--json", codes=(3,))
    finally:
        tree.chmod(0o700)
    probe.run(back, tree, "--json")
    require(snapshot(tree) == before, "an interrupted restore did not resume to the original tree")
    return result


def restore_onto_occupied_path(probe):
    """A journalled move, then a restore whose original path is now occupied.
    The occupying file is kept, the held file stays held, the command still
    exits 0, and the result must say the restore is not finished."""
    tree = screenshots(probe.directory / "tree")
    before = snapshot(tree)
    name = "Screenshot 2025-01-01 at 1.00.00 PM.png"
    move = ("apply", tree, "--yes", "--since", "0") if probe.tool == "sweep" else (tree,)
    back = "undo" if probe.tool == "sweep" else "pop"
    probe.run(*move, "--json")
    (tree / name).write_bytes(b"occupied restore path")
    result = probe.run(back, tree, "--json")
    require(json.loads(result.stdout)["status"] == "incomplete",
            "a restore that left a file behind is not reported as incomplete")
    require((tree / name).read_bytes() == b"occupied restore path", "a restore overwrote a file")
    require(sum(path.name == name for path in tree.rglob("*")) == 2,
            "a blocked restore lost the held payload")
    (tree / name).unlink()
    probe.run(back, tree, "--json")
    require(snapshot(tree) == before, "a blocked restore did not resume to the original tree")
    return result


def sweep_unreadable_scan(probe):
    """A folder the scan cannot read yields a partial plan, not a finished one."""
    needs_denied_permissions()
    tree = screenshots(probe.directory / "tree")
    tree.chmod(0)
    try:
        result = probe.run(tree, "--json", codes=(1,))
    finally:
        tree.chmod(0o700)
    require(json.loads(result.stdout)["status"] == "incomplete",
            "a scan that could not read its folder is not reported as incomplete")
    return result


def stale_plan(probe):
    """Export a plan, change the tree, replay the plan with its own digest."""
    tree = screenshots(probe.directory / "tree")
    plan = probe.directory / "plan.json"
    scan = (tree, "--since", "0") if probe.tool == "sweep" else (tree,)
    exported = json.loads(probe.run(*scan, "--export-plan", plan, "--json").stdout)
    digest = exported["details"]["binding"]["plan_digest"]
    (tree / "arrived after the plan.txt").write_bytes(b"synthetic late arrival")
    before = snapshot(tree)
    replay = ("apply", "--plan", plan, "--plan-digest", digest, "--yes") if probe.tool == "sweep" \
        else ("--plan", plan, "--plan-digest", digest)
    result = probe.run(*replay, "--json", codes=(2,))
    require(snapshot(tree) == before, "a stale plan still moved files")
    return result


def sweep_propose(probe, decisions, setup=None, **overrides):
    """Export a plan, hand `sweep propose` a bad proposal, require a clean refusal.

    The proposal is built from the export's own digest, so the one fault each
    probe plants is the only thing wrong with it. A refusal must move nothing
    and must write no plan."""
    tree = screenshots(probe.directory / "tree")
    (tree / "Reading").mkdir()
    if setup:
        setup(tree)
    plan = probe.directory / "plan.json"
    exported = json.loads(probe.run(tree, "--since", "0", "--export-plan", plan, "--json").stdout)
    proposal = {"schema_version": 1, "rest": "leave", "decisions": decisions,
                "plan_digest": exported["details"]["binding"]["plan_digest"]}
    proposal.update(overrides)
    proposal_file = probe.directory / "proposal.json"
    proposal_file.write_text(json.dumps(proposal))
    out = probe.directory / "out.plan"
    before = snapshot(tree)
    result = probe.run("propose", "--plan", plan, "--plan-digest",
                       exported["details"]["binding"]["plan_digest"],
                       "--proposal", proposal_file, "--export-plan", out, "--json", codes=(2,))
    require(snapshot(tree) == before, "a refused proposal moved or changed files")
    require(not out.exists(), "a refused proposal still wrote a plan")
    return result


def sweep_unknown_item(probe):
    return sweep_propose(probe, [{"item": "i-999999", "folder": "Reading", "mode": "require_existing"}])


def sweep_item_assigned_twice(probe):
    return sweep_propose(probe, [
        {"item": "i-000000", "folder": "Reading", "mode": "require_existing"},
        {"item": "i-000000", "skip": True}])


def sweep_propose_outside_root(probe):
    return sweep_propose(probe, [{"item": "i-000000", "folder": "..", "mode": "require_existing"}])


def sweep_propose_folder_absent(probe):
    return sweep_propose(probe, [{"item": "i-000000", "folder": "Absent", "mode": "require_existing"}])


def sweep_propose_name_taken(probe):
    return sweep_propose(
        probe, [{"item": "i-000000", "folder": "Reading", "mode": "require_existing"}],
        setup=lambda tree: (tree / "Reading" / "Screenshot 2025-01-01 at 1.00.00 PM.png").write_bytes(b"x"))


def sweep_propose_other_plan(probe):
    return sweep_propose(
        probe, [{"item": "i-000000", "folder": "Reading", "mode": "require_existing"}],
        plan_digest="0" * 64)


def sweep_destination_exists(probe):
    tree = probe.directory / "tree"
    for folder in ("a", "b"):
        (tree / folder).mkdir(parents=True)
        (tree / folder / "Screenshot 2026-07-01 at 9.00.00 AM.png").write_bytes(folder.encode())
    for day in (2, 3):
        (tree / f"Screenshot 2026-07-0{day} at 9.00.00 AM.png").write_bytes(b"synthetic payload")
    return probe.run("apply", tree, "--since", "0", "--depth", "2", "--yes", "--json", codes=(2,))


def sweep_profile_missing(probe):
    (probe.state / "sweep-existing-profile.toml").write_text(
        'schema_version = 1\n[[routes]]\nextension = "xyz"\nfolder = "Folder That Is Absent"\n')
    tree = screenshots(probe.directory / "tree")
    return probe.run(tree, "--scheme", "existing", "--since", "0", "--json", codes=(2,))


def sweep_download_in_flight(probe):
    return probe.run(screenshots(probe.directory / "film.mp4.download"), "--json", codes=(2,))


def sweep_unsafe_input(probe):
    tree = probe.directory / "tree"
    tree.mkdir()
    for name in ("one.xyz", "two.xyz"):
        (tree / name).write_bytes(b"synthetic payload")
    before = snapshot(tree)
    result = probe.run("apply", tree, "--map", "xyz=Routed", "--no-journal", "--yes",
                       "--since", "0", "--json", codes=(2,))
    require(snapshot(tree) == before, "a refused mapped apply still moved files")
    return result


def sweep_permission_denied(probe):
    needs_denied_permissions()
    locked = probe.directory / "read-only"
    locked.mkdir()
    locked.chmod(0o500)
    try:
        return probe.run(screenshots(probe.directory / "tree"), "--since", "0",
                         "--export-plan", locked / "plan.json", "--json", codes=(2,))
    finally:
        locked.chmod(0o700)


def stash_not_due_yet(probe):
    tree = screenshots(probe.directory / "tree")
    probe.run(tree, "--for", "1d", "--json")
    held = snapshot(tree)
    result = probe.run("pop", tree, "--if-due", "--json", codes=(2,))
    require(snapshot(tree) == held, "an early pop moved files")
    probe.run("pop", tree, "--json")
    return result


def stash_unsafe_input(probe):
    tree = screenshots(probe.directory / "tree")
    (tree / "link").symlink_to(tree / "Screenshot 2025-01-01 at 1.00.00 PM.png")
    return probe.run("select", tree / "link", "--json", codes=(2,))


def unpack_archive(probe):
    return zip_archive(probe.directory / "payload.zip", [("payload", os.urandom(200_000))])


def unpack_unsupported_format(probe):
    archive = probe.directory / "image.dmg"
    archive.write_bytes(b"not a supported format")
    return probe.run(archive, "--json", codes=(2,))


def unpack_insufficient_space(probe):
    result = probe.run(unpack_archive(probe), "--max-size", "10", "--json", codes=(2,))
    require(not (probe.directory / "payload").exists(), "a refused extraction published a destination")
    return result


def unpack_destination_exists(probe):
    archive = unpack_archive(probe)
    (probe.directory / "payload").mkdir()
    return probe.run(archive, "--json", codes=(2,))


def unpack_unsafe_input(probe):
    archive = zip_archive(probe.directory / "escape.zip", [("../escape", b"synthetic payload")])
    result = probe.run(archive, "--json", codes=(2,))
    require(not (probe.directory.parent / "escape").exists(), "an unsafe member was extracted")
    return result


def unpack_permission_denied(probe):
    needs_denied_permissions()
    locked = probe.directory / "locked"
    locked.mkdir()
    archive = zip_archive(locked / "payload.zip", [("payload", b"synthetic payload")])
    locked.chmod(0)
    try:
        return probe.run(archive, "--json", codes=(3,))
    finally:
        locked.chmod(0o700)


def unpack_operation_incomplete(probe):
    """Stop a real extraction with SIGTERM once its private staging tree exists.
    An extraction that finishes first is a missed window, not a failure."""
    archive = zip_archive(probe.directory / "large.zip",
                          [(f"member-{number}", bytes(64 * 1024 * 1024)) for number in range(4)])
    process = subprocess.Popen(
        [probe.binary, str(archive), "--json"], cwd=probe.directory, env=probe.environment,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    deadline = time.monotonic() + 30
    try:
        while process.poll() is None and time.monotonic() < deadline:
            if any(probe.directory.glob(".large.unpack-*.partial")):
                process.send_signal(signal.SIGTERM)
                break
            time.sleep(0.002)
        stdout, stderr = process.communicate(timeout=30)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()
    require(probe.environment["ETUDE_JOURNAL_KEY"].encode() not in stdout + stderr,
            "a supplied key was disclosed")
    if process.returncode == 0:
        raise Unreached("the extraction finished before the signal arrived")
    require(process.returncode == 2, f"unpack: stopped extraction exited {process.returncode}, expected 2")
    probe.envelope(json.loads(stdout))
    require(not (probe.directory / "large").exists() and
            not any(probe.directory.glob(".large.unpack-*.partial")),
            "a stopped extraction left a destination or staging tree behind")
    return subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)


# Real invocations for each refusal code, each on its own throwaway fixture. A
# probe returns the tool's result; the caller compares its reason with the
# contract. A code with several probes is proven only if none disagrees. Nothing here reads a tool's source or trusts a self-description.
CODE_PROBES = {
    "sweep": {
        "invalid_arguments": lambda p: p.run("apply", screenshots(p.directory / "tree"), "--json", codes=(2,)),
        "unknown_option": lambda p: p.run("--not-an-option", "--json", codes=(2,)),
        "missing_input": lambda p: p.run(p.directory / "absent", "--json", codes=(3,)),
        "project_folder": lambda p: p.run(project(p.directory / "tree"), "--json", codes=(2,)),
        "destination_exists": (sweep_destination_exists, sweep_propose_name_taken),
        "destination_outside_root": sweep_propose_outside_root,
        "plan_stale": (stale_plan, sweep_propose_other_plan),
        "profile_missing": (sweep_profile_missing, sweep_propose_folder_absent),
        "download_in_flight": sweep_download_in_flight,
        "journal_unreadable": garbage_journal,
        "unknown_operation": lambda p: p.run("status", "--id", "absent", "--json", codes=(1,)),
        "unknown_item": sweep_unknown_item,
        "item_assigned_twice": sweep_item_assigned_twice,
        "unsafe_input": sweep_unsafe_input,
        "permission_denied": (sweep_permission_denied, sweep_unreadable_scan),
        "operation_incomplete": (restore_into_read_only_folder, restore_onto_occupied_path),
    },
    "stash": {
        "invalid_arguments": lambda p: p.run(screenshots(p.directory / "tree"), "--for", "soon", "--json", codes=(2,)),
        "unknown_option": lambda p: p.run("--not-an-option", "--json", codes=(2,)),
        "missing_input": lambda p: p.run(p.directory / "absent", "--json", codes=(3,)),
        "project_folder": lambda p: p.run(project(p.directory / "tree"), "--json", codes=(2,)),
        "plan_stale": stale_plan,
        "not_due_yet": stash_not_due_yet,
        "journal_unreadable": garbage_journal,
        "unknown_operation": tuple(
            (lambda p, command=command: p.run(command, "--id", "absent", "--json", codes=(1,)))
            for command in ("status", "pop", "inspect")),
        "unsafe_input": stash_unsafe_input,
        # A machine-wide path listing answers only to a terminal; stdin is closed.
        "permission_denied": lambda p: p.run("status", "--all", "--paths", "--json", codes=(2,)),
        "operation_incomplete": (restore_into_read_only_folder, restore_onto_occupied_path),
    },
    "unpack": {
        "invalid_arguments": lambda p: p.run("--list", "--json", codes=(2,)),
        "unknown_option": lambda p: p.run("--not-an-option", "--json", codes=(2,)),
        "missing_input": lambda p: p.run(p.directory / "absent.zip", "--json", codes=(3,)),
        "unsupported_format": unpack_unsupported_format,
        "insufficient_space": unpack_insufficient_space,
        "destination_exists": unpack_destination_exists,
        "unsafe_input": unpack_unsafe_input,
        "permission_denied": unpack_permission_denied,
        "operation_incomplete": unpack_operation_incomplete,
    },
}

# Listed codes this witness cannot make a tool return, and why. Each needs an
# environment these probes do not have; the tool has a code path for it. They
# are reported by name on every run. An entry here is a gap, never a pass.
NO_PROBE = {
    "internal_error": "needs a panic or a failed terminal read; no input reaches it on purpose",
    "journal_key_unavailable": "needs the login keychain; every probe supplies a throwaway key",
    "destination_outside_root": "needs a plan whose destination leaves its root; argument and digest checks refuse one first",
    "destination_exists": "needs a destination to appear inside {tool}'s private holding folder mid-move",
}


def refusal_codes(tool, binary, listed, directory):
    """Make the tool return each code its contract lists, and compare.

    Proven: a real invocation returned that code with the contract's retry
    class and, where the class needs one, the contract's subject kind.
    Unproven: no probe reached it. Mismatched: a probe ran and the tool
    answered with something other than what its contract promises.
    """
    names = [row["code"] for row in listed]
    require(len(set(names)) == len(names), "contract lists a refusal code twice")
    require(set(CODE_PROBES[tool]) <= set(names),
            f"{tool}: a probe exists for a code the contract no longer lists: "
            f"{sorted(set(CODE_PROBES[tool]) - set(names))}")
    proven, unproven, mismatched = [], [], []
    for row in listed:
        code = row["code"]
        attempt = CODE_PROBES[tool].get(code)
        if attempt is None:
            why = NO_PROBE.get(code, "no probe is written for it").format(tool=tool)
            unproven.append(f"{code} ({why})")
            continue
        attempts = attempt if isinstance(attempt, tuple) else (attempt,)
        reached, missed, wrong = 0, [], []
        for number, one in enumerate(attempts):
            fixture = directory / f"{code}-{number}"
            fixture.mkdir()
            try:
                result = one(Probe(tool, binary, fixture))
                observed = json.loads(result.stdout)["reason"]
            except Unreached as unreached:
                missed.append(str(unreached))
                continue
            except AssertionError as failure:
                wrong.append(f"{code}: {failure}")
                continue
            expected = {"code": code, "retry": row["retry"]}
            actual = {"code": observed["code"], "retry": observed["retry"]}
            if row["subject_kind"] is not None:
                expected["subject.kind"] = row["subject_kind"]
                actual["subject.kind"] = observed.get("subject", {}).get("kind")
            if actual == expected:
                reached += 1
            else:
                wrong.append(f"{code}: contract promises {expected}, the tool returned {actual}")
        if wrong:
            mismatched.extend(wrong)
        elif reached:
            proven.append(code)
        else:
            unproven.append(f"{code} ({'; '.join(missed)})")
    print(f"refusal codes {tool}: {len(proven)} proven, {len(unproven)} unproven, "
          f"{len(mismatched)} mismatched, of {len(listed)} listed")
    print(f"  proven: {', '.join(proven) or 'none'}")
    for line in unproven:
        print(f"  unproven: {line}")
    for line in mismatched:
        print(f"  mismatched: {line}")
    require(not mismatched,
            f"{tool}: refusal codes disagree with the contract: " + "; ".join(mismatched))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tool", choices=("sweep", "stash", "unpack"))
    parser.add_argument("--bin", type=Path)
    parser.add_argument("--negative-controls", action="store_true")
    arguments = parser.parse_args()
    if arguments.tool is None:
        require(arguments.bin is None, "--bin requires --tool")
        for tool in ("sweep", "stash", "unpack"):
            result = subprocess.run(["python3", __file__, "--tool", tool, "--bin", str(ROOT / "target/debug" / tool)], stdin=subprocess.DEVNULL, check=False)
            require(result.returncode == 0, f"{tool} contract witness failed")
        return
    require(arguments.bin is not None, "--tool requires --bin")
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
            if arguments.tool == "sweep" and sys.platform == "darwin":
                probe.tags()
    with tempfile.TemporaryDirectory(prefix="etudes-contract-codes-") as directory:
        refusal_codes(arguments.tool, arguments.bin, probe.contract["refusal_codes"],
                      Path(directory).resolve())
    print(f"ok {arguments.tool}: contract pin, scope, formats, overwrite, custody and state probes")
    print("unproven: absence of file reads, subprocess networking, and unexercised paths; "
          "no zero counter is access evidence")


if __name__ == "__main__":
    main()
