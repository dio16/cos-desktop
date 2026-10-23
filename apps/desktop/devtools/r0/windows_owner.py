"""Safe CPython/Windows development ownership boundary (no ctypes or own FFI).

_winapi.CreateFile is CPython's compiled standard-library wrapper. Its returned
non-inheritable handle transfers exactly once to an msvcrt descriptor and then a
Python FileIO owner. Every ancestor is pinned before resolving the next component;
write/delete sharing is denied, reparse objects rejected through fstat. CPython's
Windows fstat supplies volume/file identity, checked again before publication.
Unsupported interpreters/filesystems fail closed, not through another launcher.
"""
from __future__ import annotations

import hashlib
import ntpath
import os
import stat
from pathlib import Path


def known_folder(name: str) -> Path:
    """Current-user Shell paths, never optional environment variables.

    GUID storage and the returned PWSTR are owned locally. The Shell allocates
    the PWSTR; CoTaskMemFree releases it once, including failure paths.
    No impersonation, folder creation or security-policy change is requested.
    """
    import ctypes as c
    import uuid

    ids = {
        "profile": "5e6c858f-0e22-4760-9afe-ea3317b67173",
        "local_appdata": "f1b32785-6fba-4fcf-9d55-7b8e7f157091",
    }
    class Guid(c.Structure):
        _fields_ = [("a", c.c_uint32), ("b", c.c_uint16),
                    ("c", c.c_uint16), ("d", c.c_ubyte * 8)]

    shell = c.WinDLL("shell32", use_last_error=True)
    ole = c.WinDLL("ole32", use_last_error=True)
    shell.SHGetKnownFolderPath.argtypes = [
        c.POINTER(Guid), c.c_uint32, c.c_void_p, c.POINTER(c.c_void_p),
    ]
    shell.SHGetKnownFolderPath.restype = c.c_int32
    ole.CoTaskMemFree.argtypes = [c.c_void_p]
    ole.CoTaskMemFree.restype = None
    identifier = Guid.from_buffer_copy(uuid.UUID(ids[name]).bytes_le)
    result = c.c_void_p()
    try:
        if shell.SHGetKnownFolderPath(c.byref(identifier), 0, None, c.byref(result)) != 0 or not result.value:
            raise RuntimeError("Windows user folder resolution is unavailable")
        path = Path(c.wstring_at(result))
        local_path(path)
        if not path.is_dir():
            raise RuntimeError("Windows user folder is unavailable")
        return path
    finally:
        if result.value:
            ole.CoTaskMemFree(result)


def local_path(path: Path) -> str:
    text = os.fspath(path).replace("/", "\\")
    drive, tail = ntpath.splitdrive(text)
    if len(drive) != 2 or not drive[0].isascii() or not drive[0].isalpha() or drive[1] != ":" or not tail.startswith("\\"):
        raise RuntimeError("only absolute local disk paths are supported")
    for part in tail.split("\\")[1:]:
        if part in (".", "..") or ":" in part or "\0" in part or part.endswith((".", " ")):
            raise RuntimeError("ambiguous local disk path is refused")
    return drive.upper() + tail


class Pin:
    def __init__(self, path: Path, *, directory: bool = False):
        if os.name != "nt":
            raise RuntimeError("Windows ownership support is required")
        try:
            import _winapi
            import msvcrt
        except ImportError:
            raise RuntimeError("CPython Windows ownership support is required") from None
        self.api, self.crt = _winapi, msvcrt
        self.files = []
        self.path = local_path(path)
        self.directory = directory
        parents = list(Path(self.path).parents)
        parents.reverse()
        try:
            for parent in parents:
                self._open(local_path(parent), True)
            self.file = self._open(self.path, directory)
        except BaseException:
            self.close()
            raise

    def _open(self, path: str, directory: bool):
        flags = 0x00200000 | (0x02000000 if directory else 0)
        handle = self.api.CreateFile(path, 0x80000000, 1, 0, 3, flags, 0)
        try:
            descriptor = self.crt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
        except BaseException:
            self.api.CloseHandle(handle)
            raise
        file = None
        if not directory:
            try:
                file = os.fdopen(descriptor, "rb", buffering=0)
            except BaseException:
                os.close(descriptor)
                raise
        self.files.append((path, file, descriptor))
        info = os.fstat(descriptor if file is None else file.fileno())
        if info.st_file_attributes & 0x400 or stat.S_ISDIR(info.st_mode) != directory or (not directory and not stat.S_ISREG(info.st_mode)) or not info.st_ino:
            raise RuntimeError("reparse, wrong-type or unidentified file is refused")
        current = os.stat(path, follow_symlinks=False)
        if (info.st_dev, info.st_ino) != (current.st_dev, current.st_ino):
            raise RuntimeError("opened object does not match its path")
        return file

    def verify(self):
        for path, file, descriptor in self.files:
            expected = os.fstat(descriptor if file is None else file.fileno())
            current = os.stat(path, follow_symlinks=False)
            if current.st_file_attributes & 0x400 or (expected.st_dev, expected.st_ino) != (current.st_dev, current.st_ino):
                raise RuntimeError("owned path identity changed")

    def digest(self) -> tuple[str, int]:
        if self.directory:
            raise RuntimeError("directory content cannot be hashed")
        if self.file is None:
            raise RuntimeError("regular file ownership is unavailable")
        digest, total = hashlib.sha256(), 0
        for chunk in iter(lambda: self.file.read(64 * 1024), b""):
            digest.update(chunk)
            total += len(chunk)
        return "sha256:" + digest.hexdigest(), total

    def identity(self) -> tuple[int, int]:
        _, file, descriptor = self.files[-1]
        info = os.fstat(descriptor if file is None else file.fileno())
        return info.st_dev, info.st_ino

    def close(self):
        while self.files:
            _, file, descriptor = self.files.pop()
            if file is None:
                os.close(descriptor)
            else:
                file.close()

    def __enter__(self):
        return self

    def __exit__(self, *_unused):
        self.close()



