// In-memory `/tmp` for the guest: a WASI preview1 file system with one preopened directory.
//
// Per call and discarded after it (the host resets it when a call starts and when it ends cleanly).
// File data lives in the GUEST's linear memory when the module exports `convex_alloc` (segments are
// allocated through it and handed back through `convex_free`), so it does not count toward the JS heap
// of the isolate (64 MiB in the default Convex runtime). Without those exports (hand-written test
// modules) the segments are plain `Uint8Array`s. Either way the total is capped (`maxBytes`, default
// 64 MiB, ENOSPC beyond it). contracts/ABI.md "Optional WASI: /tmp".

export const TMP_DIR = "/tmp";
/** Default cap on the allocated bytes of all files. */
export const DEFAULT_TMP_BYTES = 64 * 1024 * 1024;
const FIRST_SEG = 4096;
const MAX_SEG = 1024 * 1024;
const PREOPEN_FD = 3;

const E = {
  SUCCESS: 0,
  ACCES: 2,
  BADF: 8,
  EXIST: 20,
  INVAL: 28,
  ISDIR: 31,
  NOENT: 44,
  NOSPC: 51,
  NOSYS: 52,
  NOTDIR: 54,
  NOTEMPTY: 55,
  PERM: 63,
  SPIPE: 70,
  NOTCAPABLE: 76,
} as const;

const FT_DIR = 3;
const FT_FILE = 4;
// oflags
const O_CREAT = 1;
const O_DIRECTORY = 2;
const O_EXCL = 4;
const O_TRUNC = 8;
// fdflags
const FD_APPEND = 1;
// rights we care about (the rest are granted freely: the whole tree is the guest's own)
const RIGHT_FD_WRITE = 1n << 6n;
const ALL_RIGHTS = 0x1fffffffn;

const enc = new TextEncoder();
const dec = new TextDecoder();

interface Seg {
  /** Guest address, or -1 when `arr` holds the bytes. */
  ptr: number;
  arr?: Uint8Array;
  cap: number;
}

interface Base {
  ino: number;
  parent: DirNode | null;
  atim: bigint;
  mtim: bigint;
  ctim: bigint;
  /** Open descriptors; an unlinked node is released when the last one closes. */
  open: number;
  unlinked: boolean;
}
interface FileNode extends Base {
  kind: "file";
  segs: Seg[];
  size: number;
}
interface DirNode extends Base {
  kind: "dir";
  entries: Map<string, Node>;
}
type Node = FileNode | DirNode;

interface OpenFd {
  node: Node;
  pos: number;
  append: boolean;
  writable: boolean;
}

export interface TmpFsOptions {
  maxBytes?: number;
  memory: () => WebAssembly.Memory;
  /** Guest allocator (`convex_alloc`); absent => JS-backed segments. */
  alloc?: (len: number) => number;
  /** `convex_free(ptr, len)`; absent => guest segments are only forgotten on reset. */
  free?: (ptr: number, len: number) => void;
}

const nowNs = () => BigInt(Date.now()) * 1_000_000n;

class Errno extends Error {
  constructor(public code: number) {
    super(`errno ${code}`);
  }
}

export class TmpFs {
  maxBytes: number;
  private opts: TmpFsOptions;
  private root!: DirNode;
  private fds = new Map<number, OpenFd>();
  private nextFd = PREOPEN_FD + 1;
  private nextIno = 1;
  /** Sum of the allocated segment capacities. */
  used = 0;

  constructor(opts: TmpFsOptions) {
    this.opts = opts;
    this.maxBytes = opts.maxBytes ?? DEFAULT_TMP_BYTES;
    this.reset(false);
  }

  /** Empties the file system. `release` hands the guest segments back to the guest allocator. */
  reset(release: boolean) {
    if (release && this.root) this.freeTree(this.root);
    this.fds.clear();
    this.nextFd = PREOPEN_FD + 1;
    this.used = 0;
    this.root = this.mkdir(null);
  }

  private freeTree(n: Node) {
    if (n.kind === "dir") for (const c of n.entries.values()) this.freeTree(c);
    else this.freeSegs(n);
  }

