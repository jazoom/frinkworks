use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use rand::rand_core::TryRng;
use rand::rngs::SysRng;

use super::candidate::CaptureError;

pub(crate) struct WorkspaceDir {
    dir: Dir,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceKind {
    File { executable: bool },
    Symlink,
    Directory,
    Other,
}

impl WorkspaceDir {
    pub(crate) fn create_empty(path: &Path) -> Result<Self, CaptureError> {
        std::fs::create_dir_all(path).map_err(|_| CaptureError::ArtefactWrite)?;
        crate::storage::ensure_private_dir(path).map_err(|_| CaptureError::ArtefactWrite)?;
        let dir = Dir::open_ambient_dir(path, ambient_authority())
            .map_err(|_| CaptureError::ArtefactWrite)?;
        if dir
            .entries()
            .map_err(|_| CaptureError::ArtefactWrite)?
            .next()
            .is_some()
        {
            return Err(CaptureError::ArtefactWrite);
        }
        Ok(Self { dir })
    }

    pub(crate) fn open(path: &Path) -> Result<Self, CaptureError> {
        let dir = Dir::open_ambient_dir(path, ambient_authority())
            .map_err(|_| CaptureError::SourceRead)?;
        Ok(Self { dir })
    }

    pub(crate) fn kind(&self, relative: &str) -> Result<WorkspaceKind, CaptureError> {
        let meta = match self.dir.symlink_metadata(relative) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CaptureError::SourceRead);
            }
            Err(_) => return Err(CaptureError::SourceRead),
        };
        if meta.file_type().is_symlink() {
            return Ok(WorkspaceKind::Symlink);
        }
        if meta.file_type().is_file() {
            return Ok(WorkspaceKind::File {
                executable: is_executable(&meta),
            });
        }
        if meta.file_type().is_dir() {
            return Ok(WorkspaceKind::Directory);
        }
        Ok(WorkspaceKind::Other)
    }

    pub(crate) fn mode(&self, relative: &str) -> Result<u32, CaptureError> {
        let meta = self
            .dir
            .symlink_metadata(relative)
            .map_err(|_| CaptureError::SourceRead)?;
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            if meta.mode() & 0o7000 != 0 {
                return Err(CaptureError::SourceUnsupported);
            }
        }
        Ok(permission_mode(&meta))
    }

    pub(crate) fn exists(&self, relative: &str) -> bool {
        self.dir.symlink_metadata(relative).is_ok()
    }

    pub(crate) fn read_file(&self, relative: &str) -> Result<(Vec<u8>, bool, u64), CaptureError> {
        let mut options = OpenOptions::new();
        options.read(true);
        apply_nofollow(&mut options);
        let mut file = self
            .dir
            .open_with(relative, &options)
            .map_err(|_| CaptureError::SourceRead)?;
        let meta = file.metadata().map_err(|_| CaptureError::SourceRead)?;
        if !meta.file_type().is_file() {
            return Err(CaptureError::SourceUnsupported);
        }
        let executable = is_executable(&meta);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| CaptureError::SourceRead)?;
        if bytes.len() as u64 != meta.len() {
            return Err(CaptureError::SourceChanged);
        }
        Ok((bytes, executable, meta.len()))
    }

    pub(crate) fn read_link(&self, relative: &str) -> Result<String, CaptureError> {
        let target = self
            .dir
            .read_link_contents(relative)
            .map_err(|_| CaptureError::SourceRead)?;
        let text = target
            .to_str()
            .ok_or(CaptureError::SourceUnsupported)?
            .to_owned();
        if text.as_bytes().contains(&0) {
            return Err(CaptureError::SourceUnsupported);
        }
        Ok(text)
    }

    pub(crate) fn write_file(
        &self,
        relative: &str,
        bytes: &[u8],
        executable: bool,
    ) -> Result<(), CaptureError> {
        self.ensure_parents(relative)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        apply_nofollow(&mut options);
        let mut file = self
            .dir
            .open_with(relative, &options)
            .map_err(|_| CaptureError::ArtefactWrite)?;
        file.write_all(bytes)
            .map_err(|_| CaptureError::ArtefactWrite)?;
        set_executable(&file, executable)?;
        Ok(())
    }

    pub(crate) fn write_file_mode(
        &self,
        relative: &str,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), CaptureError> {
        self.write_file(relative, bytes, mode & 0o111 != 0)?;
        self.set_entry_mode(relative, mode)
    }

    pub(crate) fn create_directory_mode(
        &self,
        relative: &str,
        mode: u32,
    ) -> Result<(), CaptureError> {
        self.create_placeholder_dir(relative)?;
        self.set_entry_mode(relative, mode)
    }

    pub(crate) fn set_entry_mode(&self, relative: &str, mode: u32) -> Result<(), CaptureError> {
        #[cfg(unix)]
        {
            use cap_std::fs::{Permissions, PermissionsExt};
            self.dir
                .set_permissions(relative, Permissions::from_mode(mode & 0o777))
                .map_err(|_| CaptureError::ArtefactWrite)?;
        }
        #[cfg(not(unix))]
        let _ = (relative, mode);
        Ok(())
    }

    pub(crate) fn create_symlink(&self, relative: &str, target: &str) -> Result<(), CaptureError> {
        self.ensure_parents(relative)?;
        self.dir
            .symlink_contents(target, relative)
            .map_err(|_| CaptureError::ArtefactWrite)
    }

    pub(crate) fn replace_file(
        &self,
        relative: &str,
        bytes: &[u8],
        executable: bool,
    ) -> Result<(), CaptureError> {
        self.ensure_parents(relative)?;
        let parent = Path::new(relative).parent().unwrap_or(Path::new(""));
        let dir = if parent.as_os_str().is_empty() {
            self.dir
                .try_clone()
                .map_err(|_| CaptureError::ArtefactWrite)?
        } else {
            self.dir
                .open_dir(parent)
                .map_err(|_| CaptureError::ArtefactWrite)?
        };
        let tmp_name = temporary_name();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        apply_nofollow(&mut options);
        let mut file = dir
            .open_with(&tmp_name, &options)
            .map_err(|_| CaptureError::ArtefactWrite)?;
        file.write_all(bytes)
            .map_err(|_| CaptureError::ArtefactWrite)?;
        file.flush().map_err(|_| CaptureError::ArtefactWrite)?;
        file.sync_all().map_err(|_| CaptureError::ArtefactWrite)?;
        set_executable(&file, executable)?;
        drop(file);
        let dest = Path::new(relative)
            .file_name()
            .ok_or(CaptureError::SourceUnsupported)?;
        if let Err(error) = dir.rename(&tmp_name, &dir, dest) {
            let _ = dir.remove_file(&tmp_name);
            return Err(if error.kind() == std::io::ErrorKind::NotFound {
                CaptureError::SourceRead
            } else {
                CaptureError::ArtefactWrite
            });
        }
        Ok(())
    }

    pub(crate) fn remove_leaf(&self, relative: &str) -> Result<(), CaptureError> {
        match self.kind(relative) {
            Ok(WorkspaceKind::Directory) => self
                .dir
                .remove_dir(relative)
                .map_err(|_| CaptureError::ArtefactWrite),
            Ok(_) => self
                .dir
                .remove_file(relative)
                .map_err(|_| CaptureError::ArtefactWrite),
            Err(CaptureError::SourceRead) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn create_placeholder_dir(&self, relative: &str) -> Result<(), CaptureError> {
        self.ensure_parents(relative)?;
        self.dir
            .create_dir(relative)
            .map_err(|_| CaptureError::ArtefactWrite)
    }

    pub(crate) fn dir_is_empty(&self, relative: &str) -> Result<bool, CaptureError> {
        let nested = self
            .dir
            .open_dir(relative)
            .map_err(|_| CaptureError::SourceRead)?;
        Ok(nested
            .entries()
            .map_err(|_| CaptureError::SourceRead)?
            .next()
            .is_none())
    }

    pub(crate) fn collect_leaf_paths(&self) -> Result<Vec<String>, CaptureError> {
        self.collect_leaf_paths_excluding(&[])
    }

    pub(crate) fn collect_leaf_paths_excluding(
        &self,
        exclusions: &[String],
    ) -> Result<Vec<String>, CaptureError> {
        let mut paths = Vec::new();
        collect_leaves(&self.dir, Path::new(""), exclusions, false, &mut paths)?;
        paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(paths)
    }

    pub(crate) fn collect_paths_excluding(
        &self,
        exclusions: &[String],
    ) -> Result<Vec<String>, CaptureError> {
        let mut paths = Vec::new();
        collect_leaves(&self.dir, Path::new(""), exclusions, true, &mut paths)?;
        paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(paths)
    }

    fn ensure_parents(&self, relative: &str) -> Result<(), CaptureError> {
        let path = Path::new(relative);
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        if parent == Path::new("") {
            return Ok(());
        }
        let mut current = PathBuf::new();
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(CaptureError::SourceUnsupported);
            };
            current.push(name);
            match self.dir.symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(CaptureError::SourceUnsupported);
                }
                Ok(meta) if meta.file_type().is_dir() => {}
                Ok(_) => return Err(CaptureError::SourceUnsupported),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.dir
                        .create_dir(&current)
                        .map_err(|_| CaptureError::ArtefactWrite)?;
                }
                Err(_) => return Err(CaptureError::ArtefactWrite),
            }
        }
        Ok(())
    }
}