class Job:
    """Kill-on-close containment. ctypes uses only fixed Win32 x64 signatures;
    pointers are borrowed for synchronous calls and every handle has one owner."""
    def __init__(self):
        import ctypes as c
        self.c = c
        self.kernel = c.WinDLL("kernel32", use_last_error=True)
        self.kernel.CreateJobObjectW.restype = c.c_void_p
        self.kernel.CreateJobObjectW.argtypes = [c.c_void_p, c.c_wchar_p]
        self.kernel.SetInformationJobObject.restype = c.c_int
        self.kernel.SetInformationJobObject.argtypes = [c.c_void_p, c.c_int, c.c_void_p, c.c_uint32]
        self.kernel.AssignProcessToJobObject.restype = c.c_int
        self.kernel.AssignProcessToJobObject.argtypes = [c.c_void_p, c.c_void_p]
        self.kernel.QueryInformationJobObject.restype = c.c_int
        self.kernel.QueryInformationJobObject.argtypes = [c.c_void_p, c.c_int, c.c_void_p, c.c_uint32, c.c_void_p]
        self.kernel.CloseHandle.restype = c.c_int
        self.kernel.CloseHandle.argtypes = [c.c_void_p]
        self.handle = self.kernel.CreateJobObjectW(None, None)
        if not self.handle:
            raise RuntimeError("process containment is unavailable")
        class Basic(c.Structure):
            _fields_ = [("process_time", c.c_int64), ("job_time", c.c_int64), ("flags", c.c_uint32),
                        ("minimum_working_set", c.c_size_t), ("maximum_working_set", c.c_size_t),
                        ("active_limit", c.c_uint32), ("affinity", c.c_size_t),
                        ("priority", c.c_uint32), ("scheduling", c.c_uint32)]
        class Extended(c.Structure):
            _fields_ = [("basic", Basic), ("io", c.c_uint64 * 6), ("process_memory", c.c_size_t),
                        ("job_memory", c.c_size_t), ("peak_process", c.c_size_t), ("peak_job", c.c_size_t)]
        self.limits = Extended()
        self.limits.basic.flags = 0x2000  # KILL_ON_JOB_CLOSE; no breakaway/silent escape.
        if not self.kernel.SetInformationJobObject(self.handle, 9, c.byref(self.limits), c.sizeof(self.limits)):
            self.close()
            raise RuntimeError("process containment could not be configured")

    def assign(self, process):
        handle = getattr(process, "_handle", None)
        if handle is None or not self.kernel.AssignProcessToJobObject(self.handle, int(handle)):
            raise RuntimeError("helper could not enter the owned process job")

    def empty(self) -> bool:
        c = self.c
        class Accounting(c.Structure):
            _fields_ = [("times", c.c_uint64 * 4), ("faults", c.c_uint32), ("total", c.c_uint32),
                        ("active", c.c_uint32), ("terminated", c.c_uint32)]
        info = Accounting()
        if not self.kernel.QueryInformationJobObject(self.handle, 1, c.byref(info), c.sizeof(info), None):
            raise RuntimeError("process cleanup could not be confirmed")
        return info.active == 0

    def close(self):
        handle, self.handle = self.handle, None
        if handle and not self.kernel.CloseHandle(handle):
            raise RuntimeError("owned process job release failed")

    def __enter__(self):
        return self

    def __exit__(self, *_unused):
        self.close()
