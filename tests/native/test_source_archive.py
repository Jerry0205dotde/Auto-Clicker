"""Source archive of the release build reflects the current work tree."""

from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "source-archive.sh"


def git(repo, *args):
    subprocess.run(
        ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", *args],
        cwd=repo,
        check=True,
        capture_output=True,
    )


class SourceArchiveTests(unittest.TestCase):
    def test_archive_contains_edits_and_new_files_without_deleted_or_ignored(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp) / "repo"
            repo.mkdir()
            git(repo, "init", "-q")
            for name, text in {
                ".gitignore": "ignored.txt\n",
                "kept.txt": "old\n",
                "deleted.txt": "gone\n",
                "staged-deleted.txt": "gone\n",
            }.items():
                (repo / name).write_text(text)
            git(repo, "add", ".")
            git(repo, "commit", "-q", "-m", "init")

            (repo / "kept.txt").write_text("edited\n")
            (repo / "new.txt").write_text("new\n")
            (repo / "ignored.txt").write_text("ignored\n")
            (repo / "deleted.txt").unlink()
            git(repo, "rm", "-q", "staged-deleted.txt")

            archive = Path(temp) / "source.tar.gz"
            result = subprocess.run(
                ["bash", SCRIPT, archive, "prefix"],
                cwd=repo,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            with tarfile.open(archive) as tar:
                self.assertEqual(
                    sorted(tar.getnames()),
                    ["prefix/.gitignore", "prefix/kept.txt", "prefix/new.txt"],
                )
                self.assertEqual(tar.extractfile("prefix/kept.txt").read(), b"edited\n")

    def test_relative_symlink_keeps_its_target(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp) / "repo"
            repo.mkdir()
            git(repo, "init", "-q")
            (repo / "kept.txt").write_text("kept\n")
            git(repo, "add", ".")
            git(repo, "commit", "-q", "-m", "init")
            (repo / "link.txt").symlink_to("kept.txt")

            archive = Path(temp) / "source.tar.gz"
            result = subprocess.run(
                ["bash", SCRIPT, archive, "prefix"],
                cwd=repo,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            with tarfile.open(archive) as tar:
                link = tar.getmember("prefix/link.txt")
                self.assertTrue(link.issym())
                # Resolved next to the link, this is prefix/kept.txt.
                self.assertEqual(link.linkname, "kept.txt")


if __name__ == "__main__":
    unittest.main()