fn collect_leaves(
    dir: &Dir,
    prefix: &Path,
    exclusions: &[String],
    include_directories: bool,
    paths: &mut Vec<String>,
) -> Result<(), CaptureError> {
    for entry in dir.entries().map_err(|_| CaptureError::SourceRead)? {
        let entry = entry.map_err(|_| CaptureError::SourceRead)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| CaptureError::SourceUnsupported)?;
        let relative = if prefix.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            prefix.join(&name)
        };
        let relative_text = relative.to_str().ok_or(CaptureError::SourceUnsupported)?;
        if exclusions.iter().any(|excluded| excluded == relative_text) {
            continue;
        }
        if relative_text.len() > super::candidate::MAXIMUM_PATH_BYTES
            || paths.len() >= super::candidate::MAXIMUM_ENTRIES
        {
            return Err(CaptureError::SourceTooLarge);
        }
        let file_type = entry.file_type().map_err(|_| CaptureError::SourceRead)?;
        if file_type.is_symlink() || file_type.is_file() {
            paths.push(relative.to_string_lossy().into_owned());
        } else if file_type.is_dir() {
            let nested = dir.open_dir(&name).map_err(|_| CaptureError::SourceRead)?;
            let mut children = nested.entries().map_err(|_| CaptureError::SourceRead)?;
            let empty = children.next().is_none();
            if empty || include_directories {
                paths.push(relative.to_string_lossy().into_owned());
            }
            if !empty {
                collect_leaves(&nested, &relative, exclusions, include_directories, paths)?;
            }
        } else {
            return Err(CaptureError::SourceUnsupported);
        }
    }
    Ok(())
}

