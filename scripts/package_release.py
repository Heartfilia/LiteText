#!/usr/bin/env python3
"""Package only our CI binary/resources; no recursive archive extraction."""
import argparse
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import zipfile
from macos_documents import document_types

parser = argparse.ArgumentParser()
parser.add_argument("--os", choices=["macos", "linux", "windows"], required=True)
parser.add_argument("--arch", choices=["arm64", "x64"], required=True)
parser.add_argument("--target", required=True)
args = parser.parse_args()
tag = os.environ.get("LITETEXT_VERSION", "")
if not re.fullmatch(r"v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", tag):
    raise SystemExit("LITETEXT_VERSION must be a semver release tag")
out = pathlib.Path("dist")
out.mkdir(exist_ok=True)
staging = out / f"stage-{args.os}-{args.arch}"
staging.mkdir(exist_ok=False)
binary = pathlib.Path("target") / args.target / "release" / ("litetext.exe" if args.os == "windows" else "litetext")
if args.os == "macos":
    bundle = staging / "LiteText.app"
    contents = bundle / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources").mkdir()
    shutil.copy2(binary, contents / "MacOS/litetext")
    shutil.copy2("assets/app-icon.icns", contents / "Resources/app-icon.icns")
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump({
            "CFBundleName": "LiteText", "CFBundleDisplayName": "LiteText",
            "CFBundleIdentifier": "com.heartfilia.litetext", "CFBundleExecutable": "litetext",
            "CFBundlePackageType": "APPL", "CFBundleIconFile": "app-icon.icns",
            "CFBundleShortVersionString": tag.removeprefix("v").split("-")[0].split("+")[0],
            "CFBundleVersion": tag.removeprefix("v").split("-")[0].split("+")[0],
            "LSMinimumSystemVersion": "11.0", "NSHighResolutionCapable": True,
            **document_types(),
        }, stream)
    # Ad-hoc signing makes the bundle structurally valid on Apple Silicon.
    # This is NOT Developer ID signing/notarization and does not bypass Gatekeeper.
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(bundle)], check=True)
elif args.os == "windows":
    shutil.copy2(binary, out / f"LiteText-windows-{args.arch}.exe")
else:
    shutil.copy2(binary, staging / binary.name)
    archive = out / f"LiteText-{args.os}-{args.arch}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
        for path in sorted(staging.rglob("*")):
            if path.is_file():
                zipped.write(path, path.relative_to(staging))
if args.os == "macos":
    (staging / "安装后打开失败查看.txt").write_text(
        "LiteText 首次安装打开说明\n\n"
        "请先将 LiteText.app 拖入 Applications（应用程序）文件夹，再从应用程序中打开。\n\n"
        "首次安装可能会因 macOS 安全检查而打不开（如提示无法验证开发者或应用已损坏）。\n"
        "仅在确认安装包来自本项目可信 Release、并核对 SHA256SUMS 后，"
        "请使用下面的命令在终端执行一次：\n\n"
        "xattr -dr com.apple.quarantine /Applications/LiteText.app\n\n"
        "执行后重新打开 LiteText。后续可在 App 内部检测更新，下载后点击“安装并重启”即可。\n\n"
        "注意：该命令会递归移除 LiteText 的下载隔离标记，削弱 macOS 对该应用的首次打开检查；"
        "它不等于开发者签名或公证，也不能修复真正损坏的文件。"
        "不要对未知应用、整个 Applications 文件夹或系统范围执行。\n"
        "如安装在其他位置，请将命令中的路径替换为实际路径；权限不足时不要盲目添加 sudo。\n",
        encoding="utf-8",
    )
    os.symlink("/Applications", staging / "Applications")
    subprocess.run(["hdiutil", "create", "-volname", "LiteText", "-srcfolder", str(staging),
                    "-format", "UDZO", str(out / f"LiteText-macos-{args.arch}.dmg")], check=True)
