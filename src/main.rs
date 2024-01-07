use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    Request,
};
use gio::prelude::FileExt;
use gio::FileInfo;
use libc::ENOENT;
use ostree;
use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsStr;
use std::os::unix::fs::DirEntryExt;
use std::time::{Duration, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1); // 1 second
const FLAGS_NONE: gio::FileQueryInfoFlags = gio::FileQueryInfoFlags::NONE;
const CANCEL_NONE: Option<&gio::Cancellable> = gio::Cancellable::NONE;
const HELLO_DIR_ATTR: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH, // 1970-01-01 00:00:00
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
    blksize: 512,
};
const HELLO_TXT_CONTENT: &str = "Hello World!\n";
const HELLO_TXT_ATTR: FileAttr = FileAttr {
    ino: 2,
    size: 13,
    blocks: 1,
    atime: UNIX_EPOCH, // 1970-01-01 00:00:00
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::RegularFile,
    perm: 0o644,
    nlink: 1,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
    blksize: 512,
};

struct MyFS {
    ostree_file: gio::File,
    inoMap: HashMap<u64, String>,
    pathMap: HashMap<String, u64>,
    inoIndex: u64,
}

impl Filesystem for MyFS {
    // 读取目录
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        println!("readdir ino:{} offset:{}", ino, offset);
        let file = if ino == 1 {
            self.ostree_file.clone()
        } else {
            let path = self.inoMap.get(&ino);
            if path.is_none() {
                println!("get path by ino error {}", ino);
                return reply.error(ENOENT);
            }
            self.ostree_file.resolve_relative_path(path.unwrap())
        };
        let children = file.enumerate_children("", FLAGS_NONE, CANCEL_NONE);
        if children.is_err() {
            println!("enumerate_children err({}): {:?}", ino, children.err());
            return reply.error(ENOENT);
        }
        let mut i = offset;
        for info in children.unwrap().skip(offset as usize) {
            if info.is_err() {
                println!("children err {} {:?}", ino, info.err());
                return reply.error(ENOENT);
            }
            let info = info.unwrap();
            let path = format!("/{}", info.name().to_str().unwrap().to_string());

            let path_ino = self.pathMap.get(&path);
            let ino = if path_ino.is_none() {
                self.inoIndex += 1;
                self.inoIndex.clone()
            } else {
                path_ino.unwrap().clone()
            };

            let attr = info2attr(&info, ino);
            i = i + 1;
            println!(
                "add ino:{} offset:{} kind:{:?} name:{:?}",
                attr.ino,
                i,
                attr.kind,
                info.name()
            );
            self.inoMap.insert(ino, path.clone());
            self.pathMap.insert(path.clone(), ino);
            let ok = reply.add(attr.ino, i as i64, attr.kind, info.name());
            if !ok {
                println!("reply add failed");
                break;
            }
        }
        return reply.ok();
    }
    // 定位目录内的文件
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        println!("lookup {} {:?}", parent, name.to_str());
        // 获取完整的path
        let mut path = name.to_str().unwrap().to_string();
        if parent != 1 {
            let parent_file = self.inoMap.get(&parent);
            if parent_file.is_none() {
                return reply.error(ENOENT);
            }
            path = format!("{}/{}", parent_file.unwrap(), path);
        }
        // 根据path获取文件信息
        println!("lookup {}", path);
        let f = self.ostree_file.resolve_relative_path(&path);
        let info = f.query_info("", FLAGS_NONE, CANCEL_NONE);
        if info.is_err() {
            println!("query info err {:?}", info.err());
            return reply.error(ENOENT);
        }
        let path_ino = self.pathMap.get(&path);
        let ino = if path_ino.is_none() {
            self.inoIndex += 1;
            self.inoIndex.clone()
        } else {
            path_ino.unwrap().clone()
        };
        self.inoMap.insert(ino, path.clone());
        self.pathMap.insert(path.clone(), ino);
        let attr = info2attr(&info.unwrap(), ino);
        return reply.entry(&TTL, &attr, 0);
    }
    //  获取文件属性
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        println!("getattr {}", ino);
        if ino == 1 {
            let info = self.ostree_file.query_info("", FLAGS_NONE, CANCEL_NONE);
            let attr = info2attr(&info.unwrap(), ino);
            return reply.attr(&TTL, &attr);
        }
        let path = self.inoMap.get(&ino);
        if path.is_none() {
            println!("ino map none {}", ino);
            return reply.error(ENOENT);
        }
        let f = self.ostree_file.resolve_relative_path(path.unwrap());
        let info = f.query_info("", FLAGS_NONE, CANCEL_NONE);
        if info.is_err() {
            println!("query info error({}): {:?}", path.unwrap(), info.err());
            return reply.error(ENOENT);
        }
        let attr = info2attr(&info.unwrap(), ino);
        return reply.attr(&TTL, &attr);
    }
    // 读取文件
    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        _size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        println!("read {}", ino);
        let path = self.inoMap.get(&ino);
        if path.is_none() {
            println!("ino map none {}", ino);
            return reply.error(ENOENT);
        }
        let f = self.ostree_file.resolve_relative_path(path.unwrap());
        let data = f.load_bytes(CANCEL_NONE);
        if data.is_err() {
            println!("load bytes {}", ino);
            return reply.error(ENOENT);
        }
        reply.data(&data.unwrap().0);
    }
}

