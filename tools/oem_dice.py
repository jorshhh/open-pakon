#!/usr/bin/env python3
"""The OEM dust removal (Applied Science Fiction Digital ICE, oem/DMLDICELib.dll)
under Unicorn: what the export uses for colour negative (web/decode.py), with
tools/pakon_ice.py as the fallback when Unicorn is missing.

TLB.dll calls it on the 16-bit planar picture before the density LUT
(docs/TLB_FINDINGS.md "Digital ICE"): DMLDICEBegin(0x65, static {0x20, 2, 1,
14, 2, 0, 0, 0}), then per picture DMLDICEProcess(handle, dynamic {0x30,
planes, d0, d1, 300, negative, 0, 0, 0, d0 - 1, d1 - 1, 0}). The planes are
R, G, B, IR back to back, 14-bit (14 sets a 16384-entry log table the pixels
index directly), so the frame goes in as (raw - 300) / 4, like oem_rpd.

The DLL links the MSVC runtime statically and imports only KERNEL32: every
import is a stdcall stub here, the runtime's DllMain runs first, fs:[0]
points at a thread block, and its worker threads run on the spot when
WaitForSingleObject waits for them (the stub jumps into the thread function
and comes back to the wait when it returns, whatever its calling
convention).

DICE works on 14-bit values; only the pixels it changes are taken back into
the 16-bit frame, so the rest keep their full precision. One Dice per thread
(an emulator is ~1.5 GB for the first, ~0.4 GB for each more); frames run in
parallel because Unicorn releases the GIL. Measured 2026-11-02 on Base 16
frames: 2.45 s per frame on 1 thread, 0.69 s on 4; on colour negative it
catches dust as well as pakon_ice and leaves fine detail alone. Slides: both
smooth the grain over most of the frame, so the web app offers no dust
removal for them.
"""
import os
import struct
import time

import numpy as np

DLL = os.environ.get("PAKON_DICE_DLL") or os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "oem", "DMLDICELib.dll")

F_DLLMAIN = 0x1002C2C0          # entry point (_DllMainCRTStartup)
F_BEGIN = 0x1002B640            # DMLDICEBegin(ulong, DICEInfoStaticTag)
F_PROCESS = 0x1002B6F0          # DMLDICEProcess(void*, DICEInfoDynamicTag)
F_COUNT = 0x1002B780            # DMLDICEDefectCount(void*, dyn, ulong*, ulong*)
F_END = 0x1002B6D0              # DMLDICEEnd(void*)

STACK_TOP, STACK_SIZE = 0x0F000000, 0x00800000
TEB, GDT = 0x0F100000, 0x0F110000
STUB_BASE = 0x0E000000
HEAP_BASE, HEAP_SIZE = 0x20000000, 0x60000000      # 1.5 GB, committed lazily
RET_MAGIC = 0x0E7FFFF0
THREAD_RET = 0x0E7FFF00         # where a thread function run by a wait returns

