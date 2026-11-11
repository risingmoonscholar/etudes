"""The test-key check rejects a tool run that could reach the login keychain."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "check_test_keys", Path(__file__).with_name("check-test-keys.py"))
check_test_keys = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_test_keys)

HELPER = """
const F: &str = r#"(version 1)(allow default)(deny process-exec (literal "/usr/bin/security"))"#;
pub fn with_test_key(c: &mut Command) -> &mut Command {
    c.env("ETUDE_JOURNAL_KEY", "k").env("ETUDE_STATE_DIR", "s").env("HOME", "h")
}
"""

MANIFEST = '[package]\nname = "sweep-cli"\n\n[[bin]]\nname = "sweep"\npath = "src/main.rs"\n'


class CheckTestKeysTests(unittest.TestCase):
    def problems(self, test_source, helper=HELPER, extra=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            support = root / "crates/etude-cli-support/test-support"
            support.mkdir(parents=True)
            if helper is not None:
                (support / "test_tool.rs").write_text(helper)
            tests = root / "crates/sweep-cli/tests"
            tests.mkdir(parents=True)
            (root / "crates/sweep-cli/Cargo.toml").write_text(MANIFEST)
            (tests / "case.rs").write_text(test_source)
            for name, text in (extra or {}).items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text(text)
            return check_test_keys.check(root)

    def test_a_spawn_without_the_helper_fails(self):
        problems = self.problems("""
fn sweep() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
}
""")
        self.assertEqual(1, len(problems), problems)
        self.assertIn("case.rs:3", problems[0])
        self.assertIn("no test key", problems[0])

    def test_a_spawn_through_the_helper_passes(self):
        self.assertEqual([], self.problems("""
fn sweep() -> Command {
    keyed_tool(env!("CARGO_BIN_EXE_sweep"))
}
"""))

    def test_a_raw_spawn_fails_even_when_the_function_also_uses_the_helper(self):
        problems = self.problems("""
fn two() {
    let _ = keyed_tool(env!("CARGO_BIN_EXE_sweep")).output();
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_sweep")).output();
}
""")
        self.assertEqual(1, len(problems), problems)
        self.assertIn("case.rs:4", problems[0])

    def test_a_wrapper_needs_the_key_too(self):
        unkeyed = """
fn shell() {
    let _ = Command::new("sh").arg("-c").arg(env!("CARGO_BIN_EXE_sweep")).output();
}
"""
        self.assertEqual(1, len(self.problems(unkeyed)))
        keyed = unkeyed.replace('Command::new("sh")', 'with_test_key(&mut Command::new("sh"))')
        self.assertEqual([], self.problems(keyed))

    def test_a_constant_is_followed_to_its_uses(self):
        source = """
const SWEEP: &str = env!("CARGO_BIN_EXE_sweep");
fn run() { let _ = Command::new(SWEEP).output(); }
"""
        problems = self.problems(source)
        self.assertEqual(1, len(problems), problems)
        self.assertIn("Command::new(SWEEP)", problems[0])
        self.assertEqual([], self.problems(source.replace("Command::new(SWEEP)", "keyed_tool(SWEEP)")))

    def test_a_bare_binary_name_is_a_spawn(self):
        problems = self.problems('fn run() { let _ = Command::new("sweep").output(); }\n')
        self.assertEqual(1, len(problems), problems)

    def test_a_fenced_marker_must_actually_fence(self):
        fenced = """
// keychain-fenced: runs with no key on purpose.
#[test]
fn keyless() {
    let _ = fenced_tool(env!("CARGO_BIN_EXE_sweep")).output();
}
"""
        self.assertEqual([], self.problems(fenced))
        fake = """
// keychain-fenced: says so, does not do it.
#[test]
fn keyless() {
    let _ = Command::new(env!("CARGO_BIN_EXE_sweep")).env_remove("ETUDE_JOURNAL_KEY").output();
}
"""
        problems = self.problems(fake)
        self.assertEqual(1, len(problems), problems)
        self.assertIn("never fences", problems[0])

    def test_comments_and_strings_do_not_confuse_it(self):
        self.assertEqual([], self.problems("""
// Command::new(env!("CARGO_BIN_EXE_sweep")) in a comment is not a spawn.
/* nor Command::new(env!("CARGO_BIN_EXE_sweep")) here */
fn braces() -> String { format!("{{ }} {}", "}") }
fn sweep() -> Command { keyed_tool(env!("CARGO_BIN_EXE_sweep")) }
"""))

    def test_a_missing_or_keyless_helper_fails(self):
        source = 'fn sweep() -> Command { keyed_tool(env!("CARGO_BIN_EXE_sweep")) }\n'
        self.assertTrue(any("missing" in p for p in self.problems(source, helper=None)))
        broken = HELPER.replace('"ETUDE_JOURNAL_KEY"', '"SOMETHING_ELSE"')
        self.assertTrue(any("ETUDE_JOURNAL_KEY" in p for p in self.problems(source, helper=broken)))

    def test_shared_files_and_unit_tests_are_checked(self):
        problems = self.problems("", extra={
            "crates/etude-cli-support/test-support/probe.rs":
                'fn p() { let _ = Command::new(env!("CARGO_BIN_EXE_sweep")).output(); }\n',
            "crates/sweep-cli/src/main.rs":
                '#[cfg(test)] mod t { fn p() { let _ = Command::new(env!("CARGO_BIN_EXE_sweep")).output(); } }\n',
        })
        self.assertEqual(2, len(problems), problems)

    def test_the_rule_is_per_spawn_not_per_function(self):
        problems = self.problems("""
