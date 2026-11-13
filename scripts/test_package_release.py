import os
import pathlib
import runpy
import tempfile
import unittest
import zipfile
from unittest.mock import patch


class PackageReleaseTest(unittest.TestCase):
    def test_only_platform_artifact_is_generated(self):
        script = pathlib.Path(__file__).with_name("package_release.py").resolve()
        for platform, arch, extension in [("macos", "arm64", "dmg"), ("windows", "x64", "exe"), ("linux", "x64", "zip")]:
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as directory:
                previous = pathlib.Path.cwd()
                try:
                    os.chdir(directory)
                    binary = pathlib.Path("target/test-target/release") / ("litetext.exe" if platform == "windows" else "litetext")
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"test-binary")
                    pathlib.Path("assets").mkdir()
                    pathlib.Path("assets/app-icon.icns").write_bytes(b"icon")

                    def command(args, **kwargs):
                        if args[0] == "hdiutil":
                            staging = pathlib.Path(args[args.index("-srcfolder") + 1])
                            notice = staging / "安装后打开失败查看.txt"
                            text = notice.read_text(encoding="utf-8")
                            self.assertIn("xattr -dr com.apple.quarantine /Applications/LiteText.app", text)
                            self.assertIn("后续可在 App 内部检测更新", text)
                            self.assertIn("可信 Release", text)
                            self.assertIn("削弱 macOS", text)
                            self.assertEqual(set(p.name for p in staging.iterdir()), {"LiteText.app", notice.name})
                            pathlib.Path(args[-1]).write_bytes(b"dmg")

                    with patch.dict(os.environ, {"LITETEXT_VERSION": "v1.2.3-beta.1"}), patch("sys.argv", [str(script), "--os", platform, "--arch", arch, "--target", "test-target"]), patch("subprocess.run", side_effect=command), patch("os.symlink"):
                        runpy.run_path(str(script), run_name="__main__")
                    artifacts = [p.name for p in pathlib.Path("dist").iterdir() if p.is_file()]
                    expected = f"LiteText-{platform}-{arch}.{extension}"
                    self.assertEqual(artifacts, [expected])
                    if platform == "windows":
                        self.assertEqual(pathlib.Path("dist", expected).read_bytes(), b"test-binary")
                    if platform == "linux":
                        with zipfile.ZipFile(pathlib.Path("dist", expected)) as archive:
                            self.assertEqual(archive.namelist(), ["litetext"])
                finally:
                    os.chdir(previous)


if __name__ == "__main__":
    unittest.main()
