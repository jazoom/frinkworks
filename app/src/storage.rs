use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rand::rand_core::TryRng;
use rand::rngs::SysRng;

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug)]
pub(crate) struct PersistError;

impl std::fmt::Display for PersistError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("private file persist failed")
    }
}

impl std::error::Error for PersistError {}

/// With `deserialize_with`, this rejects an absent field but accepts `null`.
pub(crate) fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), PersistError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PersistError);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| PersistError)?;
        }
        Err(_) => return Err(PersistError),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| PersistError)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PersistError);
    }
    restrict_dir_permissions(path).map_err(|_| PersistError)
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    write_private_outcome(path, bytes).map_err(|_| PersistError)
}

pub(crate) enum PrivateWriteError {
    Unchanged,
    Replaced,
}

pub(crate) fn write_private_outcome(path: &Path, bytes: &[u8]) -> Result<(), PrivateWriteError> {
    let dir = path.parent().ok_or(PrivateWriteError::Unchanged)?;
    let (tmp, mut file) =
        create_unique_file(dir, ".tmp").map_err(|_| PrivateWriteError::Unchanged)?;
    let mut replaced = false;
    let result: io::Result<()> = (|| {
        restrict_file_permissions(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        replaced = true;
        sync_dir(dir)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            Err(if replaced {
                PrivateWriteError::Replaced
            } else {
                PrivateWriteError::Unchanged
            })
        }
    }
}

pub(crate) fn create_unique_private(dir: &Path, bytes: &[u8]) -> Result<PathBuf, PersistError> {
    let (path, mut file) = create_unique_file(dir, ".staging").map_err(|_| PersistError)?;
    let result: io::Result<()> = (|| {
        restrict_file_permissions(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        sync_dir(dir)
    })();
    match result {
        Ok(()) => Ok(path),
        Err(_) => {
            let _ = fs::remove_file(&path);
            Err(PersistError)
        }
    }
}

pub(crate) fn rename_in_dir(from: &Path, to: &Path) -> Result<(), PersistError> {
    let from_dir = from.parent().ok_or(PersistError)?;
    let to_dir = to.parent().ok_or(PersistError)?;
    if from_dir != to_dir {
        return Err(PersistError);
    }
    restrict_private_file(from)?;
    match fs::symlink_metadata(to) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(PersistError),
        Ok(meta) if meta.file_type().is_symlink() => return Err(PersistError),
        Ok(_) => {}
    }
    fs::rename(from, to).map_err(|_| PersistError)?;
    sync_dir(from_dir).map_err(|_| PersistError)
}

pub(crate) fn remove_private(path: &Path) -> Result<(), PersistError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PersistError),
        Ok(meta) if meta.file_type().is_symlink() => return Err(PersistError),
        Ok(_) => {}
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PersistError),
    }
    if let Some(dir) = path.parent() {
        sync_dir(dir).map_err(|_| PersistError)?;
    }
    Ok(())
}

fn create_unique_file(dir: &Path, suffix: &str) -> io::Result<(PathBuf, File)> {
    for _ in 0..16 {
        let mut bytes = [0u8; 8];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| io::Error::other("system random source unavailable"))?;
        let mut name = String::from(".");
        for byte in bytes {
            name.push(HEX[(byte >> 4) as usize] as char);
            name.push(HEX[(byte & 0x0f) as usize] as char);
        }
        name.push_str(suffix);
        let path = dir.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "private temporary file collision",
    ))
}

pub(crate) fn read_private(path: &Path) -> Result<Vec<u8>, PersistError> {
    let mut file = open_private_file(path)?;
    restrict_file_permissions(&file).map_err(|_| PersistError)?;
    file.sync_all().map_err(|_| PersistError)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| PersistError)?;
    Ok(bytes)
}

pub(crate) fn read_private_bounded(
    path: &Path,
    maximum_bytes: usize,
) -> Result<Vec<u8>, PersistError> {
    let file = open_private_file(path)?;
    restrict_file_permissions(&file).map_err(|_| PersistError)?;
    file.sync_all().map_err(|_| PersistError)?;
    let maximum = u64::try_from(maximum_bytes).map_err(|_| PersistError)?;
    if file.metadata().map_err(|_| PersistError)?.len() > maximum {
        return Err(PersistError);
    }
    let read_limit = maximum.checked_add(1).ok_or(PersistError)?;
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| PersistError)?;
    if bytes.len() > maximum_bytes {
        return Err(PersistError);
    }
    Ok(bytes)
}

pub(crate) fn restrict_private_file(path: &Path) -> Result<(), PersistError> {
    let file = open_private_file(path)?;
    restrict_file_permissions(&file).map_err(|_| PersistError)?;
    file.sync_all().map_err(|_| PersistError)
}

fn open_private_file(path: &Path) -> Result<File, PersistError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PersistError)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PersistError);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOFOLLOW: i32 = 0o400000;
        options.custom_flags(O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| PersistError)?;
    if !file.metadata().map_err(|_| PersistError)?.is_file() {
        return Err(PersistError);
    }
    Ok(file)
}

pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

fn restrict_file_permissions(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

pub(crate) const LOG_LIMIT_BYTES: u64 = 1024 * 1024;
const LOG_PREFIX_BYTES: usize = 256 * 1024;
const LOG_SUFFIX_BYTES: usize = 256 * 1024;
pub(crate) const LOG_TRUNCATION_MARKER: &[u8] = b"\n--- log truncated ---\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LogState {
    pub(crate) captured_bytes: u64,
    pub(crate) truncated: bool,
}