  // ---- storage ----
  private newBase(parent: DirNode | null): Base {
    const t = nowNs();
    return { ino: this.nextIno++, parent, atim: t, mtim: t, ctim: t, open: 0, unlinked: false };
  }
  private mkdir(parent: DirNode | null): DirNode {
    return { ...this.newBase(parent), kind: "dir", entries: new Map() };
  }
  private mkfile(parent: DirNode): FileNode {
    return { ...this.newBase(parent), kind: "file", segs: [], size: 0 };
  }
  private segCap(i: number) {
    return Math.min(FIRST_SEG * 2 ** Math.min(i, 20), MAX_SEG);
  }
  private view(s: Seg): Uint8Array {
    return s.arr ?? new Uint8Array(this.opts.memory().buffer, s.ptr, s.cap);
  }
  private freeSegs(f: FileNode) {
    for (const s of f.segs) {
      if (s.ptr >= 0 && this.opts.free) this.opts.free(s.ptr, s.cap);
      this.used -= s.cap;
    }
    f.segs = [];
    f.size = 0;
  }
  /** Makes sure the file has segments for `size` bytes; throws ENOSPC beyond the cap or when the guest cannot allocate. */
  private reserve(f: FileNode, size: number) {
    let have = f.segs.reduce((n, s) => n + s.cap, 0);
    while (have < size) {
      const cap = this.segCap(f.segs.length);
      if (this.used + cap > this.maxBytes) throw new Errno(E.NOSPC);
      let seg: Seg;
      if (this.opts.alloc) {
        let ptr = 0;
        try {
          ptr = this.opts.alloc(cap) >>> 0;
        } catch {
          throw new Errno(E.NOSPC);
        }
        if (ptr === 0) throw new Errno(E.NOSPC);
        // convex_alloc hands out uninitialised memory, a fresh segment must read as zeros.
        new Uint8Array(this.opts.memory().buffer, ptr, cap).fill(0);
        seg = { ptr, cap };
      } else {
        seg = { ptr: -1, arr: new Uint8Array(cap), cap };
      }
      f.segs.push(seg);
      this.used += cap;
      have += cap;
    }
  }
  /** Copies between the file and `buf`; callers have reserved the range for writes. */
  private xfer(f: FileNode, pos: number, buf: Uint8Array, write: boolean): number {
    let done = 0;
    let base = 0;
    for (let i = 0; i < f.segs.length && done < buf.length; i++) {
      const s = f.segs[i];
      const end = base + s.cap;
      if (pos + done < end) {
        const off = pos + done - base;
        const n = Math.min(s.cap - off, buf.length - done);
        const v = this.view(s);
        if (write) v.set(buf.subarray(done, done + n), off);
        else buf.set(v.subarray(off, off + n), done);
        done += n;
      }
      base = end;
    }
    return done;
  }
  private zero(f: FileNode, from: number, to: number) {
    if (to <= from) return;
    this.xfer(f, from, new Uint8Array(to - from), true);
  }
  private setSize(f: FileNode, size: number) {
    if (size > f.size) {
      this.reserve(f, size);
      this.zero(f, f.size, size); // truncated tails keep stale bytes: zero them
    }
    f.size = size;
  }
  private release(n: Node) {
    if (n.kind === "file" && n.unlinked && n.open === 0) this.freeSegs(n);
  }

  // ---- paths ----
  private dirOf(fd: number): DirNode {
    if (fd === PREOPEN_FD) return this.root;
    const o = this.fds.get(fd);
    if (!o) throw new Errno(E.BADF);
    if (o.node.kind !== "dir") throw new Errno(E.NOTDIR);
    return o.node;
  }
  /** Resolves `path` relative to `dir`. Returns the parent directory, the last name and the node (if it exists). */
  private walk(dir: DirNode, path: string): { parent: DirNode; name: string; node: Node | undefined } {
    if (path.startsWith("/")) throw new Errno(E.NOTCAPABLE);
    const parts = path.split("/").filter((p) => p !== "" && p !== ".");
    let cur = dir;
    if (parts.length === 0) return { parent: dir, name: "", node: dir };
    for (let i = 0; i < parts.length; i++) {
      const last = i === parts.length - 1;
      const p = parts[i];
      if (p === "..") {
        if (!cur.parent) throw new Errno(E.NOTCAPABLE); // would leave the sandbox
        cur = cur.parent;
        if (last) return { parent: cur.parent ?? cur, name: "..", node: cur };
        continue;
      }
      if (p.length > 255) throw new Errno(E.INVAL);
      const next = cur.entries.get(p);
      if (last) return { parent: cur, name: p, node: next };
      if (!next) throw new Errno(E.NOENT);
      if (next.kind !== "dir") throw new Errno(E.NOTDIR);
      cur = next;
    }
    throw new Errno(E.INVAL);
  }
  private unlinkNode(parent: DirNode, name: string, n: Node) {
    parent.entries.delete(name);
    parent.mtim = parent.ctim = nowNs();
    n.unlinked = true;
    this.release(n);
  }

