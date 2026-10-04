use std::{
    fs::{self, File, FileTimes},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[path = "../build/source.rs"]
mod source;

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let out = temp.path().join("out");
    write_file(&source.join("configure.ac"), "AC_INIT([flint], [1.0])\n");
    write_file(&source.join("src/input.c"), "int value = 1;\n");
    fs::create_dir_all(&out).unwrap();
    (temp, source, out)
}

fn set_modified(path: &Path, modified: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn unchanged_content_preserves_generated_files_and_timestamps() {
    let (_temp, upstream, out) = fixture();
    write_file(&upstream.join(".git"), "gitdir: first\n");
    let copied = source::prepare(&upstream, &out).unwrap();
    assert_eq!(copied, out.join("flint"));
    assert!(!copied.join(".git").exists());

    write_file(&copied.join("configure"), "generated configure\n");
    write_file(&copied.join("src/input.o"), "compiled object\n");
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
    let preserved = [
        ("src/input.c", "int value = 1;\n"),
        ("configure", "generated configure\n"),
        ("src/input.o", "compiled object\n"),
    ];
    for (name, _) in preserved {
        set_modified(&copied.join(name), modified);
    }

    // Changes to checkout metadata and source mtimes do not change the contents.
    write_file(&upstream.join(".git"), "gitdir: second\n");
    write_file(&upstream.join("src/input.c"), "int value = 1;\n");
    source::prepare(&upstream, &out).unwrap();

    for (name, contents) in preserved {
        let path = copied.join(name);
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn removed_and_renamed_sources_clear_only_flint_outputs() {
    let (_temp, upstream, out) = fixture();
    write_file(&upstream.join("src/removed.c"), "removed\n");
    write_file(&upstream.join("src/old_name.c"), "renamed\n");
    let copied = source::prepare(&upstream, &out).unwrap();
    let stale_outputs = [
        "flint/configure",
        "flint/src/old_name.o",
        "include/flint/obsolete.h",
        "lib/libflint.a",
        "lib/pkgconfig/flint.pc",
        "flint-configure-command",
    ];
    for path in stale_outputs {
        write_file(&out.join(path), "stale\n");
    }
    let unrelated_outputs = ["include/gmp.h", "lib/libgmp.a", "lib/pkgconfig/gmp.pc"];
    for path in unrelated_outputs {
        write_file(&out.join(path), "keep\n");
    }

    fs::remove_file(upstream.join("src/removed.c")).unwrap();
    fs::rename(
        upstream.join("src/old_name.c"),
        upstream.join("src/new_name.c"),
    )
    .unwrap();
    source::prepare(&upstream, &out).unwrap();

    assert!(!copied.join("src/removed.c").exists());
    assert!(!copied.join("src/old_name.c").exists());
    assert_eq!(
        fs::read_to_string(copied.join("src/new_name.c")).unwrap(),
        "renamed\n"
    );
    for path in stale_outputs {
        assert!(!out.join(path).exists(), "stale output remains: {path}");
    }
    for path in unrelated_outputs {
        assert_eq!(fs::read_to_string(out.join(path)).unwrap(), "keep\n");
    }
}

#[test]
fn content_and_file_kind_changes_replace_the_copy() {
    let (_temp, upstream, out) = fixture();
    write_file(&upstream.join("shape"), "file\n");
    let copied = source::prepare(&upstream, &out).unwrap();
    write_file(&copied.join("src/input.o"), "compiled object\n");

    // Equal size and mtime must not hide a source change.
    let input = upstream.join("src/input.c");
    let modified = fs::metadata(&input).unwrap().modified().unwrap();
    write_file(&input, "int value = 2;\n");
    set_modified(&input, modified);
    source::prepare(&upstream, &out).unwrap();
    assert_eq!(
        fs::read_to_string(copied.join("src/input.c")).unwrap(),
        "int value = 2;\n"
    );
    assert!(!copied.join("src/input.o").exists());

    fs::remove_file(upstream.join("shape")).unwrap();
    write_file(&upstream.join("shape/nested"), "directory\n");
    source::prepare(&upstream, &out).unwrap();
    assert_eq!(
        fs::read_to_string(copied.join("shape/nested")).unwrap(),
        "directory\n"
    );

    fs::remove_dir_all(upstream.join("shape")).unwrap();
    write_file(&upstream.join("shape"), "file again\n");
    source::prepare(&upstream, &out).unwrap();
    assert_eq!(
        fs::read_to_string(copied.join("shape")).unwrap(),
        "file again\n"
    );
}

#[test]
fn legacy_and_interrupted_copies_are_repaired() {
    let (_temp, upstream, out) = fixture();
    write_file(&out.join("flint/src/legacy.c"), "legacy\n");
    write_file(&out.join("include/flint/legacy.h"), "legacy\n");
    let copied = source::prepare(&upstream, &out).unwrap();
    let stamp = out.join("flint-source-stamp");
    assert!(stamp.is_file());
    assert!(!copied.join("src/legacy.c").exists());
    assert!(!out.join("include/flint/legacy.h").exists());

    // An interrupted copy has no completed stamp and may have partial outputs.
    fs::remove_file(&stamp).unwrap();
    write_file(&copied.join("src/input.c"), "partial\n");
    write_file(&copied.join("configure"), "stale\n");
    write_file(&out.join("lib/libflint.a"), "stale\n");
    source::prepare(&upstream, &out).unwrap();
    assert!(stamp.is_file());
    assert_eq!(
        fs::read_to_string(copied.join("src/input.c")).unwrap(),
        "int value = 1;\n"
    );
    assert!(!copied.join("configure").exists());
    assert!(!out.join("lib/libflint.a").exists());

    // A surviving stamp cannot make a missing working tree up to date.
    fs::remove_dir_all(&copied).unwrap();
    source::prepare(&upstream, &out).unwrap();
    assert_eq!(
        fs::read_to_string(copied.join("src/input.c")).unwrap(),
        "int value = 1;\n"
    );
}
