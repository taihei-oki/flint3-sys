use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

// The copied source tree also contains generated configure files and objects.
// Recreate it on source changes so deleted/renamed inputs cannot survive there.
pub(super) fn prepare(source: &Path, out_dir: &Path) -> io::Result<PathBuf> {
    let destination = out_dir.join("flint");
    let stamp = out_dir.join("flint-source-stamp");
    let fingerprint = fingerprint(source)?;
    if destination.is_dir() && fs::read_to_string(&stamp).ok().as_deref() == Some(&fingerprint) {
        return Ok(destination);
    }

    // Invalidate first: a failed copy must be retried even if the sources later
    // revert to the version recorded by the previous stamp.
    remove_if_present(&stamp)?;
    remove_if_present(&destination)?;
    // make install overwrites headers but does not remove obsolete ones.
    for path in [
        "include/flint",
        "lib/libflint.a",
        "lib/pkgconfig/flint.pc",
        "flint-configure-command",
    ] {
        remove_if_present(&out_dir.join(path))?;
    }
    copy_source(source, &destination)?;
    fs::write(stamp, fingerprint)?;
    Ok(destination)
}

fn entries(directory: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(directory)?.collect::<io::Result<Vec<_>>>()?;
    entries.retain(|entry| entry.file_name() != ".git");
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn fingerprint(source: &Path) -> io::Result<String> {
    fn hash_tree(directory: &Path, hasher: &mut DefaultHasher) -> io::Result<()> {
        let entries = entries(directory)?;
        entries.len().hash(hasher);
        for entry in entries {
            entry.file_name().hash(hasher);
            let is_dir = entry.file_type()?.is_dir();
            is_dir.hash(hasher);
            if is_dir {
                hash_tree(&entry.path(), hasher)?;
            } else {
                fs::read(entry.path())?.hash(hasher);
            }
        }
        Ok(())
    }

    // This is only a local rebuild key; a Rust version changing the hash
    // algorithm simply triggers a fresh build.
    let mut hasher = DefaultHasher::new();
    hash_tree(source, &mut hasher)?;
    Ok(format!("{:016x}", hasher.finish()))
}

fn copy_source(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in entries(source)? {
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_source(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