  // ---- operations (errno in, errno out) ----
  open(dirfd: number, path: string, oflags: number, rightsBase: bigint, fdflags: number): number {
    const dir = this.dirOf(dirfd);
    const { parent, name, node } = this.walk(dir, path);
    let target = node;
    if (!target) {
      if (!(oflags & O_CREAT)) throw new Errno(E.NOENT);
      if (oflags & O_DIRECTORY) throw new Errno(E.INVAL);
      if (name === "" || name === "..") throw new Errno(E.INVAL);
      const f = this.mkfile(parent);
      parent.entries.set(name, f);
      parent.mtim = parent.ctim = nowNs();
      target = f;
    } else {
      if (oflags & O_CREAT && oflags & O_EXCL) throw new Errno(E.EXIST);
      if (oflags & O_DIRECTORY && target.kind !== "dir") throw new Errno(E.NOTDIR);
      if (target.kind === "file" && oflags & O_TRUNC) {
        this.setSize(target, 0);
        target.mtim = target.ctim = nowNs();
      }
    }
    if (target.kind === "dir" && oflags & O_TRUNC) throw new Errno(E.ISDIR);
    const fd = this.nextFd++;
    target.open++;
    this.fds.set(fd, {
      node: target,
      pos: 0,
      append: (fdflags & FD_APPEND) !== 0,
      writable: (rightsBase & RIGHT_FD_WRITE) !== 0n,
    });
    return fd;
  }
  close(fd: number) {
    const o = this.fds.get(fd);
    if (!o) throw new Errno(E.BADF);
    this.fds.delete(fd);
    o.node.open--;
    this.release(o.node);
  }
  has(fd: number) {
    return fd === PREOPEN_FD || this.fds.has(fd);
  }
  private file(fd: number): OpenFd & { node: FileNode } {
    const o = this.fds.get(fd);
    if (!o) throw new Errno(fd === PREOPEN_FD ? E.ISDIR : E.BADF);
    if (o.node.kind !== "file") throw new Errno(E.ISDIR);
    return o as OpenFd & { node: FileNode };
  }
  read(fd: number, bufs: Uint8Array[], at?: number): number {
    const o = this.file(fd);
    let pos = at ?? o.pos;
    let total = 0;
    for (const b of bufs) {
      const n = Math.max(0, Math.min(b.length, o.node.size - pos));
      if (n === 0) break;
      this.xfer(o.node, pos, b.subarray(0, n), false);
      pos += n;
      total += n;
      if (n < b.length) break;
    }
    if (at === undefined) o.pos = pos;
    o.node.atim = nowNs();
    return total;
  }
  /** `chunks` are copied before any allocation (an allocation can grow, and so detach, the guest memory). */
  write(fd: number, chunks: Uint8Array[], at?: number): number {
    const o = this.file(fd);
    if (!o.writable) throw new Errno(E.BADF);
    const len = chunks.reduce((n, c) => n + c.length, 0);
    let pos = at ?? (o.append ? o.node.size : o.pos);
    if (pos + len > 0xffffffff) throw new Errno(E.NOSPC);
    const joined = new Uint8Array(len);
    let k = 0;
    for (const c of chunks) {
      joined.set(c, k);
      k += c.length;
    }
    this.reserve(o.node, pos + len);
    if (pos > o.node.size) this.zero(o.node, o.node.size, pos);
    this.xfer(o.node, pos, joined, true);
    pos += len;
    if (pos > o.node.size) o.node.size = pos;
    if (at === undefined) o.pos = pos;
    o.node.mtim = o.node.ctim = nowNs();
    return len;
  }
  seek(fd: number, offset: bigint, whence: number): number {
    const o = this.fds.get(fd);
    if (!o) throw new Errno(E.BADF);
    if (o.node.kind !== "file") throw new Errno(E.SPIPE);
    const size = o.node.size;
    const base = whence === 0 ? 0 : whence === 1 ? o.pos : whence === 2 ? size : -1;
    if (base < 0) throw new Errno(E.INVAL);
    const target = BigInt(base) + offset;
    if (target < 0n || target > 0xffffffffn) throw new Errno(E.INVAL);
    o.pos = Number(target);
    return o.pos;
  }
  tell(fd: number): number {
    const o = this.fds.get(fd);
    if (!o) throw new Errno(E.BADF);
    return o.pos;
  }
  truncate(fd: number, size: bigint) {
    const o = this.file(fd);
    if (!o.writable) throw new Errno(E.BADF);
    if (size > 0xffffffffn) throw new Errno(E.NOSPC);
    this.setSize(o.node, Number(size));
    o.node.mtim = o.node.ctim = nowNs();
  }
  allocate(fd: number, offset: bigint, len: bigint) {
    const o = this.file(fd);
    const end = offset + len;
    if (end > 0xffffffffn) throw new Errno(E.NOSPC);
    if (Number(end) > o.node.size) this.setSize(o.node, Number(end));
  }
  node(fd: number): Node {
    if (fd === PREOPEN_FD) return this.root;
    const o = this.fds.get(fd);
    if (!o) throw new Errno(E.BADF);
    return o.node;
  }
  stat(dirfd: number, path: string): Node {
    const dir = this.dirOf(dirfd);
    const { node } = this.walk(dir, path);
    if (!node) throw new Errno(E.NOENT);
    return node;
  }
  createDir(dirfd: number, path: string) {
    const { parent, name, node } = this.walk(this.dirOf(dirfd), path);
    if (node) throw new Errno(E.EXIST);
    if (name === "") throw new Errno(E.EXIST);
    parent.entries.set(name, this.mkdir(parent));
    parent.mtim = parent.ctim = nowNs();
  }
  removeDir(dirfd: number, path: string) {
    const { parent, name, node } = this.walk(this.dirOf(dirfd), path);
    if (!node) throw new Errno(E.NOENT);
    if (node.kind !== "dir") throw new Errno(E.NOTDIR);
    if (name === "" || name === "..") throw new Errno(E.INVAL);
    if (node.entries.size > 0) throw new Errno(E.NOTEMPTY);
    this.unlinkNode(parent, name, node);
  }
  unlink(dirfd: number, path: string) {
    const { parent, name, node } = this.walk(this.dirOf(dirfd), path);
    if (!node) throw new Errno(E.NOENT);
    if (node.kind === "dir") throw new Errno(E.ISDIR);
    this.unlinkNode(parent, name, node);
  }
  rename(oldfd: number, oldPath: string, newfd: number, newPath: string) {
    const a = this.walk(this.dirOf(oldfd), oldPath);
    const b = this.walk(this.dirOf(newfd), newPath);
    if (!a.node) throw new Errno(E.NOENT);
    if (a.name === "" || a.name === ".." || b.name === "" || b.name === "..") throw new Errno(E.INVAL);
    if (a.node === b.node) return;
    if (a.node.kind === "dir") {
      for (let p: DirNode | null = b.parent; p; p = p.parent) if (p === a.node) throw new Errno(E.INVAL);
    }
    if (b.node) {
      if (a.node.kind === "dir" && b.node.kind !== "dir") throw new Errno(E.NOTDIR);
      if (a.node.kind !== "dir" && b.node.kind === "dir") throw new Errno(E.ISDIR);
      if (b.node.kind === "dir" && b.node.entries.size > 0) throw new Errno(E.NOTEMPTY);
      this.unlinkNode(b.parent, b.name, b.node);
    }
    a.parent.entries.delete(a.name);
    b.parent.entries.set(b.name, a.node);
    a.node.parent = b.parent;
    a.parent.mtim = b.parent.mtim = a.node.ctim = nowNs();
  }
  /** Directory listing for `fd_readdir`: `.`, `..`, then the entries in creation order. */
  list(fd: number): { name: string; node: Node }[] {
    const o = this.fds.get(fd);
    const dir = fd === PREOPEN_FD ? this.root : o?.node;
    if (!dir) throw new Errno(E.BADF);
    if (dir.kind !== "dir") throw new Errno(E.NOTDIR);
    return [
      { name: ".", node: dir },
      { name: "..", node: dir.parent ?? dir },
      ...[...dir.entries].map(([name, node]) => ({ name, node })),
    ];
  }
  setTimes(n: Node, atim: bigint | null, mtim: bigint | null) {
    if (atim !== null) n.atim = atim;
    if (mtim !== null) n.mtim = mtim;
  }
  fdInfo(fd: number): { filetype: number; append: boolean } {
    const n = this.node(fd);
    return { filetype: n.kind === "dir" ? FT_DIR : FT_FILE, append: this.fds.get(fd)?.append ?? false };
  }
}

