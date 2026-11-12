use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub(crate) fn safe_relative(name: &str) -> Result<PathBuf> {
    ensure!(
        !name.contains('\\') && !name.contains(':') && !name.contains('\0'),
        "Invalid archive path"
    );
    let path = Path::new(name);
    ensure!(
        !name.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Archive path escapes destination"
    );
    Ok(path.to_owned())
}
fn destination(root: &Path, name: &str) -> Result<PathBuf> {
    let relative = safe_relative(name.trim_end_matches('/'))?;
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Archive destination contains a symlink"
            );
        }
    }
    Ok(path)
}
pub fn extract(input: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    ensure!(
        !fs::symlink_metadata(output)?.file_type().is_symlink(),
        "Archive destination is a symlink"
    );
    let mut signature = [0; 6];
    File::open(input)?.read_exact(&mut signature)?;
    if signature.starts_with(b"PK") {
        let mut zip = ZipArchive::new(File::open(input)?)?;
        // Validate the entire directory before extracting any entry.
        for i in 0..zip.len() {
            let entry = zip.by_index(i)?;
            destination(output, entry.name())?;
            ensure!(
                entry.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
                "Archive symlinks are not supported"
            );
        }
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let target = destination(output, entry.name())?;
            if entry.is_dir() {
                fs::create_dir_all(target)?;
            } else {
                fs::create_dir_all(target.parent().context("Missing parent")?)?;
                let mut file = File::create(target)?;
                std::io::copy(&mut entry, &mut file)?;
            }
        }
    } else if signature == [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c] {
        sevenz_rust::decompress_file_with_extract_fn(input, output, |entry, reader, _| {
            let target = destination(output, entry.name())
                .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
            if entry.is_directory() {
                fs::create_dir_all(target).map_err(sevenz_rust::Error::io)?;
            } else {
                fs::create_dir_all(target.parent().unwrap()).map_err(sevenz_rust::Error::io)?;
                let mut file = File::create(target).map_err(sevenz_rust::Error::io)?;
                std::io::copy(reader, &mut file).map_err(sevenz_rust::Error::io)?;
            }
            Ok(true)
        })?;
    } else {
        anyhow::bail!("Unsupported archive type");
    }
    Ok(())
}

pub fn write_zip(
    output: &Path,
    build: impl FnOnce(&mut ZipWriter<&mut File>) -> Result<()>,
) -> Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut zip = ZipWriter::new(temporary.as_file_mut());
        build(&mut zip)?;
        zip.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(output)?;
    Ok(())
}
pub fn add_bytes(
    zip: &mut ZipWriter<&mut File>,
    name: &str,
    data: &[u8],
    stored: bool,
) -> Result<()> {
    safe_relative(name)?;
    zip.start_file(
        name,
        SimpleFileOptions::default().compression_method(if stored {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        }),
    )?;
    zip.write_all(data)?;
    Ok(())
}
pub fn add_file(zip: &mut ZipWriter<&mut File>, name: &str, path: &Path) -> Result<()> {
    safe_relative(name)?;
    zip.start_file(
        name,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )?;
    std::io::copy(&mut File::open(path)?, zip)?;
    Ok(())
}
pub fn compress_folder(input: &Path, output: &Path) -> Result<()> {
    fn add(zip: &mut ZipWriter<&mut File>, root: &Path, dir: &Path) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            ensure!(!kind.is_symlink(), "Cannot archive a symlink");
            if kind.is_dir() {
                add(zip, root, &path)?;
            } else if kind.is_file() {
                add_file(
                    zip,
                    &path
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    &path,
                )?;
            }
        }
        Ok(())
    }
    let root = fs::canonicalize(input)?;
    let output_parent = output.parent().context("Missing output directory")?;
    fs::create_dir_all(output_parent)?;
    ensure!(
        !fs::canonicalize(output_parent)?.starts_with(&root),
        "Archive output must be outside its input folder"
    );
    write_zip(output, |zip| add(zip, &root, &root))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zip_roundtrip_and_traversal_rejected() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let archive = dir.path().join("test.zip");
        write_zip(&archive, |z| add_bytes(z, "章节/1.txt", b"test", false))?;
        extract(&archive, &dir.path().join("out"))?;
        assert_eq!(fs::read(dir.path().join("out/章节/1.txt"))?, b"test");
        for name in ["../escape", "/absolute", "C:/escape", "a/../../b", "a\\b"] {
            assert!(safe_relative(name).is_err());
        }
        Ok(())
    }
}
