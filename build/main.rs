use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

mod source;

#[cfg(feature = "run-bindgen")]
mod runbindgen;

#[cfg(feature = "use-system-libs")]
mod system;

#[cfg(target_env = "msvc")]
mod msvc;

const CONFIGURE_ENV: &[&str] = &["CC", "AR", "CFLAGS", "CPPFLAGS", "LDFLAGS"];

// Build or locate FLINT, then provide `flint.rs` to the crate.
//
// Normal builds copy the checked-in bindings from `bindgen/flint.rs`.
// `--features run-bindgen` regenerates that file by running bindgen once per
// FLINT header. Per-header bindgen is much faster than a single mega-header, but
// it requires a few explicit policies below to avoid duplicate declarations.

#[cfg(not(feature = "run-bindgen"))]
const GENERATED_BINDINGS: &str = "bindgen/flint.rs";

fn run(mut command: Command) -> Result<()> {
    let command_string = format!("{command:?}");

    let output = command
        .output()
        .with_context(|| format!("Command {command_string} did not execute normally"))?;

    if !output.status.success() {
        anyhow::bail!(
            "Command failed\nCommand: {}\n===== stdout\n{}===== stderr\n{}\n",
            command_string,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    }

    Ok(())
}

fn build_command(program: &str, root: &Path, tmp_dir: &str) -> Result<Command> {
    let mut command = Command::new(program);
    command.current_dir(root).env("TMPDIR", tmp_dir);

    // MSYS2 converts ACLOCAL_PATH to Windows syntax when launching native Cargo.
    // Convert it back for Autotools, preserving custom macro search directories.
    #[cfg(windows)]
    if let Some(paths) = std::env::var_os("ACLOCAL_PATH") {
        let paths = paths.to_str().context("Non-Unicode ACLOCAL_PATH")?;
        let bytes = paths.as_bytes();
        let has_drive_letter =
            bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        // Leave empty and already-POSIX path lists alone.
        if paths.contains([';', '\\']) || has_drive_letter {
            let output = Command::new("cygpath")
                .args(["--unix", "--path", "--", paths])
                .output()
                .context("Could not run cygpath to convert ACLOCAL_PATH")?;
            anyhow::ensure!(
                output.status.success(),
                "cygpath failed for ACLOCAL_PATH: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let paths = String::from_utf8(output.stdout)?;
            command.env("ACLOCAL_PATH", paths.trim_end_matches(['\r', '\n']));
        }
    }

    Ok(command)
}

enum MakeAction {
    Build,
    Clean,
    Install,
}

fn make(root: &Path, tmp_dir: &str, action: MakeAction) -> Result<()> {
    let mut command = build_command("make", root, tmp_dir)?;
    if cfg!(windows) {
        // Cargo's Windows semaphore jobserver is incompatible with MSYS make.
        command.env_remove("MAKEFLAGS");
    }
    match action {
        MakeAction::Build if cfg!(windows) => {
            command.arg(format!("-j{}", std::env::var("NUM_JOBS")?));
        }
        MakeAction::Build => {
            if let Ok(flags) = std::env::var("CARGO_MAKEFLAGS") {
                command.env("MAKEFLAGS", flags);
            }
        }
        MakeAction::Clean => {
            command.arg("clean").env_remove("MAKEFLAGS");
        }
        MakeAction::Install => {
            command.arg("install");
        }
    }
    run(command)
}

fn mingw_library_dir(compiler: &cc::Tool, library: &str) -> Result<PathBuf> {
    for suffix in ["dll.a", "a"] {
        let output = compiler
            .to_command()
            .arg(format!("-print-file-name=lib{library}.{suffix}"))
            .output()
            .context("Failed to query the MinGW compiler's library search path")?;
        let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        if output.status.success() && path.is_file() {
            return path
                .parent()
                .map(Path::to_path_buf)
                .context("MinGW library path has no parent directory");
        }
    }
    anyhow::bail!(
        "Cannot find MinGW library {library}; install the MSYS2 MinGW 64-bit dependencies"
    )
}

// Rust's canonical Windows paths start with `\\?\`, which MSYS tools do not
// understand. Keep native paths for Cargo/Rust, and translate only arguments
// passed to the POSIX build tools.
fn shell_path(path: &Path) -> Result<String> {
    #[cfg(windows)]
    {
        let path = path.to_str().context("Non-Unicode build path")?;
        let path = path.strip_prefix(r"\\?\").unwrap_or(path);
        let output = Command::new("cygpath")
            .args(["-u", path])
            .output()
            .context("Could not run cygpath; build from an MSYS2 MinGW 64-bit terminal")?;
        anyhow::ensure!(output.status.success(), "cygpath failed for `{path}`");
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_str().context("Non-Unicode build path")?.to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Other,
    WindowsGnu,
    WindowsMsvc,
}

impl Target {
    fn from_env() -> Result<Self> {
        let target_triple = std::env::var("TARGET").context("Missing TARGET")?;
        let target = match target_triple.as_str() {
            "x86_64-pc-windows-gnu" => Self::WindowsGnu,
            "x86_64-pc-windows-msvc" => Self::WindowsMsvc,
            _ => Self::Other,
        };
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            anyhow::ensure!(
                target != Self::Other,
                "Windows builds require x86_64-pc-windows-gnu or x86_64-pc-windows-msvc; \
                 32-bit and ARM Windows are not supported"
            );
            anyhow::ensure!(
                cfg!(windows),
                "Cross-compiling FLINT to Windows is not supported; build on Windows"
            );
            anyhow::ensure!(
                std::env::var("HOST").context("Missing HOST")? == target_triple,
                "Windows builds require matching Rust host and target toolchains"
            );
        }
        if target == Self::WindowsMsvc {
            anyhow::ensure!(
                cfg!(feature = "run-bindgen"),
                "MSVC requires `--features run-bindgen` and vcpkg dependencies; \
                 add `use-system-libs` to use vcpkg's FLINT; see README.md"
            );
            anyhow::ensure!(
                !cfg!(feature = "gmp-mpfr-sys"),
                "gmp-mpfr-sys does not support MSVC; use the GMP/MPFR libraries provided by vcpkg"
            );
        }
        Ok(target)
    }
}

// Shared by source builds, system discovery, and binding generation.
struct FlintInstallation {
    // Prefix containing flint/flint.h.
    include_dir: PathBuf,
    lib_dir: Option<PathBuf>,
    // External headers needed when bundled FLINT has a separate install prefix.
    #[cfg_attr(not(feature = "run-bindgen"), allow(dead_code))]
    dependency_include_dirs: Vec<PathBuf>,
}

impl FlintInstallation {
    #[cfg(any(not(feature = "use-system-libs"), target_env = "msvc"))]
    fn from_prefix(prefix: &Path) -> Self {
        Self {
            include_dir: prefix.join("include"),
            lib_dir: Some(prefix.join("lib")),
            dependency_include_dirs: Vec::new(),
        }
    }

    fn emit_metadata(&self) {
        if let Some(lib_dir) = &self.lib_dir {
            println!("cargo::metadata=LIB_DIR={}", lib_dir.display());
        }
        println!("cargo::metadata=INCLUDE_DIR={}", self.include_dir.display());
    }
}

struct Build {
    // Cargo build-script scratch directory.
    out_dir: PathBuf,
    // Bindings included by src/lib.rs from OUT_DIR.
    flint_rs: PathBuf,
    flint: FlintInstallation,
    source: FlintSource,
    target: Target,
}

enum FlintSource {
    Bundled,
    System,
}

impl Build {
    fn new() -> Result<Self> {
        let target = Target::from_env()?;
        // Cargo supplies an absolute path. Do not canonicalize it into a
        // Windows verbatim path, which GCC and libclang cannot reliably use.
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("Missing OUT_DIR")?);

        #[cfg(feature = "use-system-libs")]
        let flint = system::find(target)?;
        #[cfg(not(feature = "use-system-libs"))]
        let flint = FlintInstallation::from_prefix(&out_dir);

        Ok(Build {
            out_dir: out_dir.clone(),
            flint_rs: out_dir.join("flint.rs"),
            flint,
            source: if cfg!(feature = "use-system-libs") {
                FlintSource::System
            } else {
                FlintSource::Bundled
            },
            target,
        })
    }

    fn build_flint(&mut self) -> Result<()> {
        if matches!(self.source, FlintSource::Bundled) {
            self.build_bundled_flint()?;
        }
        self.flint.emit_metadata();
        Ok(())
    }

    fn build_bundled_flint(&mut self) -> Result<()> {
        #[cfg(target_env = "msvc")]
        if self.target == Target::WindowsMsvc {
            self.flint = msvc::build_bundled(&self.out_dir)?;
            return Ok(());
        }

        let tmp_dir = self.out_dir.join("tmp");
        std::fs::create_dir_all(&tmp_dir)
            .context(format!("Failed to create `{}`", tmp_dir.display()))?;

        anyhow::ensure!(
            Path::new("flint/src/flint.h.in").is_file(),
            "FLINT sources are missing; run `git submodule update --init --recursive`"
        );
        let flint_root_dir = source::prepare(Path::new("flint"), &self.out_dir)
            .context("Failed to prepare the FLINT source build tree")?;
        let tmp_dir = shell_path(&tmp_dir)?;
        let prefix = shell_path(&self.out_dir)?;
        anyhow::ensure!(
            !prefix.chars().any(char::is_whitespace),
            "FLINT's Makefiles require a build path without spaces: {prefix}"
        );

        if !flint_root_dir.join("configure").is_file() {
            let mut bootstrap = build_command("sh", &flint_root_dir, &tmp_dir)?;
            bootstrap.arg("./bootstrap.sh");
            run(bootstrap)?;
        }

        self.configure_flint(&flint_root_dir, &tmp_dir, &prefix)?;
        make(&flint_root_dir, &tmp_dir, MakeAction::Build)?;
        make(&flint_root_dir, &tmp_dir, MakeAction::Install)
    }

    fn configure_flint(&self, root: &Path, tmp_dir: &str, prefix: &str) -> Result<()> {
        let mut configure = build_command("sh", root, tmp_dir)?;
        configure.args(["./configure", "--prefix", prefix, "--disable-shared"]);
        if self.target == Target::WindowsGnu {
            configure.arg("ABI=64");
        }
        if cfg!(feature = "gmp-mpfr-sys") {
            let lib = shell_path(Path::new(&std::env::var("DEP_GMP_LIB_DIR")?))?;
            let include = shell_path(Path::new(&std::env::var("DEP_GMP_INCLUDE_DIR")?))?;
            for library in ["gmp", "mpfr"] {
                configure.arg(format!("--with-{library}-lib={lib}"));
                configure.arg(format!("--with-{library}-include={include}"));
            }
        }

        // Reconfigure only when compiler/options change, and clean because
        // FLINT's Makefiles do not track changes to compiler flags.
        let mut configuration = format!("{configure:?}");
        for name in CONFIGURE_ENV {
            configuration.push_str(&format!("\n{name}={:?}", std::env::var_os(name)));
        }
        let stamp = self.out_dir.join("flint-configure-command");
        let configured = root.join("Makefile").is_file();
        if configured && std::fs::read_to_string(&stamp).ok().as_deref() == Some(&configuration) {
            return Ok(());
        }
        if configured {
            make(root, tmp_dir, MakeAction::Clean)?;
        }
        run(configure)?;
        std::fs::write(stamp, configuration)?;
        Ok(())
    }

    fn emit_link_flags(&self) -> Result<()> {
        // The MSVC module emits FLINT and its vcpkg dependencies together.
        if self.target == Target::WindowsMsvc {
            return Ok(());
        }
        if matches!(self.source, FlintSource::Bundled) {
            let flint_lib_dir = self
                .flint
                .lib_dir
                .as_ref()
                .context("Missing bundled FLINT library directory")?;
            anyhow::ensure!(
                flint_lib_dir.join("libflint.a").is_file(),
                "Compilation is successful, but `libflint.a` is not where it should"
            );

            println!("cargo::rustc-link-lib=static=flint");
            println!("cargo::rustc-link-lib=mpfr");
            println!("cargo::rustc-link-lib=gmp");
            println!(
                "cargo::rustc-link-search=native={}",
                flint_lib_dir.display()
            );
        }

        if cfg!(feature = "gmp-mpfr-sys") {
            if let Ok(gmp_lib_dir) = std::env::var("DEP_GMP_LIB_DIR") {
                println!("cargo::rustc-link-search=native={gmp_lib_dir}");
            }
        }

        // Keep bundled GMP/MPFR ahead of MSYS2's import libraries in the search path.
        self.link_mingw_dependencies()
    }

    fn link_mingw_dependencies(&self) -> Result<()> {
        if self.target != Target::WindowsGnu || matches!(self.source, FlintSource::System) {
            return Ok(());
        }
        // Rust's bundled MinGW linker does not necessarily search the MSYS2
        // compiler's library directories. Locate the import libraries there.
        let compiler = cc::Build::new().get_compiler();
        for library in ["mpfr", "gmp", "pthread"] {
            if library != "pthread" && cfg!(feature = "gmp-mpfr-sys") {
                continue;
            }
            let directory = mingw_library_dir(&compiler, library)?;
            println!("cargo::rustc-link-search=native={}", directory.display());
        }
        println!("cargo::rustc-link-lib=pthread");
        Ok(())
    }
}

// Normal crate builds must not require libclang/bindgen. They use the checked-in
// file generated by `KEEP_BINDGEN_OUTPUT=1 cargo build --features run-bindgen`.
#[cfg(not(feature = "run-bindgen"))]
impl Build {
    fn prepare_bindings(&self) -> Result<()> {
        println!("cargo::rerun-if-changed={GENERATED_BINDINGS}");
        std::fs::copy(GENERATED_BINDINGS, &self.flint_rs)
            .context("Failed to copy pregenerated bindings")?;

        Ok(())
    }
}

#[cfg(feature = "run-bindgen")]
impl Build {
    fn prepare_bindings(&self) -> Result<()> {
        let bgen = runbindgen::BindingGeneration::new(&self.flint, &self.flint_rs, self.target)?;
        bgen.generate_bindings()
    }
}

fn main() -> Result<()> {
    println!("cargo::rerun-if-changed=build");
    println!("cargo::rerun-if-changed=flint");
    for name in CONFIGURE_ENV {
        println!("cargo::rerun-if-env-changed={name}");
    }
    #[cfg(windows)]
    println!("cargo::rerun-if-env-changed=ACLOCAL_PATH");
    let mut build = Build::new()?;

    build.build_flint()?;

    anyhow::ensure!(
        build.flint.include_dir.join("flint/flint.h").is_file(),
        "Compilation is successful, but `flint/flint.h` is not where it should"
    );

    build.emit_link_flags()?;

    build.prepare_bindings()?;

    anyhow::ensure!(build.flint_rs.is_file(), "Cannot find `flint.rs`");

    Ok(())
}