# stdcall argument counts of the KERNEL32 imports
_ARGS = {
    "VirtualFree": 3, "WaitForSingleObject": 2, "CreateThread": 6, "SetThreadPriority": 2,
    "SetPriorityClass": 2, "GetCurrentProcess": 0, "RtlUnwind": 4, "GetCommandLineA": 0,
    "GetVersion": 0, "HeapFree": 3, "GetProcAddress": 2, "GetModuleHandleA": 1,
    "HeapAlloc": 3, "GetCurrentThreadId": 0, "TlsSetValue": 2, "TlsAlloc": 0, "TlsFree": 1,
    "SetLastError": 1, "TlsGetValue": 1, "GetLastError": 0, "EnterCriticalSection": 1,
    "LeaveCriticalSection": 1, "ExitProcess": 1, "TerminateProcess": 2, "SetHandleCount": 1,
    "GetStdHandle": 1, "GetFileType": 1, "GetStartupInfoA": 1, "DeleteCriticalSection": 1,
    "GetModuleFileNameA": 3, "FreeEnvironmentStringsA": 1, "FreeEnvironmentStringsW": 1,
    "WideCharToMultiByte": 8, "GetEnvironmentStrings": 0, "GetEnvironmentStringsW": 0,
    "HeapDestroy": 1, "HeapCreate": 3, "GetTickCount": 0, "WriteFile": 5,
    "InitializeCriticalSection": 1, "VirtualAlloc": 4, "HeapReAlloc": 4, "IsBadWritePtr": 2,
    "InterlockedDecrement": 1, "InterlockedIncrement": 1, "SetUnhandledExceptionFilter": 1,
    "IsBadReadPtr": 2, "IsBadCodePtr": 1, "GetCPInfo": 2, "GetACP": 0, "GetOEMCP": 0,
    "LoadLibraryA": 1, "MultiByteToWideChar": 6, "GetStringTypeA": 5, "GetStringTypeW": 4,
    "LCMapStringA": 6, "LCMapStringW": 6, "RaiseException": 4, "FlushFileBuffers": 1,
    "SetFilePointer": 4, "CloseHandle": 1, "SetStdHandle": 2,
}


def available():
    """True when Unicorn, pefile and the DLL are present; PAKON_OEM_DICE=0
    turns it off."""
    if os.environ.get("PAKON_OEM_DICE", "1") == "0":
        return False
    try:
        import pefile  # noqa: F401
        import unicorn  # noqa: F401
    except ImportError:
        return False
    return os.path.exists(DLL)


def _align(x, a=0x1000):
    return (x + a - 1) & ~(a - 1)


def _gdt_entry(base, limit, access, flags):
    v = limit & 0xFFFF
    v |= (base & 0xFFFFFF) << 16
    v |= (access & 0xFF) << 40
    v |= ((limit >> 16) & 0xF) << 48
    v |= (flags & 0xF) << 52
    v |= ((base >> 24) & 0xFF) << 56
    return struct.pack("<Q", v)


