use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{FlintInstallation, Target};

// Headers that we don't want to expose
static SKIP_HEADERS: &[&str] = &[
    // dependend on NTL
    r"^NTL-interface\.h$",
    // platform dependent variables. In principle, it could be interesting to
    // expose them, but since we ship the rust bindings, instead of building
    // them at each compilation, it makes not much sense.
    r"^config\.h$",
    r"^flint-config\.h$",
    r"^flint-mparam\.h$",
    r"^thread_pool\.h$",
    // Irrelevant, or too low-level (platform-dependent)
    r"^crt_helpers\.h$",
    r"^gettimeofday\.h$",
    r"^machine_vectors\.h$",
    r"^gmpcompat\.h$",
    r"^longlong.*\.h$",
    r"^fft_small\.h$",
    r"^profiler\.h$",
    r"^test_helpers\.h$",
    r"^.*templates\.h$",
    r"^.*_tables\.h$",
    r"^.*impl\.h$",
    // depend on GMP. FIXME
    r"^fft\.h$",
    r"^mpn_extras\.h$",
];

// Some headers repeat declarations from other headers. In principle
// bindgen can handle that, but since we call bindgen separately on every header
// file, it loses the information. We Keep this as a small hand-curated list
// rather than adding complex global dedup logic.
static SKIP_ITEMS: &[(&str, &[&str])] = &[
    (
        "flint.h",
        &["n_randlimb", "n_randtest", "n_randtest_not_zero"],
    ),
    (
        "gr_generic.h",
        &[
            "gr_generic_ctx_predicate",
            "gr_generic_ctx_predicate_true",
            "gr_generic_ctx_predicate_false",
        ],
    ),
    ("mpn_mod.h", &["gr_ctx_init_mpn_mod"]),
    // CMake also installs this internal header alongside acb_dirichlet.h.
    ("secondary_zeta.h", &["acb_dirichlet_secondary_zeta"]),
];

fn clang_path(path: &Path) -> Result<String> {
    let path = path.to_str().context("Non-Unicode header path")?;
    #[cfg(windows)]
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/");
    Ok(path.to_owned())
}

pub(super) struct BindingGeneration<'a> {
    flint: &'a FlintInstallation,

    // a folder where we copy the header files before (maybe) patching.
    tmp_dir: tempfile::TempDir,

    // Target file.
    flint_rs: &'a Path,

    target: Target,
}

impl<'a> BindingGeneration<'a> {
    pub(super) fn new(
        flint: &'a FlintInstallation,
        flint_rs: &'a Path,
        target: Target,
    ) -> Result<Self> {
        let tmp_dir = tempfile::tempdir()?;
        Ok(Self {
            flint,
            flint_rs,
            tmp_dir,
            target,
        })
    }

    // Temporary bindgen workaround for FLINT headers where `mpoly_void_ring_t` is
    // an array typedef over an anonymous struct. Bindgen then emits unstable
    // `_bindgen_ty_*` names in unrelated headers. Naming the struct gives bindgen a
    // stable Rust type. Remove this when upstream FLINT has the fix.
    fn patch_flint_mpoly_void_ring_type(&self) -> Result<()> {
        let header = &self.tmp_dir.path().join("mpoly_types.h");
        let source = std::fs::read_to_string(header)
            .with_context(|| format!("Failed to read `{}`", header.display()))?;

        if source.contains("mpoly_void_ring_struct") {
            return Ok(());
        }

        let patched = source.replace(
            "} mpoly_void_ring_t[1];",
            "} mpoly_void_ring_struct;\n\ntypedef mpoly_void_ring_struct mpoly_void_ring_t[1];",
        );
        anyhow::ensure!(
            patched != source,
            "Could not find anonymous `mpoly_void_ring_t` declaration in `{}`",
            header.display()
        );

        std::fs::write(header, patched)
            .with_context(|| format!("Failed to patch `{}`", header.display()))?;

        Ok(())
    }

    fn preserve_clang_attributes(&self) -> Result<()> {
        let header = self.tmp_dir.path().join("flint.h");
        let source = std::fs::read_to_string(&header)
            .with_context(|| format!("Failed to read `{}`", header.display()))?;

        // Clang's MSVC target does not define __GNUC__, so FLINT's fallback
        // erases __attribute__, including vector_size in Clang's intrinsics.
        // Remove only the empty macro in the bindgen overlay. Defining __GNUC__
        // instead would select different FLINT types and break the MSVC ABI.
        let empty_attribute = regex::Regex::new(
            r"(?m)^[\t ]*#[\t ]*define[\t ]+__attribute__\([\t ]*\w+[\t ]*\)[\t ]*\r?$",
        )?;
        let patched = empty_attribute.replace_all(&source, "");
        if patched != source {
            std::fs::write(&header, patched.as_bytes())
                .with_context(|| format!("Failed to patch `{}`", header.display()))?;
        }
        Ok(())
    }

