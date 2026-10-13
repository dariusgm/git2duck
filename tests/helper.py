import os
import sys
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def find_extension(debug: bool = False) -> str:
    build = "debug" if debug else "release"
    base = ROOT / "target" / build
    if os.name == "nt":
        so_name = "git2duck.dll"
    elif hasattr(os, "uname") and os.uname().sysname == "Darwin":  # type: ignore[attr-defined]
        so_name = "libgit2duck.dylib"
    else:
        so_name = "libgit2duck.so"
    so_path = base / so_name
    ext_path = base / "git2duck.duckdb_extension"
    needs_pack = not ext_path.exists()
    if not needs_pack and so_path.exists():
        try:
            needs_pack = so_path.stat().st_mtime > ext_path.stat().st_mtime
        except Exception:
            needs_pack = True
    if needs_pack and so_path.exists():
        packager = Path(os.environ.get("GIT2DUCK_PACKAGER", str(ROOT / "extension-ci-tools" / "scripts" / "append_extension_metadata.py")))
        if packager.exists():
            import duckdb

            # The metadata must match the DuckDB build that loads the extension.
            probe = duckdb.connect(":memory:")
            duckdb_version = probe.execute("SELECT version()").fetchone()[0]
            duckdb_platform = probe.execute("PRAGMA platform").fetchone()[0]
            probe.close()
            cmd = [
                sys.executable,
                str(packager),
                "--library-file",
                str(so_path),
                "--extension-name",
                "git2duck",
                "--out-file",
                str(ext_path),
                "--duckdb-platform",
                duckdb_platform,
                "--duckdb-version",
                duckdb_version,
                "--extension-version",
                "v0.1.0",
                "--abi-type",
                "C_STRUCT",
            ]
            try:
                subprocess.run(cmd, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception:
                pass
    return str(ext_path if ext_path.exists() else so_path)