fn two() {
    let _ = keyed_tool(env!("CARGO_BIN_EXE_sweep")).output();
    let _ = Command::new("sh").arg(env!("CARGO_BIN_EXE_sweep")).output();
}
""")
        self.assertEqual(1, len(problems), problems)
        self.assertIn("case.rs:4", problems[0])

    def test_a_held_path_is_followed_to_where_it_is_used(self):
        source = """
fn held() {
    let bin = Path::new(env!("CARGO_BIN_EXE_sweep")).parent().unwrap();
    let _ = Command::new("/bin/sh").env("PATH", bin).output();
}
"""
        problems = self.problems(source)
        self.assertEqual(1, len(problems), problems)
        self.assertIn("case.rs:4", problems[0])
        self.assertEqual([], self.problems(source.replace(
            'Command::new("/bin/sh")', 'with_test_key(&mut Command::new("/bin/sh"))')))

    def test_a_keyed_variable_carries_its_key_to_later_statements(self):
        self.assertEqual([], self.problems("""
fn later() {
    let mut command = Command::new("python3");
    with_test_key(&mut command);
    command.args(["x", env!("CARGO_BIN_EXE_sweep")]);
}
"""))

    def test_removing_the_key_or_the_environment_needs_a_fence(self):
        for removal in ('env_remove("ETUDE_JOURNAL_KEY")', "env_clear()"):
            with self.subTest(removal=removal):
                problems = self.problems(f"""
fn keyless() {{
    let _ = keyed_tool(env!("CARGO_BIN_EXE_sweep")).{removal}.output();
}}
""")
                self.assertEqual(1, len(problems), problems)
                self.assertIn("outside a function marked", problems[0])
                self.assertEqual([], self.problems(f"""
// keychain-fenced: no key on purpose.
fn keyless() {{
    let _ = fenced_tool(env!("CARGO_BIN_EXE_sweep")).{removal}.output();
}}
"""))

    def test_a_helper_name_in_a_string_or_comment_is_not_a_call(self):
        problems = self.problems("""
fn pretend() {
    // keyed_tool( is only mentioned here
    let _ = Command::new("sh").args(["-c", "keyed_tool(", env!("CARGO_BIN_EXE_sweep")]).output();
}
""")
        self.assertEqual(1, len(problems), problems)

    def test_sandbox_exec_in_a_string_or_comment_is_not_a_fence(self):
        problems = self.problems("""
// keychain-fenced: claims a fence.
fn keyless() {
    // sandbox-exec would go here
    eprintln!("runs under sandbox-exec");
    let _ = Command::new("sh").output();
}
""")
        self.assertEqual(1, len(problems), problems)
        self.assertIn("never fences", problems[0])
        self.assertEqual([], self.problems("""
// keychain-fenced: fences by hand.
fn keyless() {
    let _ = Command::new("/usr/bin/sandbox-exec").arg("-p").output();
}
"""))

    def test_cargo_run_is_a_spawn(self):
        source = 'fn run() { let _ = Command::new("cargo").args(["run", "-p", "sweep-cli"]).output(); }\n'
        self.assertEqual(1, len(self.problems(source)))
        self.assertEqual([], self.problems(source.replace('Command::new("cargo")',
                                                          'with_test_key(&mut Command::new("cargo"))')))
        self.assertEqual([], self.problems(
            'fn tree() { let _ = Command::new("cargo").args(["tree"]).output(); }\n'))

    def test_a_current_exe_spawn_needs_a_key(self):
        bare = 'fn again() { let _ = Command::new(std::env::current_exe().unwrap()).output(); }\n'
        problems = self.problems(bare)
        self.assertEqual(1, len(problems), problems)
        self.assertIn("current_exe", problems[0])
        keyed = bare.replace(".output()", '.env("ETUDE_JOURNAL_KEY", key).output()')
        self.assertEqual([], self.problems(keyed))
        held = """
fn again() {
    let exe = std::env::current_exe().unwrap();
    let _ = Command::new(&exe).output();
}
"""
        self.assertEqual(1, len(self.problems(held)))
        # Reading the path (to scan it, say) is not a spawn.
        self.assertEqual([], self.problems("""
fn scan() {
    let exe = std::env::current_exe().unwrap();
    let _ = Command::new("nm").arg(&exe).output();
}
"""))

    def test_a_shell_string_naming_a_build_folder_is_a_spawn(self):
        source = 'fn sh() { let _ = Command::new("sh").args(["-c", "target/debug/sweep apply x"]).output(); }\n'
        self.assertEqual(1, len(self.problems(source)))
        self.assertEqual([], self.problems(source.replace('Command::new("sh")',
                                                          'with_test_key(&mut Command::new("sh"))')))

    def test_automatic_binaries_count_as_tools(self):
        extra = {
            "crates/journal-dump/Cargo.toml": '[package]\nname = "journal-dump"\n',
            "crates/journal-dump/src/main.rs": "fn main() {}\n",
            "crates/fixtures/Cargo.toml": '[package]\nname = "fixtures"\n',
            "crates/fixtures/src/bin/mkfx.rs": "fn main() {}\n",
        }
        for program in ("journal-dump", "mkfx", "target/debug/mkfx"):
            with self.subTest(program=program):
                problems = self.problems(
                    f'fn run() {{ let _ = Command::new("{program}").output(); }}\n', extra=extra)
                self.assertTrue(problems, program)

    def test_the_repository_passes(self):
        root = Path(__file__).resolve().parent.parent
        self.assertEqual([], check_test_keys.check(root))


if __name__ == "__main__":
    unittest.main()