fn temporary_name() -> String {
    let mut bytes = [0u8; 8];
    let _ = SysRng.try_fill_bytes(&mut bytes);
    let mut name = String::from(".frinkworks-");
    for byte in bytes {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        name.push(HEX[(byte >> 4) as usize] as char);
        name.push(HEX[(byte & 0x0f) as usize] as char);
    }
    name.push_str(".tmp");
    name
}

fn apply_nofollow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(0o400000);
    }
}

fn permission_mode(meta: &cap_std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        meta.mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0o644
    }
}

fn is_executable(meta: &cap_std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        permission_mode(meta) & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

fn set_executable(file: &cap_std::fs::File, executable: bool) -> Result<(), CaptureError> {
    #[cfg(unix)]
    {
        use cap_std::fs::{Permissions, PermissionsExt};
        let mode = if executable { 0o755 } else { 0o644 };
        let permissions = Permissions::from_mode(mode);
        file.set_permissions(permissions)
            .map_err(|_| CaptureError::ArtefactWrite)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, executable);
    }
    Ok(())
}

pub(crate) fn split_relative(path: &str) -> Result<(), CaptureError> {
    if path.is_empty() || path.starts_with('/') || path.contains('\0') {
        return Err(CaptureError::SourceUnsupported);
    }
    if path == ".git" || path.starts_with(".git/") {
        return Err(CaptureError::SourceUnsupported);
    }
    let mut components = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str().ok_or(CaptureError::SourceUnsupported)?;
                if name == ".git" {
                    return Err(CaptureError::SourceUnsupported);
                }
                components.push(name);
            }
            _ => return Err(CaptureError::SourceUnsupported),
        }
    }
    if components.is_empty() {
        return Err(CaptureError::SourceUnsupported);
    }
    Ok(())
}