fn main() {
    let ret = refs();
    if ret.is_err() {
        println!("{:?}", ret.err())
    }
}

fn refs() -> Result<String, Box<dyn Error>> {
    let repo = ostree::Repo::new_for_path("repo");
    let cancel = gio::Cancellable::NONE;
    // let flags = gio::FileQueryInfoFlags::NONE;
    repo.open(cancel)?;
    let refs = repo.list_refs(None, cancel)?;
    for (key, val) in refs {
        println!("mount ostree branch:{} id:{}", key, val);
        let f = repo.read_commit(key.as_str(), cancel)?;

        let mountpoint = "/tmp/rootfs";
        let mut options = vec![MountOption::RO, MountOption::FSName("hello".to_string())];
        options.push(MountOption::AutoUnmount);
        let mut filesystem = MyFS {
            ostree_file: f.0,
            inoMap: HashMap::new(),
            pathMap: HashMap::new(),
            inoIndex: 1,
        };
        filesystem.inoMap.insert(1, "".to_string());
        filesystem.pathMap.insert("".to_string(), 1);
        fuser::mount2(filesystem, &mountpoint, &options).unwrap();
        break;
    }
    return Ok("".to_string());
}

fn info2attr(info: &gio::FileInfo, ino: u64) -> FileAttr {
    let mut size = info.size() as u64;
    if size == 0 {
        size = 4096
    }
    return FileAttr {
        ino: ino,
        size: size,
        blksize: 512,
        blocks: 0,
        atime: info.modification_time(),
        mtime: info.modification_time(),
        ctime: info.modification_time(),
        kind: match info.file_type() {
            gio::FileType::Directory => FileType::Directory,
            _ => FileType::RegularFile,
        },
        crtime: info.modification_time(),
        perm: match info.file_type() {
            gio::FileType::Directory => 0o755,
            _ => 0o644,
        },
        nlink: 0,
        uid: 1000,
        gid: 1000,
        rdev: 0,
        flags: 0,
    };
}

fn print_dir(dir: gio::File) {
    let cancel = gio::Cancellable::NONE;
    let children = dir.enumerate_children("", gio::FileQueryInfoFlags::NONE, cancel);
    for info in children.unwrap() {
        let info = info.unwrap();
        let t = info.file_type();
        if gio::FileType::Directory == t {
            println!(
                "dir: {} {}",
                dir.path().unwrap().to_str().unwrap(),
                info.name().to_str().unwrap()
            );
            print_dir(dir.resolve_relative_path(info.name()))
        } else {
            println!(
                "file: {}/{}",
                dir.path().unwrap().to_str().unwrap(),
                info.name().to_str().unwrap()
            );
        }
    }
}
