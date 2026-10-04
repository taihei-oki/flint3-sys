use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::FlintInstallation;

const VCPKG_ENV: &[&str] = &[
    "VCPKG_ROOT",
    "VCPKGRS_TRIPLET",
    "VCPKGRS_DYNAMIC",
    "VCPKGRS_DISABLE",
    "NO_VCPKG",
];

fn static_crt() -> bool {
    std::env::var("CARGO_CFG_TARGET_FEATURE")
        .unwrap_or_default()
        .split(',')
        .any(|feature| feature == "crt-static")
}

fn track_vcpkg_environment(package: &str) {
    for name in VCPKG_ENV {
        println!("cargo::rerun-if-env-changed={name}");
    }
    let package = package.to_ascii_uppercase();
    for name in [
        format!("VCPKGRS_NO_{package}"),
        format!("{package}_NO_VCPKG"),
    ] {
        println!("cargo::rerun-if-env-changed={name}");
    }
}

fn validate_triplet(triplet: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(
            triplet,
            "x64-windows" | "x64-windows-static-md" | "x64-windows-static"
        ),
        "Unsupported MSVC vcpkg triplet `{triplet}`; use x64-windows, \
         x64-windows-static-md, or x64-windows-static"
    );
    anyhow::ensure!(
        static_crt() == (triplet == "x64-windows-static"),
        "vcpkg triplet `{triplet}` does not match Rust's CRT linkage; use \
         x64-windows-static with -C target-feature=+crt-static, or \
         x64-windows-static-md/x64-windows with Rust's default dynamic CRT"
    );
    Ok(())
}

fn probe(package: &str) -> Result<vcpkg::Library> {
    track_vcpkg_environment(package);
    // Delay link metadata until FLINT has been emitted before its dependencies.
    let library = vcpkg::Config::new()
        .cargo_metadata(false)
        .find_package(package)
        .with_context(|| {
            format!(
                "Failed to find MSVC dependency `{package}` in vcpkg; install it for \
                 x64-windows-static-md, or set VCPKGRS_TRIPLET=x64-windows and \
                 VCPKGRS_DYNAMIC=1 to use DLLs"
            )
        })?;
    validate_triplet(&library.vcpkg_triplet)?;
    Ok(library)
}

fn emit_link_metadata(libraries: &[&vcpkg::Library]) {
    let mut emitted = std::collections::HashSet::new();
    for library in libraries {
        for metadata in &library.cargo_metadata {
            // vcpkg-rs omits the library kind on MSVC. Mark static archives so
            // rustc bundles them into the rlib for downstream consumers.
            let metadata = match metadata.strip_prefix("cargo:rustc-link-lib=") {
                // Match FLINT's CMake target: use PThreads4W's C cleanup variant.
                // The exception variants contain duplicate pthread definitions.
                Some("pthreadVCE3" | "pthreadVSE3") => continue,
                Some(name) if library.is_static => {
                    format!("cargo:rustc-link-lib=static={name}")
                }
                _ => metadata.clone(),
            };
            if emitted.insert(metadata.clone()) {
                println!("{metadata}");
            }
        }
    }
}

#[cfg(feature = "use-system-libs")]
pub(super) fn probe_system_flint() -> Result<vcpkg::Library> {
    let library = probe("flint")?;
    emit_link_metadata(&[&library]);
    Ok(library)
}

fn include_dir<'a>(library: &'a vcpkg::Library, header: &str) -> Result<&'a Path> {
    library
        .include_paths
        .iter()
        .find(|path| path.join(header).is_file())
        .map(PathBuf::as_path)
        .with_context(|| format!("vcpkg did not report an include path containing `{header}`"))
}

fn library_file<'a>(library: &'a vcpkg::Library, name: &str) -> Result<&'a Path> {
    library
        .found_names
        .iter()
        .zip(&library.found_libs)
        .find(|(found, _)| *found == name || *found == &format!("lib{name}"))
        .map(|(_, path)| path.as_path())
        .with_context(|| format!("vcpkg did not report the `{name}` library"))
}

struct Dependencies {
    gmp: vcpkg::Library,
    mpfr: vcpkg::Library,
    pthreads: vcpkg::Library,
}

impl Dependencies {
    fn probe() -> Result<Self> {
        Ok(Self {
            gmp: probe("gmp")?,
            mpfr: probe("mpfr")?,
            pthreads: probe("pthreads")?,
        })
    }