    // Copy all FLINT headers to self.overlay_dir, and apply patches.
    // Returns all FLINT headers that bindgen should visit.
    fn flint_headers(&self) -> Result<Vec<PathBuf>> {
        let skip_patterns = SKIP_HEADERS
            .iter()
            .map(|pattern| regex::Regex::new(pattern))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Invalid SKIP_HEADERS regex")?;

        let header_dir = &self.flint.include_dir.join("flint");

        anyhow::ensure!(
            header_dir.join("flint.h").is_file(),
            "Cannot find FLINT header `flint/flint.h` in `{}`",
            self.flint.include_dir.display()
        );

        let mut headers = Vec::new();
        for entry in std::fs::read_dir(&header_dir)? {
            let path = entry?.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("h") {
                continue;
            }

            let Some(file_name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
                continue;
            };

            // Copy ALL the header files
            let overlay = self.tmp_dir.path().join(file_name);
            println!("cargo::rerun-if-changed={}", path.display());
            std::fs::copy(&path, &overlay)
                .with_context(|| format!("Failed to copy `{}`", path.display()))?;

            if skip_patterns.iter().any(|re| re.is_match(file_name)) {
                continue;
            }

            // Only return the header files that bindgen should inspect
            headers.push(overlay);
        }

        headers.sort();

        if self.target == Target::WindowsMsvc {
            self.preserve_clang_attributes()?;
        }
        self.patch_flint_mpoly_void_ring_type()?;

        Ok(headers)
    }