/** The `wasi_snapshot_preview1` file-system functions backed by `fs`. Merged into the table of `makeWasi`. */
export function tmpFsImports(fs: TmpFs, memory: () => WebAssembly.Memory) {
  const view = () => new DataView(memory().buffer);
  const bytes = () => new Uint8Array(memory().buffer);
  const FAULT = 21;
  // Guest pointers are u32; a wasm i32 import argument arrives sign-extended when >= 2 GiB.
  const str = (ptr: number, len: number) => {
    ptr >>>= 0;
    len >>>= 0;
    const m = bytes();
    if (ptr + len > m.length) throw new Errno(FAULT);
    return dec.decode(m.subarray(ptr, ptr + len));
  };
  const iovs = (ptr: number, n: number) => {
    ptr >>>= 0;
    n >>>= 0;
    const v = view();
    const m = bytes();
    if (ptr + n * 8 > m.length) throw new Errno(FAULT);
    const out: Uint8Array[] = [];
    for (let i = 0; i < n; i++) {
      const base = v.getUint32(ptr + i * 8, true);
      const len = v.getUint32(ptr + i * 8 + 4, true);
      if (base + len > m.length) throw new Errno(FAULT);
      out.push(m.subarray(base, base + len));
    }
    return out;
  };
  const guard =
    <A extends any[]>(f: (...a: A) => void) =>
    (...a: A): number => {
      try {
        f(...(a.map((x) => (typeof x === "number" ? x >>> 0 : x)) as unknown as A));
        return E.SUCCESS;
      } catch (e) {
        if (e instanceof Errno) return e.code;
        if (e instanceof RangeError) return FAULT; // pointer outside linear memory
        throw e;
      }
    };
  const writeStat = (ptr: number, n: Node) => {
    const v = view();
    v.setBigUint64(ptr, 0n, true);
    v.setBigUint64(ptr + 8, BigInt(n.ino), true);
    v.setUint8(ptr + 16, n.kind === "dir" ? FT_DIR : FT_FILE);
    v.setBigUint64(ptr + 24, n.unlinked ? 0n : 1n, true);
    v.setBigUint64(ptr + 32, BigInt(n.kind === "file" ? n.size : 4096), true);
    v.setBigUint64(ptr + 40, n.atim, true);
    v.setBigUint64(ptr + 48, n.mtim, true);
    v.setBigUint64(ptr + 56, n.ctim, true);
  };
  const times = (atim: bigint, mtim: bigint, flags: number) => {
    const t = nowNs();
    return [
      flags & 1 ? atim : flags & 2 ? t : null,
      flags & 4 ? mtim : flags & 8 ? t : null,
    ] as const;
  };

  return {
    fd_prestat_get: guard((fd: number, ptr: number) => {
      if (fd !== PREOPEN_FD) throw new Errno(E.BADF);
      const v = view();
      v.setUint8(ptr, 0);
      v.setUint32(ptr + 4, TMP_DIR.length, true);
    }),
    fd_prestat_dir_name: guard((fd: number, ptr: number, len: number) => {
      if (fd !== PREOPEN_FD) throw new Errno(E.BADF);
      if (len < TMP_DIR.length) throw new Errno(E.INVAL);
      bytes().set(enc.encode(TMP_DIR), ptr);
    }),
    path_open: guard(
      (
        dirfd: number,
        _dirflags: number,
        pathPtr: number,
        pathLen: number,
        oflags: number,
        rightsBase: bigint,
        _rightsInh: bigint,
        fdflags: number,
        outPtr: number,
      ) => {
        const fd = fs.open(dirfd, str(pathPtr, pathLen), oflags, BigInt(rightsBase), fdflags);
        view().setUint32(outPtr, fd, true);
      },
    ),
    fd_close: guard((fd: number) => fs.close(fd)),
    fd_read: guard((fd: number, iov: number, n: number, outPtr: number) => {
      const r = fs.read(fd, iovs(iov, n));
      view().setUint32(outPtr, r, true);
    }),
    fd_pread: guard((fd: number, iov: number, n: number, off: bigint, outPtr: number) => {
      const r = fs.read(fd, iovs(iov, n), Number(off));
      view().setUint32(outPtr, r, true);
    }),
    fd_pwrite: guard((fd: number, iov: number, n: number, off: bigint, outPtr: number) => {
      const r = fs.write(fd, iovs(iov, n), Number(off));
      view().setUint32(outPtr, r, true);
    }),
    fd_seek: guard((fd: number, off: bigint, whence: number, outPtr: number) => {
      const p = fs.seek(fd, BigInt(off), whence);
      view().setBigUint64(outPtr, BigInt(p), true);
    }),
    fd_tell: guard((fd: number, outPtr: number) => {
      view().setBigUint64(outPtr, BigInt(fs.tell(fd)), true);
    }),
    fd_sync: guard((fd: number) => void fs.node(fd)),
    fd_datasync: guard((fd: number) => void fs.node(fd)),
    fd_advise: guard((fd: number) => void fs.node(fd)),
    fd_allocate: guard((fd: number, off: bigint, len: bigint) => fs.allocate(fd, BigInt(off), BigInt(len))),
    fd_fdstat_get: guard((fd: number, ptr: number) => {
      const info = fs.fdInfo(fd);
      const v = view();
      v.setUint8(ptr, info.filetype);
      v.setUint16(ptr + 2, info.append ? FD_APPEND : 0, true);
      v.setBigUint64(ptr + 8, ALL_RIGHTS, true);
      v.setBigUint64(ptr + 16, ALL_RIGHTS, true);
    }),
    fd_fdstat_set_flags: guard((fd: number) => void fs.node(fd)),
    fd_filestat_get: guard((fd: number, ptr: number) => writeStat(ptr, fs.node(fd))),
    fd_filestat_set_size: guard((fd: number, size: bigint) => fs.truncate(fd, BigInt(size))),
    fd_filestat_set_times: guard((fd: number, a: bigint, m: bigint, flags: number) => {
      const [at, mt] = times(BigInt(a), BigInt(m), flags);
      fs.setTimes(fs.node(fd), at, mt);
    }),
    path_filestat_get: guard((fd: number, _flags: number, p: number, l: number, out: number) =>
      writeStat(out, fs.stat(fd, str(p, l))),
    ),
    path_filestat_set_times: guard(
      (fd: number, _fl: number, p: number, l: number, a: bigint, m: bigint, flags: number) => {
        const [at, mt] = times(BigInt(a), BigInt(m), flags);
        fs.setTimes(fs.stat(fd, str(p, l)), at, mt);
      },
    ),
    path_create_directory: guard((fd: number, p: number, l: number) => fs.createDir(fd, str(p, l))),
    path_remove_directory: guard((fd: number, p: number, l: number) => fs.removeDir(fd, str(p, l))),
    path_unlink_file: guard((fd: number, p: number, l: number) => fs.unlink(fd, str(p, l))),
    path_rename: guard((ofd: number, op: number, ol: number, nfd: number, np: number, nl: number) =>
      fs.rename(ofd, str(op, ol), nfd, str(np, nl)),
    ),
    path_readlink: () => E.INVAL,
    path_symlink: () => E.PERM,
    path_link: () => E.PERM,
    fd_readdir: guard((fd: number, buf: number, bufLen: number, cookie: bigint, usedPtr: number) => {
      const entries = fs.list(fd);
      const out: number[] = [];
      for (let i = Number(cookie); i < entries.length && out.length < bufLen; i++) {
        const { name, node } = entries[i];
        const nb = enc.encode(name);
        const rec = new Uint8Array(24 + nb.length);
        const dv = new DataView(rec.buffer);
        dv.setBigUint64(0, BigInt(i + 1), true);
        dv.setBigUint64(8, BigInt(node.ino), true);
        dv.setUint32(16, nb.length, true);
        dv.setUint8(20, node.kind === "dir" ? FT_DIR : FT_FILE);
        rec.set(nb, 24);
        for (const b of rec) out.push(b);
      }
      const n = Math.min(out.length, bufLen);
      bytes().set(out.slice(0, n), buf);
      view().setUint32(usedPtr, n, true);
    }),
    /** fd_write for descriptors of this file system (stdout/stderr stay with the caller). */
    write: (fd: number, iov: number, n: number, outPtr: number): number =>
      guard(() => {
        outPtr >>>= 0;
        const r = fs.write(fd, iovs(iov, n));
        view().setUint32(outPtr, r, true);
      })(),
  };
}