class Dice:
    """DMLDICELib.dll in a 32-bit Unicorn machine, Begin'd once."""

    def __init__(self, path=DLL):
        import pefile
        from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE
        from unicorn import x86_const as X
        pe = pefile.PE(path)
        self.base = base = pe.OPTIONAL_HEADER.ImageBase
        mu = self.mu = Uc(UC_ARCH_X86, UC_MODE_32)
        mu.mem_map(base, _align(pe.OPTIONAL_HEADER.SizeOfImage))
        mu.mem_write(base, pe.header)
        for s in pe.sections:
            mu.mem_write(base + s.VirtualAddress, s.get_data())
        mu.mem_map(STACK_TOP - STACK_SIZE, STACK_SIZE)
        mu.mem_map(TEB, 0x20000)
        mu.mem_map(HEAP_BASE, HEAP_SIZE)
        mu.mem_map(STUB_BASE, 0x00800000)
        # fs -> the thread block: an exception list end, stack range, self
        mu.mem_write(TEB, struct.pack("<7I", 0xFFFFFFFF, STACK_TOP, STACK_TOP - STACK_SIZE,
                                      0, 0, 0, TEB))
        # flat 32-bit code (1) and data (2), and the thread block (3); with a
        # GDT loaded, SS must be a 32-bit segment or pushes wrap at 64 KB
        mu.mem_write(GDT + 1 * 8, _gdt_entry(0, 0xFFFFF, 0x9A, 0xC))
        mu.mem_write(GDT + 2 * 8, _gdt_entry(0, 0xFFFFF, 0x92, 0xC))
        mu.mem_write(GDT + 3 * 8, _gdt_entry(TEB, 0xFFF, 0x92, 0x4))
        mu.reg_write(X.UC_X86_REG_GDTR, (0, GDT, 4 * 8 - 1, 0x0))
        for reg in (X.UC_X86_REG_SS, X.UC_X86_REG_DS, X.UC_X86_REG_ES):
            mu.reg_write(reg, 2 << 3)
        mu.reg_write(X.UC_X86_REG_FS, 3 << 3)

        self.heap = HEAP_BASE
        self.sizes, self.free_lists = {}, {}
        self.tls, self.threads, self.out = {}, {}, []
        self.stubs = {}
        n = 0
        for entry in pe.DIRECTORY_ENTRY_IMPORT:
            for imp in entry.imports:
                name = imp.name.decode()
                addr = STUB_BASE + 0x10 * n
                n += 1
                self.stubs[addr] = name
                mu.mem_write(imp.address, struct.pack("<I", addr))
                k = _ARGS.get(name, 0)
                mu.mem_write(addr, b"\xC2" + struct.pack("<H", 4 * k))   # ret 4k
        self.wait_stub = next(a for a, nm in self.stubs.items() if nm == "WaitForSingleObject")
        self.waits = []                                # ESP at each wait running a thread
        self.strings = self.alloc(0x100, b"dice\0\0\0C:\\DMLDICELib.dll\0".ljust(0x100, b"\0"))
        mu.mem_write(RET_MAGIC, b"\xF4")
        mu.hook_add(UC_HOOK_CODE, self._stub, begin=STUB_BASE, end=STUB_BASE + 0x007FFFFF)

        if not self.call(F_DLLMAIN, base, 1, 0):
            raise RuntimeError("DMLDICELib DllMain failed")
        self.handle = self.call(F_BEGIN, 0x65, 0x20, 2, 1, 0xE, 2, 0, 0, 0)
        if not self.handle:
            raise RuntimeError("DMLDICEBegin returned NULL")

    # -- memory
    def alloc(self, n, data=None):
        n = _align(max(n, 1), 16)
        lst = self.free_lists.get(n)
        if lst:
            p = lst.pop()
        else:
            p = self.heap
            self.heap += n
            if self.heap > HEAP_BASE + HEAP_SIZE:
                raise MemoryError("emulated heap exhausted")
        self.sizes[p] = n
        self.mu.mem_write(p, bytes(data) if data is not None else bytes(n))
        return p

    def free(self, p):
        n = self.sizes.pop(p, None)
        if n:
            self.free_lists.setdefault(n, []).append(p)

    def read(self, addr, n):
        return bytes(self.mu.mem_read(addr, n))

    def u32(self, addr):
        return struct.unpack("<I", self.read(addr, 4))[0]

    def _arg(self, i):
        from unicorn.x86_const import UC_X86_REG_ESP
        return self.u32(self.mu.reg_read(UC_X86_REG_ESP) + 4 + 4 * i)

    # -- KERNEL32
    def _stub(self, mu, addr, size, user):
        from unicorn.x86_const import UC_X86_REG_EAX, UC_X86_REG_ESP, UC_X86_REG_EIP
        if addr == THREAD_RET:                     # back from a thread: finish its wait
            mu.reg_write(UC_X86_REG_ESP, self.waits.pop())
            mu.reg_write(UC_X86_REG_EIP, self.wait_stub)
            return
        name = self.stubs.get(addr)
        if name is None:
            return
        a = self._arg
        r = 0
        if name in ("HeapAlloc",):
            r = self.alloc(a(2))
        elif name == "HeapReAlloc":
            old, n = a(2), a(3)
            r = self.alloc(n)
            keep = min(n, self.sizes.get(old, 0))
            if keep:
                mu.mem_write(r, self.read(old, keep))
            self.free(old)
        elif name == "HeapFree":
            self.free(a(2))
            r = 1
        elif name == "VirtualAlloc":
            r = a(0) if a(0) else self.alloc(_align(a(1)))
        elif name == "VirtualFree":
            if a(2) == 0x8000:                     # MEM_RELEASE
                self.free(a(0))
            r = 1
        elif name == "HeapCreate":
            r = 0x00AB0000
        elif name in ("HeapDestroy", "SetHandleCount", "SetStdHandle", "FlushFileBuffers",
                      "CloseHandle", "SetThreadPriority", "SetPriorityClass", "TlsFree"):
            r = 1
        elif name == "GetVersion":
            r = 0x0A280105                         # Windows XP
        elif name == "GetCommandLineA":
            r = self.strings
        elif name == "GetModuleFileNameA":
            s = b"C:\\DMLDICELib.dll\0"
            n = min(a(2), len(s))
            mu.mem_write(a(1), s[:n])
            r = n - 1
        elif name in ("GetModuleHandleA", "LoadLibraryA", "GetProcAddress",
                      "GetEnvironmentStringsW", "GetStringTypeA", "GetStringTypeW",
                      "LCMapStringA", "LCMapStringW", "SetUnhandledExceptionFilter",
                      "IsBadWritePtr", "IsBadReadPtr", "IsBadCodePtr", "GetLastError",
                      "SetLastError", "FreeEnvironmentStringsA", "FreeEnvironmentStringsW",
                      "InitializeCriticalSection", "EnterCriticalSection",
                      "LeaveCriticalSection", "DeleteCriticalSection"):
            r = 0
        elif name == "GetEnvironmentStrings":
            r = self.strings + 5                   # "\0\0"
        elif name == "GetStartupInfoA":
            mu.mem_write(a(0), struct.pack("<I", 68) + bytes(64))
        elif name == "GetStdHandle":
            r = {0xFFFFFFF6: 3, 0xFFFFFFF5: 7, 0xFFFFFFF4: 11}.get(a(0), 0xFFFFFFFF)
        elif name == "GetFileType":
            r = 2                                  # FILE_TYPE_CHAR
        elif name == "GetCPInfo":
            mu.mem_write(a(1), struct.pack("<I", 1) + b"?\0" + bytes(12))
            r = 1
        elif name in ("GetACP", "GetOEMCP"):
            r = 1252
        elif name in ("MultiByteToWideChar", "WideCharToMultiByte"):
            r = 0
        elif name == "GetTickCount":
            r = int(time.monotonic() * 1000)
        elif name == "GetCurrentThreadId":
            r = 1
        elif name == "GetCurrentProcess":
            r = 0xFFFFFFFF
        elif name == "TlsAlloc":
            r = 1
        elif name == "TlsSetValue":
            self.tls[a(0)] = a(1)
            r = 1
        elif name == "TlsGetValue":
            r = self.tls.get(a(0), 0)
        elif name in ("InterlockedIncrement", "InterlockedDecrement"):
            p = a(0)
            r = (self.u32(p) + (1 if name == "InterlockedIncrement" else -1)) & 0xFFFFFFFF
            mu.mem_write(p, struct.pack("<I", r))
        elif name == "WriteFile":
            n = a(2)
            self.out.append(self.read(a(1), n).decode("latin-1"))
            if a(3):
                mu.mem_write(a(3), struct.pack("<I", n))
            r = 1
        elif name == "CreateThread":
            h = 0x7000 + 4 * len(self.threads)
            self.threads[h] = [a(2), a(3), False]  # proc, param, ran
            if a(5):
                mu.mem_write(a(5), struct.pack("<I", h))
            r = h
        elif name == "WaitForSingleObject":
            t = self.threads.get(a(0))
            if t and not t[2]:
                # run the thread now: call proc(param), returning to THREAD_RET
                t[2] = True
                self.waits.append(mu.reg_read(UC_X86_REG_ESP))
                esp = self.waits[-1] - 0x100 - 8
                mu.mem_write(esp, struct.pack("<II", THREAD_RET, t[1]))
                mu.reg_write(UC_X86_REG_ESP, esp)
                mu.reg_write(UC_X86_REG_EIP, t[0])
                return
            r = 0                                  # WAIT_OBJECT_0
        else:
            mu.emu_stop()
            raise RuntimeError(f"DMLDICELib called an unhandled import: {name}")
        mu.reg_write(UC_X86_REG_EAX, r & 0xFFFFFFFF)

    def call(self, func, *args):
        from unicorn import UcError
        from unicorn.x86_const import UC_X86_REG_ESP, UC_X86_REG_EAX, UC_X86_REG_EIP
        frame = struct.pack("<I", RET_MAGIC) + b"".join(
            struct.pack("<I", x & 0xFFFFFFFF) for x in args)
        esp = STACK_TOP - 0x1000 - len(frame)
        self.mu.mem_write(esp, frame)
        self.mu.reg_write(UC_X86_REG_ESP, esp)
        try:
            self.mu.emu_start(func, RET_MAGIC)
        except UcError as e:
            raise RuntimeError(f"emulation fault at "
                               f"{self.mu.reg_read(UC_X86_REG_EIP):#x}: {e}") from None
        return self.mu.reg_read(UC_X86_REG_EAX)

    # -- one picture
    def process(self, rgb14, ir14, negative=True, swap=False):
        """rgb14 (H, W, 3), ir14 (H, W): 14-bit. Cleans in place in the
        emulated buffer and returns (rgb14 out, defect counts, seconds).
        swap=True passes the dimensions the other way round."""
        h, w = ir14.shape
        planes = np.concatenate([np.moveaxis(rgb14, -1, 0).astype("<u2"),
                                 ir14[None].astype("<u2")]).tobytes()
        buf = self.alloc(len(planes), planes)
        d0, d1 = (h, w) if swap else (w, h)
        dyn = (0x30, buf, d0, d1, 300, int(bool(negative)), 0, 0, 0, d0 - 1, d1 - 1, 0)
        self.out.clear()
        t0 = time.perf_counter()
        rc = self.call(F_PROCESS, self.handle, *dyn)
        secs = time.perf_counter() - t0
        if rc:
            self.free(buf)
            raise RuntimeError(f"DMLDICEProcess returned {rc - (1 << 32) if rc >> 31 else rc}")
        counts = self.alloc(8)
        self.call(F_COUNT, self.handle, *dyn, counts, counts + 4)
        n = struct.unpack("<II", self.read(counts, 8))
        self.free(counts)
        out = np.frombuffer(self.read(buf, len(planes)), "<u2").reshape(4, h, w)
        self.free(buf)
        return np.moveaxis(out[:3], 0, -1).copy(), n, secs


