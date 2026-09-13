"""Generating and loading the Python bindings for the CarrierSettings protobufs.

Schemas come from AOSP: platform/tools/carrier_settings/proto. They are compiled on the fly
with grpcio-tools so there is no generated code in the tree and no system protoc needed.
"""

import pathlib
import sys


def _project_root() -> pathlib.Path:
    for parent in pathlib.Path(__file__).resolve().parents:
        if (parent / "pyproject.toml").exists():
            return parent
    raise RuntimeError("project root (pyproject.toml) not found")


ROOT = _project_root()
PROTO_DIR = ROOT / "proto"
GEN_DIR = ROOT / ".generated"


def _is_stale(proto: pathlib.Path) -> bool:
    generated = GEN_DIR / f"{proto.stem}_pb2.py"
    return not generated.exists() or generated.stat().st_mtime < proto.stat().st_mtime


def ensure_generated() -> None:
    protos = sorted(PROTO_DIR.glob("*.proto"))
    if not protos:
        raise RuntimeError(f"no .proto files in {PROTO_DIR}")

    if any(_is_stale(p) for p in protos):
        import grpc_tools
        from grpc_tools import protoc

        GEN_DIR.mkdir(exist_ok=True)
        well_known = pathlib.Path(grpc_tools.__file__).parent / "_proto"
        rc = protoc.main([
            "protoc",
            f"-I{PROTO_DIR}",
            f"-I{well_known}",
            f"--python_out={GEN_DIR}",
            *[str(p) for p in protos],
        ])
        if rc != 0:
            raise SystemExit(f"protoc exited with {rc}")

    if str(GEN_DIR) not in sys.path:
        sys.path.insert(0, str(GEN_DIR))


def load():
    """Return the carrier_settings_pb2 module."""
    ensure_generated()
    import carrier_settings_pb2

    return carrier_settings_pb2