    pub(super) fn generate_bindings(&self) -> Result<()> {
        use std::io::Write;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        println!("cargo::rerun-if-env-changed=KEEP_BINDGEN_OUTPUT");
        let keep_bindgen_output = std::env::var_os("KEEP_BINDGEN_OUTPUT").is_some();
        anyhow::ensure!(
            !keep_bindgen_output || self.target != Target::WindowsMsvc,
            "MSVC bindings are target-specific and cannot replace bindgen/flint.rs; \
             unset KEEP_BINDGEN_OUTPUT"
        );

        // vcpkg installs GMP, MPFR and PThreads4W alongside FLINT. Clang does
        // not search this prefix automatically as it does MSYS2's prefix.
        let mut clang_args = vec![
            "-DFLINT_NOSTDARG".to_owned(),
            format!("-I{}", clang_path(&self.flint.include_dir)?),
        ];
        for include in &self.flint.dependency_include_dirs {
            clang_args.push(format!("-I{}", clang_path(include)?));
        }
        if let Ok(include) = std::env::var("DEP_GMP_INCLUDE_DIR") {
            clang_args.push(format!("-I{}", clang_path(Path::new(&include))?));
        }
        if self.target == Target::WindowsMsvc {
            // In particular, nmod.h uses a struct for ull_t under MSVC, but
            // __int128 under GNU compilers. Ignore libclang's default target.
            clang_args.push("--target=x86_64-pc-windows-msvc".to_owned());
        }

        let headers = self.flint_headers()?;

        // FLINT implements many inline functions by compiling module-specific
        // `inlines.c` files into libflint. Defining the matching `*_INLINES_C`
        // macros tells headers to expose those functions as external
        // declarations, so bindgen can generate Rust bindings without creating
        // wrapper C files.
        let inline_macro_pattern =
            regex::Regex::new(r"(?m)^\s*#\s*ifdef\s+([A-Z0-9_]+_INLINES_C)\b")
                .context("Invalid inline macro regex")?;
        let mut inline_macros = Vec::new();
        for header in &headers {
            let source = std::fs::read_to_string(header)
                .with_context(|| format!("Failed to read `{}`", header.display()))?;
            inline_macros.extend(
                inline_macro_pattern
                    .captures_iter(&source)
                    .map(|captures| captures[1].to_owned()),
            );
        }
        inline_macros.sort();
        inline_macros.dedup();

        let _flint_rs = std::fs::File::create(self.flint_rs)?;
        let mut flint_rs = std::io::BufWriter::new(_flint_rs);

        write!(flint_rs, "/*  automatically generated by rust-bindgen */")?;
        let header_strings = headers
            .iter()
            // Match Clang's forward-slash spelling in file allowlists.
            .map(|header| clang_path(header))
            .collect::<Result<Vec<_>>>()?;

        let mut generated = std::iter::repeat_with(|| None)
            .take(header_strings.len())
            .collect::<Vec<Option<(String, String)>>>();
        let next_header = Arc::new(AtomicUsize::new(0));
        let worker_count = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(header_strings.len());

        // Bindgen is invoked once per header. This keeps each clang parse small
        // and makes the work easy to parallelize. The final file is assembled in
        // sorted header order, so output remains deterministic.
        std::thread::scope(|scope| -> Result<()> {
            let mut tasks = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let next_header = Arc::clone(&next_header);
                let header_strings = &header_strings;
                let inline_macros = &inline_macros;
                let clang_args = &clang_args;
                tasks.push(
                    scope.spawn(move || -> Result<Vec<(usize, String, String)>> {
                        let mut generated = Vec::new();
                        loop {
                            let index = next_header.fetch_add(1, Ordering::Relaxed);
                            let Some(h) = header_strings.get(index).cloned() else {
                                break;
                            };

                            let header_name = Path::new(&h)
                                .file_name()
                                .and_then(std::ffi::OsStr::to_str)
                                .context("Header path has no file name")?;

                            let mut builder = bindgen::Builder::default()
                                .clang_args(clang_args)
                                .disable_header_comment()
                                .header(&h)
                                .allowlist_file(regex::escape(&h))
                                .allowlist_recursively(false)
                                // These platform-dependent aliases are defined
                                // once in src/lib.rs, including Win64's LLP64 ABI.
                                .blocklist_type("^(u|s)long$")
                                .blocklist_var(".*")
                                .blocklist_function(".*_mpn.*")
                                .blocklist_function(".*_mpz.*")
                                .derive_default(true)
                                .derive_copy(false)
                                .derive_debug(false)
                                .default_non_copy_union_style(
                                    bindgen::NonCopyUnionStyle::ManuallyDrop,
                                )
                                .generate_cstr(true)
                                .merge_extern_blocks(true)
                                .rust_target(bindgen::RustTarget::stable(82, 0).ok().unwrap())
                                .rust_edition(bindgen::RustEdition::Edition2021)
                                .layout_tests(false)
                                .formatter(bindgen::Formatter::Prettyplease);

                            if self.target == Target::WindowsMsvc {
                                // Bindgen force-includes this before parsing h.
                                builder = builder.header_contents(
                                    "flint_bindgen_msvc.h",
                                    include_str!("bindgen_msvc.h"),
                                );
                            }

                            // FLINT instantiates the fq matrix and polynomial
                            // APIs from `fq_{mat,poly}_templates.h`. Clang
                            // attributes those declarations to the included
                            // template file rather than to the public
                            // `fq*_mat.h` / `fq*_poly.h` entry point, so the
                            // per-header file allowlist above would otherwise
                            // discard the complete instantiated API.
                            if matches!(
                                header_name,
                                "fq_mat.h"
                                    | "fq_nmod_mat.h"
                                    | "fq_zech_mat.h"
                                    | "fq_poly.h"
                                    | "fq_nmod_poly.h"
                                    | "fq_zech_poly.h"
                            ) {
                                let stem = header_name
                                    .strip_suffix(".h")
                                    .context("FLINT header has no .h suffix")?;
                                builder = builder
                                    .allowlist_function(format!("^{stem}_.*"))
                                    // The referenced public types are emitted
                                    // from their own headers. Avoid moving
                                    // those declarations into this header's
                                    // output as an incidental bindgen effect.
                                    .allowlist_recursively(false);
                            }

                            for inline_macro in inline_macros {
                                builder = builder.clang_arg(format!("-D{inline_macro}"));
                            }

                            for (_, items) in SKIP_ITEMS
                                .iter()
                                .filter(|(header, _)| *header == header_name)
                            {
                                for item in *items {
                                    builder = builder.blocklist_function(format!("^{item}$"));
                                }
                            }
                            let bindings = builder.generate().with_context(|| {
                                format!("Failed to generate FLINT bindings for `{header_name}`")
                            })?;
                            let stem = Path::new(&h)
                                .file_stem()
                                .and_then(std::ffi::OsStr::to_str)
                                .context("Header path has no file stem")?
                                .to_owned();

                            // Each bindgen run starts anonymous names at
                            // `_bindgen_ty_1`. Prefix them with the header stem
                            // before concatenating all generated fragments.
                            generated.push((
                                index,
                                h,
                                bindings
                                    .to_string()
                                    .replace("_bindgen_", &format!("_{stem}_bindgen_")),
                            ));
                        }
                        Ok(generated)
                    }),
                );
            }

            for task in tasks {
                for (index, h, bindings) in task.join().expect("bindgen worker panicked")? {
                    generated[index] = Some((h, bindings));
                }
            }
            Ok(())
        })?;

        for entry in generated {
            let (h, bindings) = entry.context("Missing generated bindings")?;
            let file_name = std::path::Path::new(&h)
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or(&h);
            write!(flint_rs, "\n\n/* {} */\n\n{}", file_name, bindings)?;
        }

        flint_rs.flush()?;
        if keep_bindgen_output {
            std::fs::copy(self.flint_rs, "bindgen/flint.rs")
                .context("Failed to copy generated bindings")?;
        }

        Ok(())
    }
}