def to14(raw16):
    """16-bit samples (black at 300) -> the 14-bit values TLB hands DICE."""
    return np.clip((np.asarray(raw16, np.float32) - 300.0) / 4.0, 0, 16383).astype(np.uint16)


def from14(v14):
    return np.clip(np.asarray(v14, np.float32) * 4.0 + 300.0 + 0.5, 0, 65535).astype(np.uint16)


def clean_frame(dice, raw, ir, negative=True):
    """pakon_ice.clean_frame's interface: raw (H, W, 3) 16-bit registered,
    flat-fielded samples (black at 300), ir (H, W) from pakon_ice.ir_plane
    -> (uint16 (H, W, 3), mask of rebuilt pixels). The rebuilt pixels come
    from DICE, every other one is raw as it was. negative=False for slides
    (TLB passes film type != 2; it made no difference on our slide scans)."""
    raw = np.asarray(raw, np.uint16)
    rgb14 = to14(raw)
    out, _, _ = dice.process(rgb14, to14(np.asarray(ir, np.float32) + 300.0), negative)
    mask = (out != rgb14).any(axis=-1)
    return np.where(mask[..., None], from14(out), raw), mask


if __name__ == "__main__":
    t0 = time.perf_counter()
    d = Dice()
    print(f"{DLL}: DllMain + Begin {time.perf_counter() - t0:.2f}s, handle {d.handle:#x}")
