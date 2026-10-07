import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
CHECKER = REPO / "scripts" / "check-public-refs"
TICK = chr(96)
BACKSLASH = chr(92)


class CheckPublicRefsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(
            dir=os.environ.get("TMPDIR"), prefix="htl-public-refs-"
        )
        self.root = Path(self.temp.name)
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        (self.root / "scripts").mkdir()
        shutil.copy2(CHECKER, self.root / "scripts" / "check-public-refs")
        (self.root / "justfile").write_text("pre-push:\n    @true\n", encoding="utf-8")
        (self.root / ".gitignore").write_text(
            "/target\n.claude\nworkspace\n.mlua-pkgs/\n.worktrees/\n.htl/\n"
            "*.hb\n.private-generated/\n",
            encoding="utf-8",
        )
        (self.root / "README.md").write_text("fixture\n", encoding="utf-8")
        subprocess.run(
            ["git", "-C", str(self.root), "add", ".gitignore", "justfile", "README.md", "scripts"],
            check=True,
        )

    def tearDown(self):
        self.temp.cleanup()

    def run_checker(self):
        return subprocess.run(
            [sys.executable, str(self.root / "scripts" / "check-public-refs")],
            cwd=self.root,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_reports_each_private_machine_and_missing_tool_reference(self):
        workspace_path = "workspace" + "/drafts/x.md"
        claude_path = ".claude" + "/settings.local.json"
        script_ref = TICK + "scripts/" + "nope" + TICK
        shell_ref = TICK + "deploy" + ".sh" + TICK
        just_ref = TICK + "just " + "nope" + TICK
        unix_user = "/" + "Users/someone/src"
        home_user = "/" + "home/alice/y"
        windows_user = "C:" + BACKSLASH + "Users" + BACKSLASH + "someone" + BACKSLASH + "x"
        refs = [
            workspace_path,
            claude_path,
            script_ref,
            shell_ref,
            just_ref,
            unix_user,
            windows_user,
            home_user,
        ]
        (self.root / "references.md").write_text("\n".join(refs) + "\n", encoding="utf-8")

        result = self.run_checker()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        findings = result.stdout.splitlines()
        self.assertEqual(
            findings,
            [
                "references.md:1: private-path",
                "references.md:2: private-path",
                "references.md:3: missing-tool: scripts/nope",
                "references.md:4: missing-tool: deploy.sh",
                "references.md:5: missing-tool: just nope",
                "references.md:6: machine-path",
                "references.md:7: machine-path",
                "references.md:8: machine-path",
            ],
            result.stdout,
        )

    def test_allows_directory_names_generated_outputs_and_existing_tools(self):
        just_ref = TICK + "just pre-push" + TICK
        script_ref = TICK + "scripts/check-public-refs" + TICK
        linuxbrew = "/" + "home/linuxbrew/.linuxbrew/bin"
        unix_placeholder = "/" + "Users/" + "…"
        home_placeholder = "/" + "home/" + "…"
        windows_placeholder = "C:" + BACKSLASH + "Users" + BACKSLASH + "…"
        content = "\n".join(
            [
                "workspace/",
                ".worktrees/",
                "target/debug/htl",
                ".htl/cache/",
                ".mlua-pkgs/cache/file",
                just_ref,
                script_ref,
                linuxbrew,
                unix_placeholder,
                home_placeholder,
                windows_placeholder,
            ]
        )
        (self.root / "allowed.md").write_text(content + "\n", encoding="utf-8")

        result = self.run_checker()

        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_scans_untracked_public_files_skips_ignored_files_and_uses_new_ignores(self):
        ticked = TICK + "just nope" + TICK
        (self.root / "untracked.md").write_text(ticked + "\n", encoding="utf-8")
        private = self.root / "workspace" / "private.md"
        private.parent.mkdir()
        unix_private = "/" + "Users/someone" + "/private\n"
        private.write_text(unix_private, encoding="utf-8")
        new_private = ".private-generated" + "/secret.txt"
        (self.root / "new-ignore.md").write_text(new_private + "\n", encoding="utf-8")
        (self.root / ".gitignore").write_text(
            (self.root / ".gitignore").read_text(encoding="utf-8")
            + "\n" + new_private.split("/", 1)[0] + "/\n",
            encoding="utf-8",
        )
        private_rel = "workspace" + "/private.md"
        subprocess.run(["git", "-C", str(self.root), "add", "-f", private_rel], check=True)

        result = self.run_checker()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(len(result.stdout.splitlines()), 2, result.stdout)
        self.assertIn("untracked.md:1: missing-tool", result.stdout)
        self.assertIn("new-ignore.md:1: private-path", result.stdout)

    def test_scans_symlink_target_without_following_it(self):
        target = "/" + "Users/someone/private"
        (self.root / "public-link").symlink_to(target)

        result = self.run_checker()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["public-link:1: machine-path"])


if __name__ == "__main__":
    unittest.main()
