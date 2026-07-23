#[cfg(not(unix))]
use std::{fs::File, path::Path};

#[cfg(not(unix))]
use anyhow::{Result, bail};

#[cfg(unix)]
mod unix {
    use std::{
        ffi::{CString, OsStr},
        fs::{self, File},
        io,
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
            unix::fs::MetadataExt,
        },
        path::{Component, Path},
    };

    use anyhow::{Context, Result, bail, ensure};

    use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;

    pub(crate) struct CatalogDirectory {
        directory: File,
        sync_directory: fn(&File) -> io::Result<()>,
    }

    struct ParentDirectory {
        directory: File,
        basename: CString,
        display: String,
    }

    impl CatalogDirectory {
        pub(crate) fn open(path: &Path) -> Result<Self> {
            ensure!(
                path.is_absolute(),
                "decision-evidence catalog_directory must be absolute"
            );
            let resolved = fs::canonicalize(path).with_context(|| {
                format!(
                    "resolve decision-evidence catalog_directory `{}`",
                    path.display()
                )
            })?;
            let mut directory = open_root_directory()?;
            for component in resolved.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(name) => {
                        directory = open_directory_at(&directory, name).with_context(|| {
                            format!(
                                "open decision-evidence catalog component `{}` without symlinks",
                                name.to_string_lossy()
                            )
                        })?;
                    }
                    Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                        bail!("decision-evidence catalog_directory is not normalized")
                    }
                }
            }
            Ok(Self {
                directory,
                sync_directory: sync_directory_entry,
            })
        }

        pub(crate) fn open_stream(&self, relative: &str) -> Result<File> {
            let Some(parent) = self.walk_parent(relative, false)? else {
                bail!("active decision-evidence stream parent is absent: `{relative}`")
            };
            parent.open_stream(self.sync_directory)
        }

        pub(crate) fn ensure_retired_absent(&self, relative: &str) -> Result<()> {
            let Some(parent) = self.walk_parent(relative, true)? else {
                return Ok(());
            };
            parent.ensure_absent()
        }

        fn walk_parent(
            &self,
            relative: &str,
            missing_parent_means_absent: bool,
        ) -> Result<Option<ParentDirectory>> {
            let path = Path::new(relative.trim());
            let mut components = path.components().peekable();
            let mut directory = self.directory.try_clone()?;
            while let Some(component) = components.next() {
                let Component::Normal(name) = component else {
                    bail!("decision-evidence relative path is not normalized")
                };
                if components.peek().is_none() {
                    return Ok(Some(ParentDirectory {
                        directory,
                        basename: component_name(name)?,
                        display: relative.trim().to_string(),
                    }));
                }
                match open_directory_at(&directory, name) {
                    Ok(next) => directory = next,
                    Err(error)
                        if missing_parent_means_absent
                            && error.kind() == io::ErrorKind::NotFound =>
                    {
                        return Ok(None);
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "open decision-evidence path component `{}` without symlinks",
                                name.to_string_lossy()
                            )
                        });
                    }
                }
            }
            bail!("decision-evidence relative path has no file name")
        }
    }

    impl ParentDirectory {
        fn open_stream(self, sync_directory: fn(&File) -> io::Result<()>) -> Result<File> {
            let create_flags = libc::O_RDWR
                | libc::O_APPEND
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW;
            // SAFETY: the parent descriptor and NUL-terminated basename remain valid for the call.
            let created_raw = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    self.basename.as_ptr(),
                    create_flags,
                    PRIVATE_ARTIFACT_FILE_MODE,
                )
            };
            let (raw, created) = if created_raw >= 0 {
                (created_raw, true)
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EEXIST) {
                    let existing_flags =
                        libc::O_RDWR | libc::O_APPEND | libc::O_CLOEXEC | libc::O_NOFOLLOW;
                    // SAFETY: the parent descriptor and NUL-terminated basename remain valid.
                    let existing_raw = unsafe {
                        libc::openat(
                            self.directory.as_raw_fd(),
                            self.basename.as_ptr(),
                            existing_flags,
                        )
                    };
                    if existing_raw >= 0 {
                        (existing_raw, false)
                    } else {
                        return self.open_error(io::Error::last_os_error());
                    }
                } else {
                    return self.open_error(error);
                }
            };
            // SAFETY: `openat` returned a new owned descriptor on success.
            let file = unsafe { File::from_raw_fd(raw) };
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file(),
                "decision-evidence path is not a regular file: `{}`",
                self.display
            );
            ensure!(
                metadata.nlink() == 1,
                "decision-evidence path has external hard-link aliases: `{}`",
                self.display
            );
            if created {
                sync_directory(&self.directory).with_context(|| {
                    format!(
                        "synchronize newly created decision-evidence stream namespace `{}`",
                        self.display
                    )
                })?;
            }
            Ok(file)
        }

        fn open_error<T>(&self, error: io::Error) -> Result<T> {
            if matches!(error.raw_os_error(), Some(libc::ELOOP)) {
                bail!(
                    "decision-evidence path is not a regular file: symlink rejected at `{}`",
                    self.display
                );
            }
            if matches!(error.raw_os_error(), Some(libc::EISDIR)) {
                bail!(
                    "decision-evidence path is not a regular file: `{}`",
                    self.display
                );
            }
            Err(error).with_context(|| {
                format!(
                    "open decision-evidence stream `{}` without symlinks",
                    self.display
                )
            })
        }

        fn ensure_absent(self) -> Result<()> {
            let mut metadata = MaybeUninit::<libc::stat>::uninit();
            // SAFETY: all pointers are valid for the call and `metadata` is only read on success.
            let result = unsafe {
                libc::fstatat(
                    self.directory.as_raw_fd(),
                    self.basename.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                bail!(
                    "retired decision-evidence path is present: `{}`",
                    self.display
                );
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(());
            }
            Err(error).with_context(|| {
                format!(
                    "inspect retired decision-evidence path `{}` without symlinks",
                    self.display
                )
            })
        }
    }

    fn open_root_directory() -> Result<File> {
        let root = CString::new("/").expect("root path has no NUL");
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: `root` is a valid NUL-terminated path for the duration of the call.
        let raw = unsafe { libc::open(root.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error())
                .context("open filesystem root for decision-evidence catalog");
        }
        // SAFETY: `open` returned a new owned descriptor on success.
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    fn sync_directory_entry(directory: &File) -> io::Result<()> {
        directory.sync_all()
    }

    fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
        let name = component_name(name)?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: the parent descriptor and NUL-terminated component remain valid for the call.
        let raw = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new owned descriptor on success.
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    fn component_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "decision-evidence path component contains NUL",
            )
        })
    }

    #[cfg(test)]
    mod tests {
        use std::{fs, io::Write, os::unix::fs::symlink};

        use super::*;

        #[test]
        fn retained_parent_descriptor_cannot_be_redirected_by_path_replacement() {
            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let original = canonical_root.join("original");
            let retained = canonical_root.join("retained");
            let outside = tempfile::tempdir().expect("outside tempdir must exist");
            fs::create_dir(&original).expect("original parent must exist");
            let catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");
            let parent = catalog
                .walk_parent("original/machine.jsonl", false)
                .expect("parent walk must succeed")
                .expect("parent must exist");

            fs::rename(&original, &retained).expect("original parent must be retained");
            symlink(outside.path(), &original).expect("old pathname must be redirected");
            let mut stream = parent
                .open_stream(sync_directory_entry)
                .expect("descriptor-relative open must succeed");
            stream.write_all(b"retained\n").expect("write must succeed");
            stream.sync_data().expect("write must sync");

            assert_eq!(
                fs::read(retained.join("machine.jsonl")).expect("retained file must read"),
                b"retained\n"
            );
            assert!(!outside.path().join("machine.jsonl").exists());
        }

        #[test]
        fn newly_created_stream_requires_parent_directory_sync() {
            fn reject_sync(_directory: &File) -> io::Result<()> {
                Err(io::Error::other("injected directory sync failure"))
            }

            let root = tempfile::tempdir().expect("tempdir must exist");
            let mut catalog = CatalogDirectory::open(root.path()).expect("catalog must open");
            catalog.sync_directory = reject_sync;

            let error = catalog
                .open_stream("machine.jsonl")
                .expect_err("creation must fail when namespace sync fails");

            assert!(
                error
                    .to_string()
                    .contains("synchronize newly created decision-evidence stream namespace")
            );
        }

        #[test]
        fn retained_stream_does_not_claim_a_new_namespace_commit() {
            fn reject_sync(_directory: &File) -> io::Result<()> {
                Err(io::Error::other("existing stream must not sync its parent"))
            }

            let root = tempfile::tempdir().expect("tempdir must exist");
            fs::write(root.path().join("machine.jsonl"), b"retained\n")
                .expect("retained stream must exist");
            let mut catalog = CatalogDirectory::open(root.path()).expect("catalog must open");
            catalog.sync_directory = reject_sync;

            let stream = catalog
                .open_stream("machine.jsonl")
                .expect("opening an existing name does not create namespace state");

            assert_eq!(
                stream.metadata().expect("stream metadata must read").len(),
                b"retained\n".len() as u64
            );
        }
    }
}

#[cfg(unix)]
pub(super) use unix::CatalogDirectory;

#[cfg(not(unix))]
pub(super) struct CatalogDirectory;

#[cfg(not(unix))]
impl CatalogDirectory {
    pub(super) fn open(_path: &Path) -> Result<Self> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn open_stream(&self, _relative: &str) -> Result<File> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn ensure_retired_absent(&self, _relative: &str) -> Result<()> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }
}
