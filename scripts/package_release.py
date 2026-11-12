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
        }, stream)
    # Ad-hoc signing makes the bundle structurally valid on Apple Silicon.
    # This is NOT Developer ID signing/notarization and does not bypass Gatekeeper.
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(bundle)], check=True)
else:
    shutil.copy2(binary, staging / binary.name)
archive = out / f"LiteText-{args.os}-{args.arch}.zip"
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
    for path in sorted(staging.rglob("*")):
        if path.is_file():
            zipped.write(path, path.relative_to(staging))
if args.os == "macos":
    os.symlink("/Applications", staging / "Applications")
    subprocess.run(["hdiutil", "create", "-volname", "LiteText", "-srcfolder", str(staging),
                    "-format", "UDZO", str(out / f"LiteText-macos-{args.arch}.dmg")], check=True)