    fn configure(&self, config: &mut cmake::Config) -> Result<()> {
        let gmp_include = include_dir(&self.gmp, "gmp.h")?;
        let mpfr_include = include_dir(&self.mpfr, "mpfr.h")?;
        let pthreads_include = include_dir(&self.pthreads, "pthread.h")?;
        let prefix = pthreads_include
            .parent()
            .context("vcpkg include directory has no parent")?;
        let installed_dir = prefix.parent().context("vcpkg prefix has no parent")?;
        let pthreads_config = prefix.join("share/PThreads4W");
        anyhow::ensure!(
            pthreads_config.join("PThreads4WConfig.cmake").is_file(),
            "vcpkg's PThreads4W CMake configuration is missing"
        );

        config
            .define("GMP_INCLUDE_DIR", gmp_include)
            .define("GMP_LIBRARY", library_file(&self.gmp, "gmp")?)
            .define("MPFR_INCLUDE_DIR", mpfr_include)
            .define("MPFR_LIBRARY", library_file(&self.mpfr, "mpfr")?)
            .define("PThreads4W_DIR", pthreads_config)
            .define("PThreads4W_INCLUDE_DIR", pthreads_include)
            .define("_VCPKG_INSTALLED_DIR", installed_dir)
            .define("VCPKG_TARGET_TRIPLET", &self.pthreads.vcpkg_triplet);
        // Refresh these cache entries if VCPKG_ROOT changes between builds.
        for (variable, name) in [
            ("PThreads4W_LIBRARY", "pthreadVC3"),
            ("PThreads4W_CXXEXC_LIBRARY", "pthreadVCE3"),
            ("PThreads4W_STRUCTEXC_LIBRARY", "pthreadVSE3"),
        ] {
            let path = library_file(&self.pthreads, name)?;
            config
                .define(variable, path)
                .define(format!("{variable}_RELEASE"), path);
        }
        if self.pthreads.is_static {
            config.cflag("/D__PTW32_STATIC_LIB");
        }
        Ok(())
    }

    fn include_dirs(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for library in [&self.gmp, &self.mpfr, &self.pthreads] {
            for path in &library.include_paths {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    fn emit_link_metadata(&self) {
        emit_link_metadata(&[&self.mpfr, &self.gmp, &self.pthreads]);
    }
}

fn cmake_config(out_dir: &Path, triplet: &str) -> cmake::Config {
    // Build outside the submodule. Use already-installed vcpkg packages directly,
    // without a vcpkg toolchain or an implicit dependency installation.
    let mut config = cmake::Config::new("flint");
    let static_crt = static_crt();
    config
        // Keep CMake's cached dependency paths separate for static and DLL builds.
        .out_dir(out_dir.join("msvc").join(triplet))
        // vcpkg-rs selects release libraries even for a debug Cargo profile.
        .profile("Release")
        .static_crt(static_crt)
        .define(
            "CMAKE_MSVC_RUNTIME_LIBRARY",
            if static_crt {
                "MultiThreaded"
            } else {
                "MultiThreadedDLL"
            },
        )
        // cmake-rs otherwise replaces Visual Studio's Release flags.
        .define("CMAKE_C_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG")
        .define("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("WITH_NTL", "OFF")
        .define("CMAKE_DISABLE_FIND_PACKAGE_CBLAS", "TRUE")
        .define("CMAKE_DISABLE_FIND_PACKAGE_PkgConfig", "TRUE")
        .define("CMAKE_INSTALL_LIBDIR", "lib");
    config
}

fn emit_bundled_link_metadata(lib_dir: &Path, dependencies: &Dependencies) -> Result<()> {
    anyhow::ensure!(
        lib_dir.join("flint.lib").is_file(),
        "Compilation is successful, but `flint.lib` is not where it should be"
    );
    println!("cargo::rustc-link-search=native={}", lib_dir.display());
    println!("cargo::rustc-link-lib=static=flint");
    dependencies.emit_link_metadata();
    Ok(())
}

pub(super) fn build_bundled(out_dir: &Path) -> Result<FlintInstallation> {
    anyhow::ensure!(
        Path::new("flint/src/flint.h.in").is_file(),
        "FLINT sources are missing; run `git submodule update --init --recursive`"
    );
    let dependencies = Dependencies::probe()?;
    let mut config = cmake_config(out_dir, &dependencies.pthreads.vcpkg_triplet);
    dependencies.configure(&mut config)?;
    let destination = config.build();
    emit_bundled_link_metadata(&destination.join("lib"), &dependencies)?;

    let mut installation = FlintInstallation::from_prefix(&destination);
    installation.dependency_include_dirs = dependencies.include_dirs();
    Ok(installation)
}
