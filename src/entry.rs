use bstr::{BStr, BString, ByteSlice};
use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use std::{
    convert::TryInto,
    ffi::{OsStr, OsString},
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{self, Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::{
    stat::{self, Mode},
    Oid, Stat,
};

#[derive(Debug, Clone)]
pub struct Entry {
    pub oid: Oid,
    pub ctime: SystemTime,
    pub mtime: SystemTime,
    pub dev: u32,
    pub ino: u32,
    pub mode: Mode,
    pub uid: u32,
    pub gid: u32,
    pub size: u32,
    pub flags: Flags,
    pub path: PathBuf,
    pub filename: BString,
}

#[derive(Debug, Clone, Copy)]
pub struct Flags {
    path_len: PathLen,
}

#[derive(Debug, Clone, Copy)]
pub enum PathLen {
    Exactly(usize),
    MaxOrGreater,
}

impl Entry {
    const BLOCK_SIZE: usize = 8;
    const PATH_OFFSET: usize = 62;

    /// # Panics
    /// If `path` contains a `..` component.
    pub fn from<P: Into<PathBuf>>(path: P, oid: Oid, stat: &Stat) -> Self {
        let path = path.into();

        if path.components().any(|c| c == path::Component::ParentDir) {
            panic!("Cannot create entry: Unnormalized path");
        }
        let filename = Self::get_filename(&path);

        Self {
            ctime: stat.ctime(),
            mtime: stat.mtime(),
            dev: stat.dev(),
            ino: stat.ino(),
            mode: stat.mode(),
            uid: stat.uid(),
            gid: stat.gid(),
            size: stat.size(),
            oid,
            flags: Flags::from_path(&path),
            path,
            filename,
        }
    }

    pub fn key(&self) -> &BStr {
        self.path().as_os_str().as_bytes().as_bstr()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Guaranteed not to include component ".."
    pub fn filename(&self) -> &BStr {
        self.filename.as_bstr()
    }

    pub fn mode(&self) -> stat::Mode {
        self.mode
    }

    #[allow(clippy::similar_names)] // unixisms
    pub fn write_to_index(&self, writer: &mut impl io::Write) -> io::Result<()> {
        let (ctime_i, ctime_n) = Self::systemtime_to_epoch(self.ctime);
        writer.write_u32::<NetworkEndian>(ctime_i)?; // offset 0
        writer.write_u32::<NetworkEndian>(ctime_n)?; // offset 4

        let (mtime_i, mtime_n) = Self::systemtime_to_epoch(self.mtime);
        writer.write_u32::<NetworkEndian>(mtime_i)?; // offset 8
        writer.write_u32::<NetworkEndian>(mtime_n)?; // offset 12

        writer.write_u32::<NetworkEndian>(self.dev)?; // offset 16
        writer.write_u32::<NetworkEndian>(self.ino)?; // offset 24
        writer.write_u32::<NetworkEndian>(self.mode.as_u32())?; // offset 28
        writer.write_u32::<NetworkEndian>(self.uid)?; // offset 32
        writer.write_u32::<NetworkEndian>(self.gid)?; // offset 36
        writer.write_u32::<NetworkEndian>(self.size)?; // offset 40

        writer.write_all(self.oid.as_bytes())?; // offset 60
        writer.write_u16::<NetworkEndian>(self.flags.as_u16())?; // offset 62

        let path = self.path().as_os_str().as_bytes();
        writer.write_all(path)?;
        for _ in 0..Self::padding_size(path) {
            writer.write_all(b"\0")?;
        }

        Ok(())
    }

    #[allow(clippy::similar_names)] // unixisms
    pub fn parse_from_index(reader: &mut impl io::Read) -> io::Result<Self> {
        let ctime_i = reader.read_u32::<NetworkEndian>()?; // offset 0
        let ctime_n = reader.read_u32::<NetworkEndian>()?; // offset 4
        let ctime = Self::systemtime_from_epoch(ctime_i, ctime_n);

        let mtime_i = reader.read_u32::<NetworkEndian>()?; // offset 8
        let mtime_n = reader.read_u32::<NetworkEndian>()?; // offset 12
        let mtime = Self::systemtime_from_epoch(mtime_i, mtime_n);

        let dev = reader.read_u32::<NetworkEndian>()?; // offset 16
        let ino = reader.read_u32::<NetworkEndian>()?; // offset 24

        let mode = reader.read_u32::<NetworkEndian>()?; // offset 28
        let mode = Mode::from_u32(mode);

        let uid = reader.read_u32::<NetworkEndian>()?; // offset 32
        let gid = reader.read_u32::<NetworkEndian>()?; // offset 36
        let size = reader.read_u32::<NetworkEndian>()?; // offset 40

        let mut oid = [0; Oid::SIZE];
        reader.read_exact(&mut oid)?; // offset 60
        let oid = Oid::new(oid);

        let flags = reader.read_u16::<NetworkEndian>()?; // offset 62
        let flags = Flags::from_u16(flags);

        let mut path = Vec::new();
        loop {
            let byte = reader.read_u8()?;
            if byte == b'\0' {
                break;
            }
            path.push(byte);
        }
        // we already read one null byte
        for _ in 0..Self::padding_size(&path) - 1 {
            reader.read_u8()?;
        }
        let path = OsString::from_vec(path);
        let path = PathBuf::from(path);

        let filename = Self::get_filename(&path);

        Ok(Self {
            oid,
            ctime,
            mtime,
            dev,
            ino,
            mode,
            uid,
            gid,
            size,
            flags,
            path,
            filename,
        })
    }

    fn padding_size(path: &[u8]) -> usize {
        let len = Self::PATH_OFFSET + path.len();
        // See <https://stackoverflow.com/a/11642218>
        (Self::BLOCK_SIZE - (len % Self::BLOCK_SIZE)) % Self::BLOCK_SIZE
    }

    fn systemtime_to_epoch(time: SystemTime) -> (u32, u32) {
        let dur = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Not before epoch");

        let secs: u32 = dur.as_secs().try_into().expect("Time overflowed");

        (secs, dur.subsec_nanos())
    }

    fn systemtime_from_epoch(secs: u32, nanos: u32) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::new(u64::from(secs), nanos)
    }

    fn get_filename(path: &Path) -> BString {
        path.file_name()
            .expect("Invalid path for index: can never end in ..")
            .as_bytes()
            .into()
    }
}

impl Flags {
    fn from_path(path: &Path) -> Self {
        Self {
            path_len: PathLen::from(path),
        }
    }

    fn from_u16(val: u16) -> Self {
        let val = val as usize;
        let path_len = if val <= PathLen::MAX {
            PathLen::Exactly(val)
        } else {
            PathLen::MaxOrGreater
        };
        Self { path_len }
    }

    fn as_u16(&self) -> u16 {
        match self.path_len {
            PathLen::Exactly(len) => len.try_into().expect("len < MAX"),
            #[allow(clippy::cast_possible_truncation)]
            PathLen::MaxOrGreater => PathLen::MAX as u16,
        }
    }
}

impl PathLen {
    pub const MAX: usize = 0xfff;

    fn from(path: &Path) -> Self {
        let path = path.as_os_str().as_bytes();
        if path.len() <= Self::MAX {
            Self::Exactly(path.len())
        } else {
            Self::MaxOrGreater
        }
    }
}