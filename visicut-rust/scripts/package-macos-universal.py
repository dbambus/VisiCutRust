#!/usr/bin/env python3
"""Combine the tested Intel and Apple Silicon app archives into a Universal app."""
import argparse
import plistlib
import subprocess
import tempfile
from pathlib import Path


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("intel_archive", type=Path)
    parser.add_argument("arm64_archive", type=Path)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent.parent / "dist")
    args = parser.parse_args()
    names = []
    for archive, target in [(args.intel_archive, "x86_64"), (args.arm64_archive, "aarch64")]:
        suffix = f"-{target}-apple-darwin.zip"
        if not archive.is_file() or not archive.name.startswith("VisiCutRust-") or not archive.name.endswith(suffix):
            parser.error(f"Expected a VisiCutRust {target} macOS archive: {archive}")
        names.append(archive.name.removesuffix(suffix))
    if names[0] != names[1]:
        parser.error("Both archives must have the same VisiCutRust version")
    version = names[0].removeprefix("VisiCutRust-")
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="visicutrust-universal-") as temporary:
        work = Path(temporary)
        run("ditto", "-x", "-k", args.intel_archive.resolve(), work / "intel")
        run("ditto", "-x", "-k", args.arm64_archive.resolve(), work / "arm64")
        intel = work / "intel/VisiCutRust.app/Contents"
        arm64 = work / "arm64/VisiCutRust.app/Contents"
        info = plistlib.loads((arm64 / "Info.plist").read_bytes())
        if info != plistlib.loads((intel / "Info.plist").read_bytes()):
            parser.error("The app metadata differs between architectures")
        if (info["CFBundleName"] != "VisiCutRust"
                or info["CFBundleExecutable"] != "visicut-rust"
                or info["CFBundleShortVersionString"] != version.split("-")[0]):
            parser.error("The app metadata does not match the release archive")
        resources = lambda root: {
            path.relative_to(root): path.read_bytes()
            for path in root.rglob("*") if path.is_file()
        }
        if resources(intel / "Resources") != resources(arm64 / "Resources"):
            parser.error("The app resources differ between architectures")
        binary = "MacOS/visicut-rust"
        run("lipo", "-verify_arch", "x86_64", intel / binary)
        run("lipo", "-verify_arch", "arm64", arm64 / binary)
        run("lipo", "-create", intel / binary, arm64 / binary, "-output", work / "visicut-rust")
        (work / "visicut-rust").replace(arm64 / binary)
        (arm64 / binary).chmod(0o755)
        run("lipo", "-verify_arch", "x86_64", "arm64", arm64 / binary)
        run("codesign", "--force", "--sign", "-", "--timestamp=none", arm64.parent)
        run("codesign", "--verify", "--strict", "--all-architectures", arm64.parent)
        archive = args.output.resolve() / f"{names[0]}-universal-apple-darwin.zip"
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", arm64.parent, archive)
        print(archive)


if __name__ == "__main__":
    main()