pub(crate) fn confined_child(root: &Path, name: &str) -> Result<PathBuf, PersistError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(PersistError);
    }
    if name
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'/' | b'\\' | 0))
    {
        return Err(PersistError);
    }
    let path = root.join(name);
    if path.parent() != Some(root) {
        return Err(PersistError);
    }
    Ok(path)
}

pub(crate) fn remove_tree_nofollow(path: &Path) -> Result<(), PersistError> {
    use cap_std::ambient_authority;
    use cap_std::fs::Dir;

    let parent = path.parent().ok_or(PersistError)?;
    let name = path.file_name().ok_or(PersistError)?;
    let parent = Dir::open_ambient_dir(parent, ambient_authority()).map_err(|_| PersistError)?;
    let Some(entry) = parent
        .entries()
        .map_err(|_| PersistError)?
        .find_map(|entry| {
            let entry = entry.ok()?;
            (entry.file_name() == name).then_some(entry)
        })
    else {
        return Ok(());
    };
    remove_entry_nofollow(entry)
}

fn remove_entry_nofollow(entry: cap_std::fs::DirEntry) -> Result<(), PersistError> {
    match open_entry_dir_nofollow(&entry) {
        Ok(dir) => {
            let entries: Vec<_> = dir
                .entries()
                .map_err(|_| PersistError)?
                .collect::<Result<_, _>>()
                .map_err(|_| PersistError)?;
            for child in entries {
                remove_entry_nofollow(child)?;
            }
            dir.remove_open_dir().map_err(|_| PersistError)
        }
        Err(_) => match entry.remove_file() {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(PersistError),
        },
    }
}

fn open_entry_dir_nofollow(entry: &cap_std::fs::DirEntry) -> io::Result<cap_std::fs::Dir> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        const O_DIRECTORY: i32 = 0o200000;
        const O_NOFOLLOW: i32 = 0o400000;
        options.custom_flags(O_DIRECTORY | O_NOFOLLOW);
    }
    let file = entry.open_with(&options)?;
    Ok(cap_std::fs::Dir::from_std_file(file.into_std()))
}

pub(crate) fn create_private_file(path: &Path) -> Result<(), PersistError> {
    let dir = path.parent().ok_or(PersistError)?;
    ensure_private_dir(dir)?;
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => restrict_file_permissions(&file).map_err(|_| PersistError),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(PersistError),
    }
}

pub(crate) struct BoundedLogger {
    path: PathBuf,
    captured_bytes: u64,
    truncated: bool,
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

impl BoundedLogger {
    pub(crate) fn create(path: PathBuf) -> Result<Self, PersistError> {
        create_private_file(&path)?;
        Ok(Self {
            path,
            captured_bytes: 0,
            truncated: false,
            prefix: Vec::new(),
            suffix: Vec::new(),
        })
    }

    pub(crate) fn state(&self) -> LogState {
        LogState {
            captured_bytes: self.captured_bytes,
            truncated: self.truncated,
        }
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<LogState, PersistError> {
        if bytes.is_empty() {
            return Ok(self.state());
        }
        let new_total = self
            .captured_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(PersistError)?;
        if !self.truncated && new_total <= LOG_LIMIT_BYTES {
            append_private(&self.path, bytes)?;
            if self.prefix.len() < LOG_PREFIX_BYTES {
                let take = (LOG_PREFIX_BYTES - self.prefix.len()).min(bytes.len());
                self.prefix.extend_from_slice(&bytes[..take]);
            }
            self.suffix.extend_from_slice(bytes);
            if self.suffix.len() > LOG_SUFFIX_BYTES {
                let extra = self.suffix.len() - LOG_SUFFIX_BYTES;
                self.suffix.drain(..extra);
            }
            self.captured_bytes = new_total;
            return Ok(self.state());
        }
        if !self.truncated {
            if self.prefix.is_empty() {
                self.prefix = fs::read(&self.path).map_err(|_| PersistError)?;
                if self.prefix.len() > LOG_PREFIX_BYTES {
                    self.prefix.truncate(LOG_PREFIX_BYTES);
                }
            }
            self.suffix.extend_from_slice(bytes);
            if self.suffix.len() > LOG_SUFFIX_BYTES {
                let extra = self.suffix.len() - LOG_SUFFIX_BYTES;
                self.suffix.drain(..extra);
            }
            self.truncated = true;
            self.captured_bytes = new_total;
            self.rewrite_truncated()?;
            return Ok(self.state());
        }
        self.suffix.extend_from_slice(bytes);
        if self.suffix.len() > LOG_SUFFIX_BYTES {
            let extra = self.suffix.len() - LOG_SUFFIX_BYTES;
            self.suffix.drain(..extra);
        }
        self.captured_bytes = new_total;
        self.rewrite_truncated()?;
        Ok(self.state())
    }

    fn rewrite_truncated(&self) -> Result<(), PersistError> {
        let mut bytes =
            Vec::with_capacity(self.prefix.len() + LOG_TRUNCATION_MARKER.len() + self.suffix.len());
        bytes.extend_from_slice(&self.prefix);
        bytes.extend_from_slice(LOG_TRUNCATION_MARKER);
        bytes.extend_from_slice(&self.suffix);
        write_private(&self.path, &bytes)
    }
}

fn append_private(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|_| PersistError)?;
    restrict_file_permissions(&file).map_err(|_| PersistError)?;
    file.write_all(bytes).map_err(|_| PersistError)?;
    file.sync_all().map_err(|_| PersistError)
}

fn restrict_dir_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
