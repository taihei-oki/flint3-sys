use std::path::PathBuf;

use anyhow::{Context, Result};

use super::Target;

pub(super) struct SystemFlint {
    pub include_dir: PathBuf,
    pub lib_dir: Option<PathBuf>,
}

impl SystemFlint {
    fn from_paths(
        provider: &str,
        include_paths: Vec<PathBuf>,
        link_paths: Vec<PathBuf>,
    ) -> Result<Self> {
        let include_dir = include_paths
            .into_iter()
            .find(|path| path.join("flint/flint.h").is_file())
            .with_context(|| {
                format!("{provider} did not report an include path containing `flint/flint.h`")
            })?;
        Ok(Self {
            include_dir,
            lib_dir: link_paths.into_iter().next(),
        })
    }
}

pub(super) fn find(target: Target) -> Result<SystemFlint> {
    let library = match target {
        Target::WindowsMsvc => find_with_vcpkg(),
        _ => find_with_pkg_config(),
    }?;
    println!("cargo::metadata=SYSTEM_LIB=1");
    Ok(library)
}

fn find_with_pkg_config() -> Result<SystemFlint> {
    let library = pkg_config::Config::new()
        .statik(false)
        .probe("flint")
        .context("Failed to find system FLINT with pkg-config")?;

    if !cfg!(feature = "run-bindgen") {
        validate_system_flint_version(&library.version)?;
    }

    for name in [
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
    ] {
        println!("cargo::rerun-if-env-changed={name}");
    }
    SystemFlint::from_paths("pkg-config", library.include_paths, library.link_paths)
}

fn find_with_vcpkg() -> Result<SystemFlint> {
    for name in [
        "VCPKG_ROOT",
        "VCPKGRS_TRIPLET",
        "VCPKGRS_DYNAMIC",
        "VCPKGRS_DISABLE",
        "VCPKGRS_NO_FLINT",
        "NO_VCPKG",
        "FLINT_NO_VCPKG",
    ] {
        println!("cargo::rerun-if-env-changed={name}");
    }
    let library = vcpkg::Config::new().find_package("flint").context(
        "Failed to find MSVC FLINT; install flint:x64-windows with vcpkg and set \
         VCPKGRS_TRIPLET=x64-windows and VCPKGRS_DYNAMIC=1",
    )?;
    anyhow::ensure!(
        !library.is_static && library.vcpkg_triplet == "x64-windows",
        "MSVC currently requires vcpkg's dynamic x64-windows triplet"
    );
    SystemFlint::from_paths("vcpkg", library.include_paths, library.link_paths)
}

fn validate_system_flint_version(version: &str) -> Result<()> {
    println!("cargo::rerun-if-changed=flint/VERSION");
    let bundled_version =
        std::fs::read_to_string("flint/VERSION").context("Failed to read `flint/VERSION`")?;
    let bundled_version = bundled_version.trim();
    let bundled_major_minor = major_minor(bundled_version)
        .with_context(|| format!("Could not parse bundled FLINT version `{bundled_version}`"))?;
    let system_major_minor = major_minor(version)
        .with_context(|| format!("Could not parse system FLINT version `{version}`"))?;

    anyhow::ensure!(
        bundled_major_minor == system_major_minor,
        "System FLINT version `{}` is incompatible with checked-in bindings for FLINT {}.x; \
         install a matching FLINT or enable `run-bindgen`",
        version,
        bundled_major_minor
    );

    Ok(())
}

fn major_minor(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    Some(format!("{}.{}", parts.next()?, parts.next()?))
}
